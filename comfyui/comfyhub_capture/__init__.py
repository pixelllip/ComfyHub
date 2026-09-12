"""ComfyHub 捕获扩展（ComfyUI 自定义节点）。

作用：把 ComfyUI 每一次跑完的运行（prompt 参数图 + 界面工作流 + 产出的文件）
**实时推送**给本机的 ComfyHub 后端：``POST /api/ingest/comfyui``。

设计原则是「**完全安全**」：ComfyHub 没开、端口不通、后端 500、甚至这里的代码自己
出异常，ComfyUI 都必须和没装这个扩展时一模一样。因此：

* 导入期所有动作都包在 try/except 里，失败只写一行日志，绝不让 ComfyUI 起不来；
* 推送在**单个守护线程**里异步做，有界队列（默认 256）满了就丢弃 + 告警，绝不阻塞执行；
* 只用标准库（urllib），不加任何依赖；
* 日志走 ``logging.getLogger("comfyhub_capture")``，正常情况只在启动时打印一行。

环境变量：
* ``COMFYHUB_URL``              后端地址（优先级最高）
* ``COMFYHUB_CAPTURE=0``        关闭扩展
* ``COMFYHUB_CAPTURE_DISABLED=1``  同上
* ``COMFYHUB_COMFYUI_URL``      覆盖上报给后端的 ComfyUI 地址
* ``COMFYHUB_CAPTURE_INCLUDE_TEMP=1``  连 temp 预览文件一起上报
"""

from __future__ import annotations

import logging
import os
import sys
import threading

log = logging.getLogger("comfyhub_capture")
if not log.handlers:
    # 只挂一个 NullHandler，真正的输出交给 ComfyUI 的 root logger，
    # 这样控制台里不会多出成片的日志。
    log.addHandler(logging.NullHandler())

_NODE_DIR = os.path.dirname(os.path.abspath(__file__))

if _NODE_DIR not in sys.path:
    sys.path.insert(0, _NODE_DIR)

try:  # capture_core 是纯标准库模块，理论上不会失败；失败也不能影响 ComfyUI
    from capture_core import (  # type: ignore
        CAPTURE_MARK,
        CaptureWorker,
        INGEST_PATH,
        RecentStore,
        build_payload,
        collect_outputs,
        is_capture_disabled,
        load_config,
        resolve_config,
    )
except Exception as _exc:  # pragma: no cover - 只在文件缺失时发生
    logging.getLogger("comfyhub_capture").warning("捕获扩展加载失败: %s", _exc)
    CAPTURE_MARK = "[ComfyHub] 捕获扩展已启用 -> {}"
    CaptureWorker = None
    RecentStore = None
    build_payload = None
    collect_outputs = None
    is_capture_disabled = lambda environ=None: True
    load_config = lambda path: {}
    resolve_config = lambda environ=None, file_config=None: {
        "url": "http://127.0.0.1:8080",
        "comfyUrl": "http://127.0.0.1:8188",
        "tags": [],
        "enabled": False,
        "includeTemp": False,
        "timeout": 5.0,
        "queueSize": 256,
        "delay": 0.5,
    }
    INGEST_PATH = "/api/ingest/comfyui"

# ComfyUI 会读这几个属性；本扩展没有节点，只做纯监听。
NODE_CLASS_MAPPINGS = {}
NODE_DISPLAY_NAME_MAPPINGS = {}
WEB_DIRECTORY = None

# 运行时状态 ---------------------------------------------------------------
_CONFIG = None
_WORKER = None
_ENABLED = False
_PROMPTS = None       # prompt_id -> 运行上下文（prompt / workflow / outputs）
_WORKFLOWS = None     # prompt 图指纹 -> 界面工作流（on_prompt 钩子写入的兜底）
_STATE = threading.local()
_PATCHED = False
_HANDLER_REGISTERED = threading.Event()


def _fingerprint(prompt):
    """给 API 格式 prompt 图算稳定指纹，用来在拿不到 prompt_id 时把 UI 工作流对上号。"""
    try:
        import hashlib
        import json

        return hashlib.sha1(
            json.dumps(prompt, sort_keys=True, ensure_ascii=False).encode("utf-8")
        ).hexdigest()
    except Exception:
        return None


def _log_line(message, level=logging.INFO):
    try:
        log.log(level, message)
    except Exception:
        pass


# ---------------------------------------------------------------------------
#  钩子 a：PromptServer.add_on_prompt_handler
# ---------------------------------------------------------------------------

def _on_prompt(json_data):
    """ComfyUI 收到 /prompt 请求时回调，这里先把 UI 工作流记下来。

    注意：0.34.2 里这个回调在生成 prompt_id **之前**执行（server.py:1076），
    所以拿不到 prompt_id；于是按 prompt 图的指纹存，等 ``execute_async`` 拿到
    prompt_id 后再取。正常情况下 ``extra_data.extra_pnginfo.workflow`` 也能直接拿到，
    这里只是多一层兜底。
    """
    try:
        if not isinstance(json_data, dict):
            return json_data
        prompt = json_data.get("prompt")
        if not isinstance(prompt, dict):
            return json_data
        extra = json_data.get("extra_data")
        workflow = None
        if isinstance(extra, dict):
            pnginfo = extra.get("extra_pnginfo")
            if isinstance(pnginfo, dict):
                workflow = pnginfo.get("workflow")
        if isinstance(workflow, dict) and _WORKFLOWS is not None:
            key = _fingerprint(prompt)
            if key:
                _WORKFLOWS.put(key, workflow)
    except Exception as exc:
        _log_line("on_prompt 钩子异常（已忽略）: %s" % exc, logging.WARNING)
    return json_data


def _try_register_prompt_handler():
    """尽早把 on_prompt 钩子挂到当前 PromptServer 实例上。

    ComfyUI 的启动顺序是 main.py:536 先建 PromptServer 实例，之后
    main.py:542 才导入 custom_nodes —— 所以导入期通常已经能拿到 instance。
    拿不到也不要紧：send_sync 的包装会在第一次收到消息时再试一次。
    """
    if _HANDLER_REGISTERED.is_set():
        return True
    try:
        import server as comfy_server

        instance = getattr(comfy_server.PromptServer, "instance", None)
        if instance is None:
            return False
        handlers = getattr(instance, "on_prompt_handlers", None)
        if handlers is None:
            return False
        if _on_prompt not in handlers:
            instance.add_on_prompt_handler(_on_prompt)
            _log_line("已注册 on_prompt 钩子（用于捕获界面工作流）", logging.DEBUG)
        _HANDLER_REGISTERED.set()
        return True
    except Exception as exc:
        _log_line("注册 on_prompt 钩子失败（已忽略）: %s" % exc, logging.DEBUG)
        return False


# ---------------------------------------------------------------------------
#  运行上下文
# ---------------------------------------------------------------------------

def _get_ctx():
    return getattr(_STATE, "ctx", None)


def _set_ctx(ctx):
    _STATE.ctx = ctx


def _accumulate_outputs(node_id, output, prompt_id=None):
    """把一条 ``executed`` 消息里的产出并进该次运行的收集桶。

    消息结构（execution.py:578）：``{"node": "9", "display_node": "9",
    "output": {"images": [{filename, subfolder, type}, ...]}, "prompt_id": ...}``

    这里把 ``node``（真正的节点 id）显式传进来当桶的键，``output`` 里的键
    （images / gifs / audio ...）是节点返回的 UI 键，两者不能混。
    """
    if not prompt_id or not _PROMPTS:
        return
    ctx = _PROMPTS.get(str(prompt_id))
    if ctx is None:
        return
    bucket = ctx.setdefault("outputs", {})
    target = bucket.setdefault(str(node_id), {})
    if isinstance(output, dict):
        for key, value in output.items():
            if isinstance(value, (list, tuple)):
                target.setdefault(key, []).extend(value)


def _mark_done(prompt_id, status="success", error=None):
    """把该次运行标记为完成；真正的推送统一放在 ``execute_async`` 的 finally 里。"""
    if not prompt_id or _PROMPTS is None:
        return
    ctx = _PROMPTS.get(str(prompt_id))
    if ctx is None:
        return
    ctx["status"] = status
    if error is not None:
        ctx["error"] = error


# ---------------------------------------------------------------------------
#  挂钩
# ---------------------------------------------------------------------------

def _install_hooks():
    """挂上 0.34.2 里真实存在的钩子。全部包 try/except，失败就退化成「什么都不做」。"""
    global _PATCHED, _PROMPTS, _WORKFLOWS
    if _PATCHED:
        return []
    installed = []

    import execution as comfy_execution
    import server as comfy_server

    _PROMPTS = _PROMPTS or RecentStore(64)
    _WORKFLOWS = _WORKFLOWS or RecentStore(64)

    # --- 钩子 b1：PromptExecutor.execute_async ------------------------------
    # 这是主执行路径上唯一同时拿得到 prompt_id / API prompt / extra_data 的地方
    # （execution.py:730），而且它自带 finally，异常结束也能收尾。
    original_execute_async = comfy_execution.PromptExecutor.execute_async
    if not getattr(original_execute_async, "_comfyhub_wrapped", False):

        async def execute_async_with_capture(self, prompt, prompt_id, extra_data={}, execute_outputs=[]):
            ctx = None
            try:
                prompt_id = str(prompt_id)
                extra_data = extra_data if isinstance(extra_data, dict) else {}
                workflow = None
                pnginfo = extra_data.get("extra_pnginfo")
                if isinstance(pnginfo, dict):
                    workflow = pnginfo.get("workflow")
                if not isinstance(workflow, dict):
                    key = _fingerprint(prompt)
                    if key:
                        workflow = _WORKFLOWS.get(key)
                ctx = {
                    "promptId": prompt_id,
                    "prompt": prompt,
                    "workflow": workflow if isinstance(workflow, dict) else None,
                    "clientId": extra_data.get("client_id"),
                    "outputs": {},
                    "status": "success",
                    "error": None,
                }
                _PROMPTS.put(prompt_id, ctx)
            except Exception as exc:
                _log_line("捕获上下文准备失败（已忽略）: %s" % exc, logging.WARNING)
                ctx = None
            _set_ctx(ctx)
            try:
                return await original_execute_async(self, prompt, prompt_id, extra_data, execute_outputs)
            finally:
                _set_ctx(None)
                try:
                    if ctx is not None:
                        _finalize(ctx)
                except Exception as exc:
                    _log_line("收尾推送失败（已忽略）: %s" % exc, logging.WARNING)

        execute_async_with_capture._comfyhub_wrapped = True
        comfy_execution.PromptExecutor.execute_async = execute_async_with_capture
        installed.append("PromptExecutor.execute_async")

    # --- 钩子 b2：PromptServer.send_sync ------------------------------------
    # 只观察，不改语义：原样转发 *args/**kwargs 并返回原值。
    original_send_sync = comfy_server.PromptServer.send_sync
    if not getattr(original_send_sync, "_comfyhub_wrapped", False):

        def send_sync_with_capture(self, event, data=None, sid=None, *args, **kwargs):
            try:
                if not _HANDLER_REGISTERED.is_set():
                    _try_register_prompt_handler()
                if isinstance(data, dict):
                    prompt_id = data.get("prompt_id")
                    if event == "executed":
                        _accumulate_outputs(data.get("node"), data.get("output"), prompt_id)
                    elif event == "executing" and data.get("node") is None and prompt_id:
                        _mark_done(prompt_id, "success", None)
                    elif event == "execution_success" and prompt_id:
                        _mark_done(prompt_id, "success", None)
                    elif event == "execution_error" and prompt_id:
                        _mark_done(
                            prompt_id,
                            "error",
                            data.get("exception_message")
                            or data.get("exception_type")
                            or "execution_error",
                        )
            except Exception:
                pass  # 观察者出错绝不能影响 ComfyUI 的推送
            return original_send_sync(self, event, data, sid, *args, **kwargs)

        send_sync_with_capture._comfyhub_wrapped = True
        comfy_server.PromptServer.send_sync = send_sync_with_capture
        installed.append("PromptServer.send_sync")

    _PATCHED = True
    return installed


# ---------------------------------------------------------------------------
#  收尾：组装 JSON + 交给后台线程
# ---------------------------------------------------------------------------

def _finalize(ctx):
    prompt_id = ctx.get("promptId")
    if not prompt_id:
        return
    prompt = ctx.get("prompt")
    node_types = {}
    if isinstance(prompt, dict):
        for node_id, node in prompt.items():
            if isinstance(node, dict) and node.get("class_type"):
                node_types[str(node_id)] = str(node["class_type"])

    output_dir = None
    try:
        import folder_paths  # ComfyUI 自带

        output_dir = folder_paths.get_directory_by_type("output")
    except Exception:
        output_dir = None

    outputs = ctx.get("outputs") or {}
    listed = []
    if isinstance(outputs, dict) and collect_outputs is not None:
        listed = collect_outputs(outputs, node_types, include_temp=_CONFIG["includeTemp"])

    _PROMPTS.pop(prompt_id, None)

    # 没有任何产出文件的运行不打扰后端（纯参数试验不占库）
    if not listed and not ctx.get("error"):
        _log_line("runKey=%s 没有可上报的产出文件，跳过" % prompt_id, logging.DEBUG)
        return

    payload = build_payload(
        prompt_id,
        prompt,
        listed,
        ctx.get("workflow"),
        status=ctx.get("status") or "success",
        error=ctx.get("error"),
        output_dir=output_dir,
        comfy_url=_CONFIG["comfyUrl"],
        client_id=ctx.get("clientId"),
        tags=_CONFIG["tags"],
        include_temp=_CONFIG["includeTemp"],
        node_types=node_types,
    )
    if _WORKER is not None:
        _WORKER.submit(payload)


def _print_status(installed):
    """启动时只打印一行中文日志（钩子明细走 DEBUG，不进控制台）。"""
    line = CAPTURE_MARK.format(_CONFIG["url"])
    try:
        log.info(line)
    except Exception:
        pass
    try:
        print(line, flush=True)
    except Exception:
        pass
    if installed:
        _log_line("已挂载钩子: %s" % ", ".join(installed), logging.DEBUG)


def _boot():
    """导入期入口：读配置 -> 挂钩子 -> 建后台线程。任何异常都吞掉。"""
    global _CONFIG, _WORKER, _ENABLED

    if is_capture_disabled():
        _log_line("COMFYHUB_CAPTURE 已关闭，捕获扩展不启用")
        return False

    _CONFIG = resolve_config(os.environ, load_config(os.path.join(_NODE_DIR, "config.json")))
    if not _CONFIG.get("enabled", True):
        _log_line("config.json 里 enabled=false，捕获扩展不启用")
        return False

    if CaptureWorker is None:
        _log_line("capture_core 不可用，捕获扩展不启用", logging.WARNING)
        return False

    installed = _install_hooks()
    _try_register_prompt_handler()

    _WORKER = CaptureWorker(
        _CONFIG["url"],
        queue_size=_CONFIG.get("queueSize", 256),
        timeout=_CONFIG.get("timeout", 5.0),
        max_attempts=3,
        backoff=0.5,
        delay=_CONFIG.get("delay", 0.5),
    )
    _ENABLED = True
    _print_status(installed)
    return True


try:
    _boot()
except Exception as _exc:  # 导入期绝不能让 ComfyUI 挂掉
    _log_line("捕获扩展初始化失败（ComfyUI 不受影响）: %s" % _exc, logging.WARNING)
    _ENABLED = False

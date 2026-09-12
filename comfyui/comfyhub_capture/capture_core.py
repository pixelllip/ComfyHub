"""ComfyHub 捕获扩展 —— 纯逻辑层。

这个模块只依赖 Python 标准库，**不导入 torch / comfy / ComfyUI 的任何东西**，
因此可以在任意 Python 3.10+ 下直接导入做单元测试与端到端 HTTP 测试：:

    python -c "import capture_core; print(capture_core.build_payload(...))"

``__init__.py`` 负责把这里的能力挂到 ComfyUI 的钩子上；这里只做三件事：

1. 把一次运行（prompt graph + workflow + 产出文件）组装成 ComfyHub
   ``POST /api/ingest/comfyui`` 需要的 JSON（``build_payload``）；
2. 按「产出节点 + 扩展名」判断产物类型 IMAGE / VIDEO / AUDIO（``detect_kind``）；
3. 一个后台守护线程 + 有界队列 + 指数退避重试的推送器（``CaptureWorker``），
   任何异常都只写日志、绝不向上抛，保证 ComfyUI 行为完全不变。
"""

from __future__ import annotations

import json
import logging
import os
import queue
import threading
import time
import urllib.error
import urllib.request
from collections import OrderedDict

__all__ = [
    "CAPTURE_NAME",
    "CAPTURE_MARK",
    "DEFAULT_COMFYHUB_URL",
    "INGEST_PATH",
    "KIND_IMAGE",
    "KIND_VIDEO",
    "KIND_AUDIO",
    "OUTPUT_KEYS",
    "IMAGE_EXTENSIONS",
    "VIDEO_EXTENSIONS",
    "AUDIO_EXTENSIONS",
    "ANIMATED_NODE_HINTS",
    "VIDEO_NODE_HINTS",
    "is_capture_disabled",
    "load_config",
    "resolve_config",
    "extension_of",
    "detect_kind",
    "collect_outputs",
    "build_payload",
    "post_json_once",
    "post_with_retry",
    "CaptureWorker",
]

log = logging.getLogger("comfyhub_capture")

# 成功启用时打印的那一行中文日志（要求「刚好一行」）
CAPTURE_MARK = "[ComfyHub] 捕获扩展已启用 -> {}"
CAPTURE_NAME = "comfyhub_capture"
DEFAULT_COMFYHUB_URL = "http://127.0.0.1:8080"
DEFAULT_COMFYUI_URL = "http://127.0.0.1:8188"
INGEST_PATH = "/api/ingest/comfyui"

KIND_IMAGE = "IMAGE"
KIND_VIDEO = "VIDEO"
KIND_AUDIO = "AUDIO"

# ComfyUI 的 /history outputs 里可能出现的 list 键，全部当成候选产出。
#   images      -> SaveImage / PreviewImage / SaveAnimatedWEBP ...
#   gifs        -> VHS_VideoCombine（videohelpersuite）
#   audio/video -> 音频、视频类保存节点
#   files       -> 自定义节点（kjnodes 等）的通用文件列表
OUTPUT_KEYS = ("images", "gifs", "audio", "video", "videos", "files", "latents")

IMAGE_EXTENSIONS = frozenset({"png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "avif"})
VIDEO_EXTENSIONS = frozenset({"mp4", "webm", "mkv", "mov", "avi", "m4v", "gif"})
AUDIO_EXTENSIONS = frozenset({"flac", "mp3", "wav", "ogg", "m4a", "opus", "aac"})

# 会产出「动图 / 视频」的节点类名特征（小写匹配）。
# 说明：SaveAnimatedWEBP / SaveAnimatedPNG 走的是 "images" 键、扩展名是 webp/png，
# 靠扩展名分不出来，只能看节点类名。
ANIMATED_NODE_HINTS = ("saveanimated", "animated", "video", "combine", "vhs_", "gif", "webm", "av1")
VIDEO_NODE_HINTS = (
    "videocombine",
    "savevideo",
    "savewebm",
    "vhs_",
    "saveanimatedwebp",
    "saveanimatedpng",
    "animated",
    "sequence",
)


# ---------------------------------------------------------------------------
#  配置
# ---------------------------------------------------------------------------

def is_capture_disabled(environ=None) -> bool:
    """COMFYHUB_CAPTURE=0 或 COMFYHUB_CAPTURE_DISABLED=1 时关闭扩展。"""
    env = os.environ if environ is None else environ
    for name, off_values in (("COMFYHUB_CAPTURE", {"0", "false", "no", "off"}),):
        raw = env.get(name)
        if raw is not None and str(raw).strip().lower() in off_values:
            return True
    raw = env.get("COMFYHUB_CAPTURE_DISABLED")
    if raw is not None and str(raw).strip().lower() in {"1", "true", "yes", "on"}:
        return True
    return False


def load_config(config_path) -> dict:
    """读节点目录下的 config.json；不存在或坏掉都返回空 dict（绝不抛）。"""
    try:
        if config_path and os.path.isfile(config_path):
            with open(config_path, "r", encoding="utf-8") as fh:
                data = json.load(fh)
            if isinstance(data, dict):
                return data
    except Exception as exc:  # 配置坏了也不能影响 ComfyUI
        log.warning("读取 config.json 失败，改用默认配置: %s", exc)
    return {}


def resolve_config(environ=None, file_config=None) -> dict:
    """解析最终生效的配置。

    优先级：环境变量 COMFYHUB_URL > 节点目录 config.json > 默认 127.0.0.1:8080。
    """
    env = os.environ if environ is None else environ
    cfg = dict(file_config or {})

    url = env.get("COMFYHUB_URL") or cfg.get("comfyhubUrl") or cfg.get("comfyhub_url")
    url = str(url).strip() if url else DEFAULT_COMFYHUB_URL
    if not url.startswith(("http://", "https://")):
        url = "http://" + url
    url = url.rstrip("/")

    comfy_url = env.get("COMFYHUB_COMFYUI_URL") or cfg.get("comfyUrl") or DEFAULT_COMFYUI_URL
    comfy_url = str(comfy_url).strip().rstrip("/")

    tags = cfg.get("tags")
    if not isinstance(tags, list):
        tags = []
    tags = [str(t) for t in tags if str(t).strip()]

    enabled = cfg.get("enabled", True)
    if isinstance(enabled, str):
        enabled = enabled.strip().lower() not in {"0", "false", "no", "off"}

    include_temp = bool(cfg.get("includeTemp", False)) or str(
        env.get("COMFYHUB_CAPTURE_INCLUDE_TEMP", "")
    ).strip().lower() in {"1", "true", "yes", "on"}

    return {
        "url": url,
        "comfyUrl": comfy_url,
        "tags": tags,
        "enabled": bool(enabled),
        "includeTemp": include_temp,
        "timeout": float(cfg.get("timeoutSeconds", 5.0) or 5.0),
        "queueSize": int(cfg.get("queueSize", 256) or 256),
        "delay": float(cfg.get("delaySeconds", 0.5) or 0.0),
    }


# ---------------------------------------------------------------------------
#  产物类型识别
# ---------------------------------------------------------------------------

def extension_of(filename) -> str:
    """取小写扩展名（不含点），没有扩展名返回空串。"""
    name = str(filename or "")
    idx = name.rfind(".")
    if idx < 0 or idx == len(name) - 1:
        return ""
    return name[idx + 1:].lower()


def _node_is_video_like(node_type) -> bool:
    cls = str(node_type or "").lower()
    if not cls:
        return False
    return any(hint in cls for hint in VIDEO_NODE_HINTS) or any(
        hint in cls for hint in ANIMATED_NODE_HINTS
    )


def detect_kind(filename, node_type=None, out_key=None) -> str:
    """按扩展名 + 产出节点类名判断产物类型：IMAGE / VIDEO / AUDIO。

    - 音频扩展名 -> AUDIO
    - mp4/webm/mkv/mov/avi/m4v -> VIDEO
    - gif/webp 只有「产出节点是视频 / 动图类」时才是 VIDEO，否则 IMAGE
      （SaveAnimatedWEBP 判 VIDEO，SaveImage 存的普通 webp 判 IMAGE）
    - 其余图片扩展名 -> IMAGE
    """
    ext = extension_of(filename)
    if ext in AUDIO_EXTENSIONS:
        return KIND_AUDIO
    if ext in VIDEO_EXTENSIONS:
        if ext in {"gif", "webp"}:
            return KIND_VIDEO if _node_is_video_like(node_type) else KIND_IMAGE
        return KIND_VIDEO
    if ext in IMAGE_EXTENSIONS:
        # SaveAnimatedWEBP / SaveAnimatedPNG 这类节点，即便扩展名是 png 也当动图处理
        if ext == "webp" and _node_is_video_like(node_type):
            return KIND_VIDEO
        return KIND_IMAGE
    # 未知扩展名：如果节点明显是视频节点就先当视频，否则保守当图片
    if _node_is_video_like(node_type):
        return KIND_VIDEO
    return KIND_IMAGE


# ---------------------------------------------------------------------------
#  产出收集
# ---------------------------------------------------------------------------

def collect_outputs(node_outputs, node_types=None, include_temp: bool = False):
    """把 /history 的 outputs（node_id -> {key: [item, ...]}）整理成推送列表。

    - ``include_temp=False`` 时跳过 ``type == "temp"``（预览图）的条目
    - 按 (filename, subfolder, type) 去重，保持首次出现顺序
    - ``node_types`` 是 node_id -> class_type 的映射，用来判断动图 / 视频节点
    """
    node_types = node_types or {}
    seen = set()
    items = []
    if not isinstance(node_outputs, dict):
        return items

    for node_id, output in node_outputs.items():
        if not isinstance(output, dict):
            continue
        node_type = node_types.get(str(node_id)) or node_types.get(node_id) or ""
        for key in OUTPUT_KEYS:
            group = output.get(key)
            if not isinstance(group, (list, tuple)):
                continue
            for entry in group:
                if not isinstance(entry, dict):
                    continue
                filename = entry.get("filename")
                if not filename:
                    continue
                filename = str(filename)
                subfolder = str(entry.get("subfolder") or "")
                item_type = str(entry.get("type") or "output")
                if item_type == "temp" and not include_temp:
                    continue
                dedup_key = (filename, subfolder, item_type)
                if dedup_key in seen:
                    continue
                seen.add(dedup_key)
                items.append(
                    {
                        "filename": filename,
                        "subfolder": subfolder,
                        "type": item_type,
                        "kind": detect_kind(filename, node_type, key),
                        "nodeId": str(node_id),
                        "nodeType": str(node_type),
                    }
                )
    return items


def _prompt_node_types(prompt) -> dict:
    """从 API 格式 prompt 里取 node_id -> class_type。"""
    types = {}
    if isinstance(prompt, dict):
        for node_id, node in prompt.items():
            if isinstance(node, dict):
                cls = node.get("class_type")
                if cls:
                    types[str(node_id)] = str(cls)
    return types


def _error_text(error) -> str | None:
    if error is None:
        return None
    if isinstance(error, str):
        return error
    try:
        return json.dumps(error, ensure_ascii=False)[:2000]
    except Exception:
        return str(error)[:2000]


# ---------------------------------------------------------------------------
#  载荷组装
# ---------------------------------------------------------------------------

def build_payload(
    prompt_id,
    prompt,
    outputs=None,
    workflow=None,
    *,
    status: str = "success",
    error=None,
    output_dir=None,
    comfy_url=None,
    client_id=None,
    tags=None,
    include_temp: bool = False,
    node_types=None,
    captured_at=None,
):
    """组装 /api/ingest/comfyui 的请求体。

    ``outputs`` 可以是两种形态：

    - /history 里的 ``outputs`` 字典（node_id -> {images|gifs|...: [...]}），
      由本函数调用 :func:`collect_outputs` 解析；
    - 已经解析好的产出列表（list[dict]），直接透传。
    """
    if isinstance(outputs, list):
        items = list(outputs)
    else:
        types = node_types or _prompt_node_types(prompt)
        items = collect_outputs(outputs or {}, types, include_temp=include_temp)

    if isinstance(prompt, dict):
        types = node_types or _prompt_node_types(prompt)
    else:
        types = node_types or {}

    # 节点类名兜底：调用方只给了 outputs 列表时，用 prompt 补 nodeType
    for item in items:
        if not item.get("nodeType"):
            item["nodeType"] = str(types.get(str(item.get("nodeId"))) or "")

    payload = {
        "runKey": str(prompt_id),
        "comfyUrl": comfy_url or DEFAULT_COMFYUI_URL,
        "outputDir": output_dir,
        "status": str(status or "success"),
        "error": _error_text(error),
        "prompt": prompt if isinstance(prompt, dict) else None,
        "workflow": workflow if isinstance(workflow, dict) else None,
        "outputs": items,
        "extra": {
            "clientId": client_id,
            "tags": [str(t) for t in (tags or [])],
        },
    }
    if captured_at is not None:
        payload["extra"]["capturedAt"] = captured_at
    return payload


# ---------------------------------------------------------------------------
#  HTTP 推送（stdlib urllib）
# ---------------------------------------------------------------------------

def post_json_once(url, payload, timeout: float = 5.0):
    """POST 一次 JSON，返回 (状态码, 响应文本)。非 2xx 由调用方决定是否重试。"""
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=body,
        headers={"Content-Type": "application/json; charset=utf-8"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            status = getattr(response, "status", None) or response.getcode()
            raw = response.read()
            return int(status), raw.decode("utf-8", "replace")
    except urllib.error.HTTPError as exc:  # 4xx / 5xx 也算「拿到了响应」
        try:
            raw = exc.read()
        except Exception:
            raw = b""
        return int(exc.code), raw.decode("utf-8", "replace")


def post_with_retry(
    url,
    payload,
    *,
    poster=None,
    timeout: float = 5.0,
    max_attempts: int = 3,
    backoff: float = 0.5,
    max_backoff: float = 8.0,
    sleep=time.sleep,
):
    """带指数退避的推送。返回 (是否成功, 最后一次的状态码或 None, 说明文本)。

    - 2xx 视为成功，直接返回
    - 4xx 是「请求本身有问题」，重试也没用，直接放弃（避免刷日志）
    - 5xx / 网络异常 / 超时按指数退避重试，最多 ``max_attempts`` 次
    - 任何异常都被吞掉，只以返回值表达失败
    """
    poster = poster or post_json_once
    attempts = max(1, int(max_attempts))
    delay = float(backoff)
    last_status = None
    last_message = "未执行"

    for attempt in range(1, attempts + 1):
        try:
            status, text = poster(url, payload, timeout=timeout)
            last_status, last_message = status, text
            if 200 <= int(status) < 300:
                return True, int(status), text
            if 400 <= int(status) < 500:
                return False, int(status), text
        except Exception as exc:  # 连接被拒 / 超时 / DNS ...
            last_status, last_message = None, "{}: {}".format(type(exc).__name__, exc)

        if attempt < attempts:
            sleep(delay)
            delay = min(delay * 2 if delay > 0 else 0.5, max_backoff)

    return False, last_status, last_message


# ---------------------------------------------------------------------------
#  后台推送线程
# ---------------------------------------------------------------------------

class CaptureWorker:
    """单守护线程 + 有界队列的异步推送器。

    队列满了就丢弃并告警一行，绝不阻塞 ComfyUI 的执行线程。
    """

    def __init__(
        self,
        url,
        *,
        queue_size: int = 256,
        timeout: float = 5.0,
        max_attempts: int = 3,
        backoff: float = 0.5,
        delay: float = 0.5,
        poster=None,
        sleep=time.sleep,
        on_result=None,
    ):
        self.url = (
            (url.rstrip("/") if url else DEFAULT_COMFYHUB_URL) + INGEST_PATH
        )
        self.timeout = timeout
        self.max_attempts = max_attempts
        self.backoff = backoff
        self.delay = delay
        self.poster = poster
        self.sleep = sleep
        self.on_result = on_result
        self.queue = queue.Queue(maxsize=max(1, int(queue_size)))
        self._thread = None
        self._lock = threading.Lock()
        self._stopping = threading.Event()
        self.dropped = 0
        self.sent = 0
        self.failed = 0

    # -- 入队 ---------------------------------------------------------------
    def submit(self, payload) -> bool:
        """入队；满则丢弃 + 告警一行。返回是否入队成功。"""
        try:
            self.queue.put_nowait(payload)
        except queue.Full:
            self.dropped += 1
            log.warning(
                "推送队列已满（%d 条待发），丢弃本次捕获 runKey=%s",
                self.queue.maxsize,
                (payload or {}).get("runKey"),
            )
            return False
        except Exception as exc:
            log.warning("捕获任务入队失败: %s", exc)
            return False
        self._ensure_thread()
        return True

    # -- 线程管理 -----------------------------------------------------------
    def _ensure_thread(self):
        if self._thread is not None and self._thread.is_alive():
            return
        with self._lock:
            if self._thread is not None and self._thread.is_alive():
                return
            self._thread = threading.Thread(
                target=self._run,
                name="comfyhub-capture",
                daemon=True,
            )
            self._thread.start()

    def stop(self, timeout: float = 2.0):
        self._stopping.set()
        try:
            self.queue.put_nowait(None)
        except Exception:
            pass
        thread = self._thread
        if thread is not None and thread.is_alive():
            thread.join(timeout=timeout)

    def _run(self):  # pragma: no cover - 由测试单独驱动
        while not self._stopping.is_set():
            try:
                payload = self.queue.get(timeout=0.5)
            except queue.Empty:
                continue
            except Exception:
                continue
            try:
                if payload is None:
                    return
                self._deliver(payload)
            except Exception as exc:  # 兜底：任何异常都不能让线程死掉
                log.warning("捕获线程异常（已忽略）: %s", exc)
            finally:
                try:
                    self.queue.task_done()
                except Exception:
                    pass

    def _deliver(self, payload):
        if self.delay > 0:
            # 稍微等一下，让 ComfyUI 把文件写完 / 落盘
            self.sleep(self.delay)
        ok, status, text = post_with_retry(
            self.url,
            payload,
            poster=self.poster,
            timeout=self.timeout,
            max_attempts=self.max_attempts,
            backoff=self.backoff,
            sleep=self.sleep,
        )
        if ok:
            self.sent += 1
            if self.on_result is not None:
                try:
                    self.on_result(payload, status, text)
                except Exception:
                    pass
        else:
            self.failed += 1
            log.warning(
                "推送失败（已放弃）runKey=%s url=%s status=%s detail=%s",
                (payload or {}).get("runKey"),
                self.url,
                status,
                (text or "")[:200],
            )

    def drain(self, timeout: float = 10.0):
        """测试用：等队列排空。"""
        deadline = time.time() + timeout
        while time.time() < deadline:
            if self.queue.empty():
                return True
            self.sleep(0.02)
        return self.queue.empty()

    @property
    def stats(self):
        return {
            "sent": self.sent,
            "failed": self.failed,
            "dropped": self.dropped,
            "pending": self.queue.qsize(),
        }


# ---------------------------------------------------------------------------
#  最近运行的小缓存（供 __init__.py 使用；纯逻辑，便于测试）
# ---------------------------------------------------------------------------

class RecentStore:
    """定长、线程安全、按 key 覆盖的 OrderedDict 包装。"""

    def __init__(self, max_items: int = 64):
        self.max_items = max(1, int(max_items))
        self._data = OrderedDict()
        self._lock = threading.Lock()

    def put(self, key, value):
        with self._lock:
            self._data[key] = value
            self._data.move_to_end(key)
            while len(self._data) > self.max_items:
                self._data.popitem(last=False)
        return value

    def get(self, key, default=None):
        with self._lock:
            return self._data.get(key, default)

    def pop(self, key, default=None):
        with self._lock:
            return self._data.pop(key, default)

    def keys(self):
        with self._lock:
            return list(self._data.keys())

    def __len__(self):
        with self._lock:
            return len(self._data)

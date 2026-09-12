# -*- coding: utf-8 -*-
"""ComfyHub 捕获扩展的纯 Python 测试（不需要 pytest / torch / comfy）。

覆盖三块纯逻辑：

1. ``build_payload``：把一次 ComfyUI 运行的 history 片段解析成 ``/api/ingest/comfyui``
   的 JSON 请求体（含 prompt / workflow / outputs / extra）；
2. ``detect_kind`` / ``collect_outputs``：按扩展名 + 产出节点类名判断 IMAGE / VIDEO / AUDIO，
   以及 temp 过滤与去重；
3. ``post_with_retry`` / ``CaptureWorker``：指数退避重试、有界队列、异步非阻塞推送。

用法（任意 Python 3.10+）：:

    python test\\comfyui_capture_test.py

另外还包含一个真实 HTTP 的端到端用例（本机 127.0.0.1 上随机端口起一个 http.server，
先返回 500、再返回 200，验证重试 + 最终落库的 JSON）。
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import threading
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(HERE)
NODE_DIR = os.path.join(REPO_ROOT, "comfyui", "comfyhub_capture")
if NODE_DIR not in sys.path:
    sys.path.insert(0, NODE_DIR)

import capture_core as cc  # noqa: E402

# ---------------------------------------------------------------------------
#  极简测试框架（不依赖 pytest）
# ---------------------------------------------------------------------------

_RESULTS = []


def check(name, fn):
    try:
        fn()
    except Exception as exc:
        _RESULTS.append((name, False, "%s: %s" % (type(exc).__name__, exc)))
        traceback.print_exc()
    else:
        _RESULTS.append((name, True, ""))


def expect_eq(actual, expected, what=""):
    if actual != expected:
        raise AssertionError("%s 期望 %r，实际 %r" % (what or "值", expected, actual))


def expect_true(value, what=""):
    if not value:
        raise AssertionError("%s 期望为真，实际 %r" % (what or "值", value))


# ---------------------------------------------------------------------------
#  测试数据：模拟一次真实的 ComfyUI 运行
# ---------------------------------------------------------------------------

PROMPT = {
    "3": {
        "class_type": "KSampler",
        "inputs": {"seed": 884213771, "steps": 32, "cfg": 7.5, "model": ["4", 0]},
    },
    "4": {
        "class_type": "CheckpointLoaderSimple",
        "inputs": {"ckpt_name": "sd_xl_base_1.0.safetensors"},
    },
    "9": {
        "class_type": "SaveImage",
        "inputs": {"filename_prefix": "ComfyUI", "images": ["8", 0]},
    },
    "12": {
        "class_type": "VHS_VideoCombine",
        "inputs": {"frame_rate": 16, "format": "video/h264-mp4"},
    },
    "13": {
        "class_type": "SaveAudioMP3",
        "inputs": {"filename_prefix": "audio/ComfyUI"},
    },
    "14": {
        "class_type": "SaveAnimatedWEBP",
        "inputs": {"filename_prefix": "ComfyUI", "fps": 8.0},
    },
    "15": {
        "class_type": "PreviewImage",
        "inputs": {"images": ["8", 0]},
    },
}

WORKFLOW = {
    "last_node_id": 15,
    "last_link_id": 20,
    "nodes": [
        {"id": 9, "type": "SaveImage", "widgets_values": ["ComfyUI"]},
    ],
    "links": [],
    "version": 0.4,
}

# 直接照抄 ComfyUI /history 的 outputs 结构
HISTORY_OUTPUTS = {
    "9": {
        "images": [
            {"filename": "ComfyUI_00001_.png", "subfolder": "", "type": "output"},
            {"filename": "ComfyUI_00002_.png", "subfolder": "batch", "type": "output"},
            # 与第一条完全重复，应被去重
            {"filename": "ComfyUI_00001_.png", "subfolder": "", "type": "output"},
        ]
    },
    "12": {
        "gifs": [
            {"filename": "ComfyUI_00001.mp4", "subfolder": "video", "type": "output"},
        ]
    },
    "13": {
        "audio": [
            {"filename": "ComfyUI_00001_.mp3", "subfolder": "audio", "type": "output"},
        ]
    },
    "14": {
        "images": [
            {"filename": "ComfyUI_00001_.webp", "subfolder": "", "type": "output"},
        ]
    },
    "15": {
        "images": [
            {"filename": "ComfyUI_temp_abcde_00001_.png", "subfolder": "", "type": "temp"},
        ]
    },
}


# ---------------------------------------------------------------------------
#  1. 载荷组装
# ---------------------------------------------------------------------------

def test_build_payload_from_history():
    payload = cc.build_payload(
        "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0",
        PROMPT,
        HISTORY_OUTPUTS,
        WORKFLOW,
        status="success",
        output_dir=r"D:\Comfy-Desktop\ComfyUI-Shared\output",
        comfy_url="http://127.0.0.1:8188",
        client_id="abc-client-id",
        tags=["ComfyUI"],
    )
    expect_eq(payload["runKey"], "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0", "runKey 必须是 prompt_id")
    expect_eq(payload["comfyUrl"], "http://127.0.0.1:8188", "comfyUrl")
    expect_eq(payload["outputDir"], r"D:\Comfy-Desktop\ComfyUI-Shared\output", "outputDir")
    expect_eq(payload["status"], "success", "status")
    expect_eq(payload["error"], None, "error")
    expect_true(payload["prompt"] is PROMPT, "prompt 应原样带上 API 格式节点图")
    expect_true(payload["workflow"] is WORKFLOW, "workflow 应原样带上界面格式工作流")
    expect_eq(payload["extra"], {"clientId": "abc-client-id", "tags": ["ComfyUI"]}, "extra")

    names = sorted(o["filename"] for o in payload["outputs"])
    expect_eq(
        names,
        [
            "ComfyUI_00001.mp4",
            "ComfyUI_00001_.mp3",
            "ComfyUI_00001_.png",
            "ComfyUI_00001_.webp",
            "ComfyUI_00002_.png",
        ],
        "outputs 文件名（temp 预览应被跳过、重复项应去重）",
    )

    by_name = {o["filename"]: o for o in payload["outputs"]}
    expect_eq(by_name["ComfyUI_00001_.png"]["kind"], "IMAGE", "png -> IMAGE")
    expect_eq(by_name["ComfyUI_00001_.png"]["nodeId"], "9", "nodeId")
    expect_eq(by_name["ComfyUI_00001_.png"]["nodeType"], "SaveImage", "nodeType")
    expect_eq(by_name["ComfyUI_00002_.png"]["subfolder"], "batch", "subfolder 要保留")
    expect_eq(by_name["ComfyUI_00001.mp4"]["kind"], "VIDEO", "mp4 -> VIDEO")
    expect_eq(by_name["ComfyUI_00001_.mp3"]["kind"], "AUDIO", "mp3 -> AUDIO")

    # 必须能被标准 json 序列化，且不含任何非字符串键
    text = json.dumps(payload, ensure_ascii=False)
    expect_true("ComfyUI_00001_.png" in text, "JSON 里应包含产出文件名")
    parsed = json.loads(text)
    expect_eq(sorted(parsed.keys()),
              ["comfyUrl", "error", "extra", "outputDir", "outputs", "prompt", "runKey", "status", "workflow"],
              "顶层字段集合")


def test_build_payload_without_workflow_and_with_error():
    payload = cc.build_payload(
        "run-2",
        PROMPT,
        {"9": {"images": [{"filename": "a.png", "subfolder": "", "type": "output"}]}},
        None,
        status="error",
        error="CUDA out of memory",
        tags=[],
    )
    expect_eq(payload["workflow"], None, "拿不到 UI 工作流时应为 null")
    expect_eq(payload["status"], "error", "status")
    expect_eq(payload["error"], "CUDA out of memory", "error")
    expect_eq(payload["outputDir"], None, "未提供 outputDir 时为 null")
    expect_eq(payload["comfyUrl"], cc.DEFAULT_COMFYUI_URL, "comfyUrl 默认值")
    expect_eq(payload["extra"]["tags"], [], "tags")

    # dict 形式的 error 也要能安全成串
    payload2 = cc.build_payload("run-3", {}, [], None, status="error", error={"type": "x"})
    expect_true("x" in payload2["error"], "dict 形式 error 应序列化进字符串")


def test_build_payload_accepts_prebuilt_output_list():
    prebuilt = [
        {"filename": "x.png", "subfolder": "", "type": "output", "kind": "IMAGE", "nodeId": "9", "nodeType": "SaveImage"}
    ]
    payload = cc.build_payload("run-4", PROMPT, prebuilt)
    expect_eq(len(payload["outputs"]), 1, "已解析好的列表应直接透传")
    expect_eq(payload["outputs"][0]["kind"], "IMAGE", "kind")


def test_include_temp_switch():
    payload = cc.build_payload("run-5", PROMPT, HISTORY_OUTPUTS, include_temp=True)
    names = [o["filename"] for o in payload["outputs"]]
    expect_true("ComfyUI_temp_abcde_00001_.png" in names, "includeTemp=True 时应带上 temp 预览")
    temp_item = [o for o in payload["outputs"] if o["type"] == "temp"][0]
    expect_eq(temp_item["kind"], "IMAGE", "temp 预览的类型")


# ---------------------------------------------------------------------------
#  2. 类型识别
# ---------------------------------------------------------------------------

def test_extension_of():
    expect_eq(cc.extension_of("a.PNG"), "png", "大小写归一")
    expect_eq(cc.extension_of("noext"), "", "无扩展名")
    expect_eq(cc.extension_of(None), "", "None 安全")


def test_detect_kind_by_extension_and_node():
    cases = [
        # (文件名, 节点类名, 期望)
        ("a.png", "SaveImage", "IMAGE"),
        ("a.jpg", "SaveImage", "IMAGE"),
        ("a.jpeg", "SaveImage", "IMAGE"),
        ("a.bmp", "SaveImage", "IMAGE"),
        ("a.tiff", "SaveImage", "IMAGE"),
        ("a.webp", "SaveImage", "IMAGE"),            # 普通 webp 静态图
        ("a.webp", "SaveAnimatedWEBP", "VIDEO"),     # 动图 webp
        ("a.gif", "SaveImage", "IMAGE"),             # 静态 gif 保守当图片
        ("a.gif", "VHS_VideoCombine", "VIDEO"),      # 视频节点产出的 gif
        ("a.mp4", "VHS_VideoCombine", "VIDEO"),
        ("a.webm", "SaveWEBM", "VIDEO"),
        ("a.mkv", "SaveVideo", "VIDEO"),
        ("a.mov", "SaveVideo", "VIDEO"),
        ("a.avi", "SomeVideoNode", "VIDEO"),
        ("a.flac", "SaveAudio", "AUDIO"),
        ("a.mp3", "SaveAudioMP3", "AUDIO"),
        ("a.wav", "SaveAudio", "AUDIO"),
        ("a.ogg", "SaveAudio", "AUDIO"),
        ("a.m4a", "SaveAudio", "AUDIO"),
        ("a.xyz", "SaveImage", "IMAGE"),             # 未知扩展名兜底
    ]
    for filename, node_type, expected in cases:
        expect_eq(cc.detect_kind(filename, node_type), expected, "%s / %s" % (filename, node_type))


def test_collect_outputs_dedup_and_node_types():
    items = cc.collect_outputs(HISTORY_OUTPUTS, {str(k): v["class_type"] for k, v in PROMPT.items()})
    keys = [(i["filename"], i["subfolder"], i["type"]) for i in items]
    expect_eq(len(keys), len(set(keys)), "不得有重复的 (filename, subfolder, type)")
    expect_eq(len(keys), 5, "共 5 个 output 文件（1 个 temp 被跳过、1 个重复被去掉）")
    expect_true(all(i["type"] == "output" for i in items), "默认只上报 output 类型")
    # latents 之类的键也要能收
    extra = cc.collect_outputs({"20": {"latents": [{"filename": "l.safetensors", "subfolder": "", "type": "output"}]}}, {"20": "SaveLatent"})
    expect_eq(len(extra), 1, "latents 键也应被收集")


# ---------------------------------------------------------------------------
#  3. 配置
# ---------------------------------------------------------------------------

def test_config_resolution():
    # 默认值
    cfg = cc.resolve_config({}, {})
    expect_eq(cfg["url"], "http://127.0.0.1:8080", "默认后端地址")
    expect_eq(cfg["enabled"], True, "默认启用")
    expect_eq(cfg["includeTemp"], False, "默认不报 temp")
    expect_eq(cfg["queueSize"], 256, "默认队列长度")
    expect_eq(cfg["timeout"], 5.0, "默认超时 5s")

    # config.json 覆盖
    cfg = cc.resolve_config(
        {},
        {"comfyhubUrl": "http://127.0.0.1:9999/", "tags": ["ComfyUI", "本地"], "includeTemp": True, "enabled": False},
    )
    expect_eq(cfg["url"], "http://127.0.0.1:9999", "config.json 的地址要去掉尾部斜杠")
    expect_eq(cfg["tags"], ["ComfyUI", "本地"], "tags")
    expect_eq(cfg["includeTemp"], True, "includeTemp")
    expect_eq(cfg["enabled"], False, "enabled")

    # 环境变量优先级最高
    cfg = cc.resolve_config({"COMFYHUB_URL": "127.0.0.1:7777"}, {"comfyhubUrl": "http://127.0.0.1:9999"})
    expect_eq(cfg["url"], "http://127.0.0.1:7777", "环境变量应覆盖 config.json，并自动补 http://")

    # 坏配置不能抛
    with tempfile.TemporaryDirectory() as tmp:
        broken = os.path.join(tmp, "config.json")
        with open(broken, "w", encoding="utf-8") as fh:
            fh.write("{ this is not json")
        expect_eq(cc.load_config(broken), {}, "坏 json 应返回空配置")
        expect_eq(cc.load_config(os.path.join(tmp, "nope.json")), {}, "不存在的文件应返回空配置")


def test_disable_switches():
    expect_eq(cc.is_capture_disabled({}), False, "默认启用")
    expect_eq(cc.is_capture_disabled({"COMFYHUB_CAPTURE": "0"}), True, "COMFYHUB_CAPTURE=0 关闭")
    expect_eq(cc.is_capture_disabled({"COMFYHUB_CAPTURE": "1"}), False, "COMFYHUB_CAPTURE=1 启用")
    expect_eq(cc.is_capture_disabled({"COMFYHUB_CAPTURE_DISABLED": "1"}), True, "COMFYHUB_CAPTURE_DISABLED=1 关闭")


# ---------------------------------------------------------------------------
#  4. 重试 / 退避
# ---------------------------------------------------------------------------

class FakePoster:
    """按脚本返回响应；记录每次调用。"""

    def __init__(self, script):
        self.script = list(script)
        self.calls = []

    def __call__(self, url, payload, timeout=5.0):
        self.calls.append((url, payload, timeout))
        item = self.script.pop(0) if self.script else (200, "{}")
        if isinstance(item, Exception):
            raise item
        return item


def test_post_with_retry_succeeds_after_500():
    poster = FakePoster([(500, "boom"), (500, "boom"), (200, '{"ok":true}')])
    sleeps = []
    ok, status, text = cc.post_with_retry(
        "http://127.0.0.1:8099/api/ingest/comfyui", {"runKey": "x"},
        poster=poster, max_attempts=3, backoff=0.5, sleep=sleeps.append,
    )
    expect_eq(ok, True, "重试后应成功")
    expect_eq(status, 200, "最终状态码")
    expect_eq(len(poster.calls), 3, "应刚好尝试 3 次")
    expect_eq(sleeps, [0.5, 1.0], "退避应为 0.5s / 1.0s（指数）")


def test_post_with_retry_gives_up_on_4xx():
    poster = FakePoster([(400, "bad request")])
    sleeps = []
    ok, status, _ = cc.post_with_retry(
        "http://127.0.0.1:8099/api/ingest/comfyui", {}, poster=poster,
        max_attempts=3, backoff=0.5, sleep=sleeps.append,
    )
    expect_eq(ok, False, "4xx 不应重试")
    expect_eq(status, 400, "状态码")
    expect_eq(len(poster.calls), 1, "4xx 只尝试 1 次")
    expect_eq(sleeps, [], "4xx 不应有等待")


def test_post_with_retry_swallows_network_errors():
    poster = FakePoster([ConnectionRefusedError("拒绝连接"), ConnectionRefusedError("拒绝连接"), ConnectionRefusedError("拒绝连接")])
    ok, status, text = cc.post_with_retry(
        "http://127.0.0.1:8099/api/ingest/comfyui", {}, poster=poster,
        max_attempts=3, backoff=0.0, sleep=lambda _s: None,
    )
    expect_eq(ok, False, "全失败时应返回 False 而不是抛异常")
    expect_eq(status, None, "没有状态码")
    expect_true("ConnectionRefusedError" in text, "应带上异常名，便于排查")
    expect_eq(len(poster.calls), 3, "网络错误应重试满 3 次")


# ---------------------------------------------------------------------------
#  5. 后台线程 / 有界队列（异步非阻塞）
# ---------------------------------------------------------------------------

def test_worker_is_async_and_bounded():
    poster = FakePoster([(200, "{}")] * 5)
    worker = cc.CaptureWorker("http://127.0.0.1:8099", queue_size=2, delay=0.0, poster=poster, sleep=time.sleep)
    gate = threading.Event()

    def slow_poster(url, payload, timeout=5.0):
        gate.wait(timeout=5)
        return (200, "{}")

    worker.poster = slow_poster
    t0 = time.perf_counter()
    # 队列只有 2 格，塞 5 条：第 1 条被线程取走后，第 2、3 条入队，剩下 2 条被丢弃
    accepted = [worker.submit({"runKey": "r%d" % i}) for i in range(5)]
    elapsed = time.perf_counter() - t0
    expect_true(elapsed < 1.0, "submit 必须立刻返回（不阻塞），实际 %.3fs" % elapsed)
    expect_true(accepted[0] is True, "第一条应入队")
    expect_true(accepted[-1] is False, "队列满时应丢弃并返回 False")
    expect_true(worker.dropped >= 1, "应有丢弃计数，实际 %s" % worker.dropped)

    gate.set()
    expect_true(worker.drain(5.0), "队列应能排空")
    worker.stop()
    expect_true(worker.sent >= 1, "至少有一条被真正推送，实际 %s" % worker.sent)


def test_worker_never_raises_on_poster_exception():
    def boom(url, payload, timeout=5.0):
        raise RuntimeError("后端炸了")

    worker = cc.CaptureWorker("http://127.0.0.1:8099", queue_size=4, delay=0.0, max_attempts=2,
                              backoff=0.0, poster=boom, sleep=lambda _s: None)
    expect_true(worker.submit({"runKey": "boom"}), "入队应成功")
    expect_true(worker.drain(5.0), "队列应能排空")
    worker.stop()
    expect_eq(worker.failed, 1, "失败应被计数而不是抛异常")

    # 后端地址非法（比如 URL 写错）也不能抛
    worker2 = cc.CaptureWorker("::not a url::", queue_size=4, delay=0.0, max_attempts=1, backoff=0.0, sleep=lambda _s: None)
    expect_true(worker2.submit({"runKey": "bad-url"}), "入队应成功")
    worker2.drain(5.0)
    worker2.stop()


# ---------------------------------------------------------------------------
#  6. 真实 HTTP 端到端：500 -> 200 重试 + 记录收到的 JSON
# ---------------------------------------------------------------------------

def test_end_to_end_http_with_retry():
    import http.server

    received = []
    first_call = {"n": 0}

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_POST(self):  # noqa: N802
            length = int(self.headers.get("Content-Length") or 0)
            raw = self.rfile.read(length)
            body = json.loads(raw.decode("utf-8"))
            first_call["n"] += 1
            if first_call["n"] == 1:
                self.send_response(500)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(b'{"error":"first attempt fails"}')
                return
            received.append(body)
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(
                json.dumps(
                    {
                        "runKey": body.get("runKey"),
                        "promptId": 42,
                        "created": True,
                        "mediaIds": [7, 8],
                        "imported": len(body.get("outputs") or []),
                        "duplicates": 0,
                        "failed": 0,
                        "message": None,
                    }
                ).encode("utf-8")
            )

        def log_message(self, *args):  # 静音
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        url = "http://127.0.0.1:%d%s" % (port, cc.INGEST_PATH)
        payload = cc.build_payload(
            "e2e-run-key",
            PROMPT,
            HISTORY_OUTPUTS,
            WORKFLOW,
            status="success",
            output_dir=r"D:\Comfy-Desktop\ComfyUI-Shared\output",
            comfy_url="http://127.0.0.1:8188",
            client_id="e2e-client",
            tags=["ComfyUI"],
        )
        expect_eq(len(payload["outputs"]), 5, "端到端用例的产出数量")

        record_path = os.path.join(tempfile.gettempdir(), "comfyhub-e2e-recorded.json")
        ok, status, text = cc.post_with_retry(
            url, payload, timeout=5.0, max_attempts=3, backoff=0.05, sleep=time.sleep
        )
        expect_eq(ok, True, "500 之后重试应最终成功")
        expect_eq(status, 200, "最终状态码")
        expect_eq(first_call["n"], 2, "服务端应被打到 2 次（1 次 500 + 1 次 200）")
        expect_eq(len(received), 1, "服务端应记录到 1 份被接受的 JSON")

        recorded = received[0]
        with open(record_path, "w", encoding="utf-8") as fh:
            json.dump(recorded, fh, ensure_ascii=False, indent=2)
        print("\n[端到端] 服务端最终收到的 JSON 已写入: %s" % record_path)
        print(json.dumps(recorded, ensure_ascii=False, indent=2)[:1200])

        expect_eq(recorded["runKey"], "e2e-run-key", "runKey")
        expect_eq(recorded["outputDir"], r"D:\Comfy-Desktop\ComfyUI-Shared\output", "outputDir")
        expect_eq(recorded["workflow"]["last_node_id"], 15, "workflow 应原样带过去")
        expect_eq(recorded["prompt"]["3"]["class_type"], "KSampler", "prompt 图")
        expect_eq(len(recorded["outputs"]), 5, "outputs")
        for item in recorded["outputs"]:
            for field in ("filename", "subfolder", "type", "kind", "nodeId", "nodeType"):
                expect_true(field in item, "产出项缺少字段 %s" % field)
        resp = json.loads(text)
        expect_eq(resp["imported"], 5, "响应里的 imported 数量")
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=3)


# ---------------------------------------------------------------------------
#  运行
# ---------------------------------------------------------------------------

def main():
    print("== ComfyHub 捕获扩展 · 纯逻辑测试 ==")
    print("节点目录: %s" % NODE_DIR)
    print("capture_core: %s" % getattr(cc, "__file__", "?"))
    print("")

    check("载荷组装：从 history 解析出 ingest JSON", test_build_payload_from_history)
    check("载荷组装：无 workflow / 错误状态", test_build_payload_without_workflow_and_with_error)
    check("载荷组装：已解析好的产出列表直接透传", test_build_payload_accepts_prebuilt_output_list)
    check("载荷组装：includeTemp 开关", test_include_temp_switch)
    check("类型识别：扩展名解析", test_extension_of)
    check("类型识别：扩展名 + 节点类名 -> IMAGE/VIDEO/AUDIO", test_detect_kind_by_extension_and_node)
    check("类型识别：outputs 去重与 temp 过滤", test_collect_outputs_dedup_and_node_types)
    check("配置：优先级 环境变量 > config.json > 默认", test_config_resolution)
    check("配置：关闭开关 COMFYHUB_CAPTURE / _DISABLED", test_disable_switches)
    check("重试：500 -> 500 -> 200 指数退避", test_post_with_retry_succeeds_after_500)
    check("重试：4xx 不重试", test_post_with_retry_gives_up_on_4xx)
    check("重试：网络异常被吞掉", test_post_with_retry_swallows_network_errors)
    check("推送线程：异步 + 有界队列 + 丢包告警", test_worker_is_async_and_bounded)
    check("推送线程：推送函数抛异常也不崩", test_worker_never_raises_on_poster_exception)
    check("端到端：真实 HTTP，500 后重试成功", test_end_to_end_http_with_retry)

    print("")
    failed = 0
    for name, ok, detail in _RESULTS:
        if ok:
            print("  [通过] %s" % name)
        else:
            failed += 1
            print("  [失败] %s -> %s" % (name, detail))
    print("")
    print("共 %d 项，通过 %d 项，失败 %d 项" % (len(_RESULTS), len(_RESULTS) - failed, failed))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

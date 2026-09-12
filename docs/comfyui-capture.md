# ComfyUI 捕获：三条通道怎么选

ComfyHub 需要知道「ComfyUI 跑了什么、产出了哪些文件」。这件事有三条互不冲突的通道，
按 `runKey = ComfyUI 的 prompt_id` 幂等 —— 谁先到谁入库，后来的会被判成重复，不会产生脏数据。

| | **① 后端轮询 `/history`** | **② 自定义节点推送** | **③ 导入已生成的 PNG** |
| --- | --- | --- | --- |
| 位置 | `server/src/.../CaptureRepo.kt` + 轮询器 | `comfyui/comfyhub_capture/` | ComfyHub「画廊 → 上传」 |
| 触发 | 定时（默认 4 秒一轮） | ComfyUI 一跑完立刻推 | 手动 |
| 延迟 | 秒级（看轮询间隔） | 亚秒级（0.5s 落盘 + 一次 POST） | 人工 |
| 覆盖范围 | **所有**连着的 ComfyUI 实例 | 只覆盖装了节点的那个实例 | 只覆盖你手动选中的文件 |
| 需要改 ComfyUI | 不需要 | 需要在 `custom_nodes` 里放一个目录 | 不需要 |
| 拿得到 UI 工作流 | ✅ `extra_data.extra_pnginfo.workflow` | ✅ 同上 | ✅ PNG 的 tEXt chunk 里就带着 |
| 拿得到参数（seed/steps/模型） | ✅ | ✅ | ✅ 从 PNG 元数据解析 |
| 拿得到视频/音频 | ✅ | ✅ | ⚠️ 视频/音频文件本身没有元数据，拿不全 |
| ComfyUI 挂了会不会丢 | 不会（重启后补轮询） | 会（推送失败即丢，轮询会补） | 不会（文件还在硬盘上） |
| 失败影响 ComfyUI | 无 | 无（全异步 + 吞异常） | 无 |

## 怎么选

**默认用 ① 后端轮询。** 它不需要动 ComfyUI，覆盖所有实例，ComfyUI 没开的时候也不会报错。
绝大多数场景这一条就够了。

**想让画廊「立刻」出现结果，再加 ② 自定义节点。**
轮询间隔内你是看不到刚跑完的那条记录的；装了节点就是跑完即到。
两条通道同时开着**没有问题**：幂等键相同，先到的赢，另一条记一次重复。

**③ 是给「历史存量」用的。** 装 ComfyHub 之前产出的那一堆 PNG，用后端轮询也能捞回来
（`/history` 里有），但如果 ComfyUI 已经重装、历史被清空，或者文件是从别的机器拷过来的，
那就只能走上传了 —— PNG 自带的 `prompt` / `workflow` 注释足够还原出完整参数。

## ① 后端轮询（默认，不用配）

```powershell
pwsh -File scripts\comfyhub.ps1 up      # 后端起来就开始轮询
curl.exe http://127.0.0.1:8080/api/health
```

后端按 `prompt_id` 记录「已捕获」，重启后接着捞，不会重复。
想改轮询间隔/地址：见后端捕获配置（`GET/PUT /api/capture/config`，字段 `pollSeconds` / `comfyUrl` / `outputDir` / `autoTag`）。

## ② 自定义节点推送（可选，低延迟）

```powershell
pwsh -File scripts\install-comfy-node.ps1 -ListOnly    # 先看探测结果，不写任何文件
pwsh -File scripts\install-comfy-node.ps1              # 装（默认 Junction）
# 重启 ComfyUI，日志里出现一行：[ComfyHub] 捕获扩展已启用 -> http://127.0.0.1:8080
```

详细配置、类型识别规则、排错见 [`comfyui/comfyhub_capture/README.md`](../comfyui/comfyhub_capture/README.md)。

## ③ 导入已经生成好的 PNG

在 ComfyHub 里走「画廊 → 上传」：选中文件（可多选），ComfyHub 会
按 SHA-256 去重、按扩展名识别类型，PNG 的 `prompt` / `workflow` 注释会被解析出来
自动建成 / 关联提示词。用命令行也行：

```powershell
curl.exe -X POST http://127.0.0.1:8080/api/media/upload -F "files=@samples\neon-street.png"
```

---

# 实现说明：为什么钩在这几个点上

以下结论来自**实测**本机安装的 **ComfyUI Desktop 0.34.2**
（源码 `D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI`，`comfyui_version.py` 里 `__version__ = "0.34.2"`），
不是照文档猜的。

## 实际用到的钩子

| 用途 | 钩子 | 源码位置 |
| --- | --- | --- |
| 拿 UI 工作流（兜底） | `PromptServer.instance.add_on_prompt_handler(handler)` | `server.py:1456`（定义）、`server.py:1459` `trigger_on_prompt`、`server.py:1076` 在 `POST /prompt` 里调用 |
| 拿 `prompt_id` + API 参数图 + `extra_data`，并在结束时收尾 | 包装 `PromptExecutor.execute_async` | `execution.py:730`（`async def execute_async(self, prompt, prompt_id, extra_data={}, execute_outputs=[])`） |
| 观察 `executed` / `execution_success` / `execution_error` / `executing(node=None)` | 包装 `PromptServer.send_sync(self, event, data, sid=None)` | `server.py:1392` |

`executed` 消息由 `execution.py:578`（正常执行）和 `execution.py:436`（缓存节点回放）发出，
结构是 `{"node": "<node_id>", "display_node": ..., "output": {"images|gifs|audio|...": [ {filename, subfolder, type}, ... ]}, "prompt_id": ...}`。

## 为什么不用「只用 add_on_prompt_handler」

因为 **0.34.2 里这个回调拿不到 `prompt_id`**：`server.py:1076` 在 `POST /prompt` 处理函数里
调用 `trigger_on_prompt(json_data)`，而 `prompt_id` 是在那之后（`server.py:1088-1104`）
才生成 / 校验出来的。回调里只有 `prompt` 图和 `extra_data`，没有 id，没法建幂等键。

所以选了两层：

1. `execute_async` 是主执行路径上**唯一**同时拿得到 `prompt_id`、API 参数图、`extra_data`
   的地方，而且它自带 `finally`（`execution.py:838`），异常中断也能正确收尾；
2. 顺带把 `on_prompt` 钩子也挂上：它按 prompt 图的 SHA-1 指纹存下 UI 工作流，
   万一 `extra_data.extra_pnginfo` 缺失，`execute_async` 里能按指纹把工作流补回来
   （默认路径其实直接就能从 `extra_data.extra_pnginfo.workflow` 拿到，
   ComfyUI 前端的 `queuePrompt` 会带 `extra_pnginfo: {workflow: ...}`，这个兜底属于双保险）。

## 为什么不用「只用 send_sync」

因为 `PromptServer.send_sync` 在有客户端时走的是 `server.client_id` 这条线路
（`execution.py:684`：`if self.server.client_id is not None or broadcast`），
`executed` 消息带的是某个具体 `sid`。只靠它观察，会受"当前有没有前端连着"的影响；
而且它是**推送**语义，一次运行的开始/结束边界不如 `execute_async` 清楚。
现在 `send_sync` 的包装只承担"收集产出 + 记录成败"，真正的边界判定在 `execute_async`。

## 为什么不用「轮询 /history」

轮询能work，但在这台机器上有两个问题：

1. `/history` 里的 `outputs` 在 `PromptQueue.task_done`（`execution.py:1286`）里才落盘，
   而 `task_done` 之后马上就是下一次执行 —— 轮询窗口要么太小（白轮询），要么太大（延迟高）；
2. 轮询需要另一个线程 + 状态比对，反而比挂钩子复杂，且拿不到"这次运行是不是刚失败"的即时性。

轮询已经由后端的捕获器承担，节点这边不重复实现。

## 兼容性风险（升级 ComfyUI 时注意）

| 风险 | 说明 | 缓解 |
| --- | --- | --- |
| `execute_async` 签名变化 | 包装函数按 `(self, prompt, prompt_id, extra_data, execute_outputs)` 透传 | 升级后跑一次 `test\comfyui_capture_test.py`；启动日志里搜 `comfyhub_capture` |
| `send_sync` 签名变化 | 包装函数按 `(self, event, data, sid)` + `*args/**kwargs` 透传，返回值原样返回 | 同上 |
| 任何钩子失败 | 导入期 `_boot()` 整体包了 try/except；挂不上钩子最多是"不推送"，ComfyUI 照常跑 | 启动日志里会有一条 `捕获扩展初始化失败（ComfyUI 不受影响）` |

# 验证记录（本机实测）

```powershell
# 1) 语法编译
& 'D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI\.venv\Scripts\python.exe' `
    -m py_compile comfyui\comfyhub_capture\__init__.py comfyui\comfyhub_capture\capture_core.py
# -> exit 0

# 2) 纯逻辑测试（不需要 pytest / torch / comfy）
& 'D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI\.venv\Scripts\python.exe' test\comfyui_capture_test.py
# -> 共 15 项，通过 15 项，失败 0 项
#    含真实 HTTP 端到端：本地 http.server 第一次返回 500、第二次 200，验证指数退避重试

# 3) 安装脚本
pwsh -NoProfile -File scripts\install-comfy-node.ps1 -ListOnly
# -> 探测到 D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI，不写任何文件

# 4) 真正装到本机在用的 ComfyUI（这一步会写入你的 custom_nodes，装完重启 ComfyUI）
pwsh -NoProfile -File scripts\install-comfy-node.ps1 -ComfyUIPath 'D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI'
# 自动探测到"正在用的那个 ComfyUI"时脚本会多要一次确认：加 -Force（或像上面那样显式给 -ComfyUIPath）
```

> 想撤掉：`pwsh -NoProfile -File scripts\install-comfy-node.ps1 -Uninstall`
> （只删 `custom_nodes\comfyhub_capture` 这个链接/副本，不碰仓库源码。）
>
> 顺带说明：**不装这个节点也能自动捕获** —— 后端轮询 `/history` 那条路不需要动 ComfyUI。
> 装它只是为了「跑完立刻入库」和「ComfyUI 崩了也留下记录」。

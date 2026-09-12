# ComfyHub 捕获扩展（ComfyUI 自定义节点）

把 ComfyUI 每一次跑完的运行（**提示词参数 + 界面工作流 + 产出的图片/视频/音频**）
实时推送给本机的 ComfyHub 后端，之后在 ComfyHub 的「画廊」里就能看到，并和提示词自动关联。

> 这是**可选**的低延迟推送通道。ComfyHub 后端自己也有一个轮询 `/history` 的捕获器，
> 两者按 `runKey = ComfyUI prompt_id` 幂等，重复推送不会产生重复数据。
> 三条捕获通道的区别与取舍见 [`docs/comfyui-capture.md`](../../docs/comfyui-capture.md)。

**核心承诺：装了这个扩展，ComfyUI 的行为和没装时一模一样。**
后端没开、端口不通、后端 500、甚至扩展自己的代码抛异常，都只会写一行日志，
不阻塞、不重试到卡顿、不改任何返回值。

---

## 1. 安装

```powershell
# 自动探测 ComfyUI（推荐先空跑看一眼探测结果）
pwsh -File scripts\install-comfy-node.ps1 -ListOnly

# 安装（默认 Junction 目录联接，改仓库代码立即生效）
pwsh -File scripts\install-comfy-node.ps1

# 跨盘 / 想固定一份快照
pwsh -File scripts\install-comfy-node.ps1 -Mode Copy

# 指定 ComfyUI 路径
pwsh -File scripts\install-comfy-node.ps1 -ComfyUIPath D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI

# 卸载（只删链接/副本，不动仓库源码）
pwsh -File scripts\install-comfy-node.ps1 -Uninstall
```

安装后**必须重启 ComfyUI**，启动日志里应出现恰好一行：

```
[ComfyHub] 捕获扩展已启用 -> http://127.0.0.1:8080
```

之后跑任意工作流，ComfyHub 的「画廊」里就会出现对应的产物。

---

## 2. 配置

优先级：**环境变量 `COMFYHUB_URL` > 节点目录下的 `config.json` > 默认 `http://127.0.0.1:8080`**。
不建 `config.json` 也能直接用默认值。

```powershell
# 方式一：环境变量（最高优先级，改完要重启 ComfyUI）
$env:COMFYHUB_URL = 'http://127.0.0.1:8080'

# 方式二：config.json
Copy-Item comfyui\comfyhub_capture\config.example.json comfyui\comfyhub_capture\config.json
```

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `comfyhubUrl` | `http://127.0.0.1:8080` | ComfyHub 后端地址 |
| `comfyUrl` | `http://127.0.0.1:8188` | 上报给后端的 ComfyUI 地址（后端读不到本地文件时会回退到用它下载） |
| `tags` | `["ComfyUI"]` | 给捕获到的提示词自动打的标签 |
| `enabled` | `true` | 设成 `false` 等于关掉扩展 |
| `includeTemp` | `false` | 是否连 `temp` 目录的预览文件一起上报（默认只上报 `output`） |
| `timeoutSeconds` | `5` | 单次 POST 超时 |
| `delaySeconds` | `0.5` | 入队后等多久再发（让 ComfyUI 把文件写完） |
| `queueSize` | `256` | 待发队列长度，满了丢弃并告警一行 |

> **Junction 模式下 `config.json` 落在仓库里**（因为节点目录本身就是仓库的
> `comfyui\comfyhub_capture`）。想每个实例单独配置就用 `-Mode Copy`。
> `config.json` 已在 `.gitignore` 里，不会被提交。

### 开关

| 环境变量 | 作用 |
| --- | --- |
| `COMFYHUB_CAPTURE=0` | 关闭扩展（等价于卸载，但不用动文件） |
| `COMFYHUB_CAPTURE_DISABLED=1` | 同上 |
| `COMFYHUB_URL` | 覆盖后端地址 |
| `COMFYHUB_COMFYUI_URL` | 覆盖上报的 ComfyUI 地址 |
| `COMFYHUB_CAPTURE_INCLUDE_TEMP=1` | 连 temp 预览一起上报（等价于 `includeTemp: true`） |
| `COMFYHUB_COMFYUI` | 只给安装脚本用：指定 ComfyUI 根目录 |

---

## 3. 它到底做了什么

1. ComfyUI 收到 `/prompt` 请求时，先记下界面工作流（`extra_data.extra_pnginfo.workflow`）；
2. 工作流开始执行时，用 `prompt_id` 建一个运行上下文（API 参数图 + 工作流 + 客户端 id）；
3. 执行过程中收集每次 `executed` 消息里的产出文件（`images` / `gifs` / `audio` / `video` /
   `videos` / `files` / `latents`），按 `(filename, subfolder, type)` 去重，跳过 `temp`；
4. 执行结束时，把这些组装成下面这个 JSON，`POST` 给 ComfyHub（**0.5 秒后**，让文件落盘）：

```json
{
  "runKey": "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0",
  "comfyUrl": "http://127.0.0.1:8188",
  "outputDir": "D:\\Comfy-Desktop\\ComfyUI-Shared\\output",
  "status": "success",
  "error": null,
  "prompt":   { "3": { "class_type": "KSampler", "inputs": { "...": "..." } } },
  "workflow": { "last_node_id": 15, "nodes": [ "..." ], "links": [] },
  "outputs": [
    { "filename": "ComfyUI_00001_.png", "subfolder": "", "type": "output",
      "kind": "IMAGE", "nodeId": "9", "nodeType": "SaveImage" }
  ],
  "extra": { "clientId": "…", "tags": ["ComfyUI"] }
}
```

`outputDir` 是运行时从 `folder_paths.get_directory_by_type("output")` 取的，
所以 ComfyUI Desktop 改了输出目录（比如指到 `ComfyUI-Shared\output`）也能对上；
后端优先直接读本地文件，读不到再回退到 `{comfyUrl}/view?filename=..&subfolder=..&type=output`。

**没有产出文件、也没有报错的运行不会推送**（纯参数试验不占库）。

---

## 4. 产物类型是怎么判断的

| 扩展名 | 判断结果 |
| --- | --- |
| `flac` `mp3` `wav` `ogg` `m4a` `opus` `aac` | AUDIO |
| `mp4` `webm` `mkv` `mov` `avi` `m4v` | VIDEO |
| `gif` / `webp` | 产出节点是视频/动图类 → VIDEO，否则 IMAGE |
| `png` `jpg` `jpeg` `bmp` `tif` `tiff` `avif` | IMAGE（但 `SaveAnimated*` 节点产出的 webp 记 VIDEO） |
| 其他未知 | 视频类节点 → VIDEO，否则 IMAGE |

判断「视频/动图类节点」看的是节点类名里有没有这些特征：
`videocombine` `savevideo` `savewebm` `vhs_` `saveanimatedwebp` `saveanimatedpng` `animated` `sequence`。

所以 `SaveImage` 存的普通 `.webp` 是 IMAGE，`SaveAnimatedWEBP` 存的是 VIDEO，
`VHS_VideoCombine` 的 `gifs` 输出是 VIDEO。

---

## 5. 排错

| 现象 | 原因 / 处理 |
| --- | --- |
| 启动日志没有 `[ComfyHub] 捕获扩展已启用` | 检查是否设了 `COMFYHUB_CAPTURE=0`；检查 `config.json` 里 `enabled` 是不是 `false`；检查节点目录是否被 ComfyUI 加载（启动日志里搜 `comfyhub_capture`） |
| 日志里出现 `推送失败（已放弃）` | 后端没起 / 端口不对。先 `pwsh -File scripts\comfyhub.ps1 up`，或用 `curl.exe http://127.0.0.1:8080/api/health` 确认 |
| 日志里出现 `推送队列已满` | 后端长时间无响应，队列（256）被塞满，超出的运行被丢弃。先修后端；轮询捕获器会补上这些运行 |
| 报 `RunKey 冲突` / 没有新记录 | `runKey` 用 `prompt_id` 做幂等，同一次运行被重复推送时后端只会记录一次。换一次新的运行再试 |
| 视频/音频没进画廊 | 很多视频节点（如 `VHS_VideoCombine`）默认把结果写进 `temp`，扩展默认只上报 `output`。把 `includeTemp` 打开（或让节点把 `save_output` 打开） |
| 出现 `nodeType: ""` | 产出节点不在这次 prompt 参数图里（极少见，多半是子图/缓存节点）。不影响入库，只是少一个类型提示 |
| 想彻底停掉 | `COMFYHUB_CAPTURE=0` 后重启 ComfyUI，或 `pwsh -File scripts\install-comfy-node.ps1 -Uninstall` |

想临时看到详细日志，可以把 ComfyUI 的日志级别调到 `--verbose DEBUG`，
扩展的日志都在 `comfyhub_capture` 这个 logger 下（正常只有一行）。

---

## 6. 实现要点（给维护者）

* **只用标准库**：`urllib.request` + `queue` + `threading`，不引入任何依赖。
* **异步非阻塞**：单守护线程 + 有界 `queue.Queue(256)`；执行线程只做一次 `put_nowait`，
  满了就 `warning` 一行然后丢弃。
* **重试**：HTTP 5xx / 网络异常按 `0.5s → 1s → 2s` 指数退避重试，最多 3 次；
  4xx 直接放弃（请求本身有问题，重试没意义）。
* **安全**：所有异常都吞掉并写日志；包装 `send_sync` 时原样转发 `*args/**kwargs` 并返回原值。
* **纯逻辑拆分**：`capture_core.py` 不导入任何 ComfyUI/torch 的东西，
  所以 `test\comfyui_capture_test.py` 可以在任意 Python 3.10+ 下直接跑。

真实bug：

* 当一个对话正在生成答复的时候，如果切到别的窗口再切回去，前面显示的部分答复全部消失
* 真手机无法安装手机端app
* comfy\_use\_attachment总是失败
* 模型不知道先生成一张替换目标，再将其用AI抠图下来，最后贴上目标背景（跟上一条有关联吗？）
* 更多bug详见昨晚本项目最新一条对话记录
* 权限问题：即便我将整个Comfy-Desktop加入写入白名单，处理遮罩这一步仍然报“拒绝访问（OS error 5）”，我不是已经不用后端本身处理遮罩，而是改用comfy里面的节点了吗？
* 调用mcp的node工具总是失败：

&#x09;Error executing tool nodes



&#x09;MCP 子进程 stderr 末尾：

&#x20;                            │ er\\storage\\ai\\mcp\\python\\Lib\\sit │

&#x20;                            │ e-packages\\mcp\\server\\mcpserver\\ │

&#x20;                            │ tools\\base.py:210 in run         │

&#x20;                            │                                  │

&#x20;                            │   207 │   │   │   raise ToolErro │

&#x20;                            │   208 │   │   except Exception a │

&#x20;                            │   209 │   │   │   # A crash: the │

&#x20;                            │ > 210 │   │   │   raise Unexpect │

&#x20;                            │   211                            │

&#x20;                            └──────────────────────────────────┘

&#x20;                            UnexpectedToolError: Error executing

&#x20;                            tool nodes

* 系统给不出可投放的附件 ID，ComfyUI 端无法读取聊天内联图片，因此不能安全提交工作流。
* MCP调用总是失败，系统被强行误解为comfy没在运行（权限问题吗？）
* 未完成的对话不统计token用量



* 安卓启动时会显示Windows的加载界面，但是没有用，因为安卓端不靠pwsh拉起后端
* 手机连接桌面端失败，报timed out
* 手机设置显示了一些只有Windows有效的设置项，应该隐藏掉



* 对于模型声明，需要重启客户端才能生效
* 当前删除provider时，密钥被设置成不自动删除



* flutter run -d windows到打开应用之后，调试直接断掉

建议：

* 继续完成implementation-master-plan.md（应该剩下很少内容了吧？）
* 使用手机真机测试（如果能）
* 给AI放权下载图片（如果要用到参考图）

\---

## 处理记录（2026-09-28）

|条目|结论 / 提交|
|-|-|
|流式答复切窗口丢失|根因：正文整轮跑完才落库，任何一次"从库里重读"都把内存里正在长的那份抹掉。重读后把内存正文盖回去（纯前端合并，`\_overlayInflight`）＋ 活跃流式条目不参与清理；另修了"跑到一半切会话、收尾对齐把别的会话消息盖进屏幕"。`2b77c5f`|
|真手机无法安装|已修：`adb install` streamed install 卡死 → 改 push + pm install 两步（`scripts\\build-apk.ps1`，2970ddf）；versionCode 统一 + `scripts\\phone-run.ps1`（0c0aa65）|
|comfy\_use\_attachment 总是失败|已修（d12e1a0）：投放改走 ComfyUI 自己的 `/upload/image`（HTTP），直写只当兜底|
|模型不知道"先生成替换目标→抠图→贴背景"|已修（cca87cd）：inpaint-composite skill 把"换人 = 三件套"提到最前|
|遮罩 OS error 5（白名单已含 Comfy-Desktop）|同 d12e1a0：遮罩也走 Delivery —— 直写被拒时改由 ComfyUI 自己收|
|MCP nodes 工具 UnexpectedToolError|真实根因（2026-09-28 复现抓到）：**不是 ComfyUI 没在运行** —— comfy-cli 1.20 起任何命令都要先探测云凭据，凭据锁文件在 `%LOCALAPPDATA%\comfy-cli\secrets.json.lock`，而受限会话把"映像在项目里"的进程（MCP 的便携 python 就是）关在项目目录里，写项目外一律 `PermissionError [Errno 13]`，整个 `comfy nodes` 家族当场炸。修复：后端起 MCP 子进程时把 comfy-cli 的落盘位置指进项目内（`COMFY_SECRETS_PATH` / `COMFY_CACHE_DIR` / `TMP`，`server-rs/src/mcp.rs` 的 `apply_comfy_cli_write_redirection`，默认位置写得进的正常机器不动）。实测重定向后 `comfy nodes search` 立刻恢复正常|
|MCP调用总是失败，系统被强行误解为comfy没在运行|就是上一条：旧版把一切 UnexpectedToolError 都译成"先确认 ComfyUI 在线"，把模型带偏。现在先认 `PermissionError / errno 13 / 拒绝访问` → 译成"写文件被系统拒绝 + 该怎么办"，认不出才回落到"确认 ComfyUI 在线"|
|偏好保存报 PathAccessException（shared\_preferences.json，errno 5）|同一根因的另一个面：viewer.exe 映像在项目里，受限会话里写不了 `%APPDATA%\com.example\viewer\`。修复：默认位置写得进就一切照旧；写不进才把落盘位置搬进项目内 `<根>\storage\app-support`，旧偏好文件一次性拷过来（`lib/core/portable_paths.dart`，main() 最先安装）。实测受限会话里启动：日志报"已重定向到项目内"、迁移文件 487 字节、不再报 errno 5|
|flutter run -d windows到打开应用之后，调试直接断掉|根因确认（用户贴的崩溃栈）：`_createDevFS: Creation of temporary directory failed, path = '…\AppData\Local\Temp' (errno 5)` —— App 自己的 `%TEMP%` 在受限会话里写不了。**已在 `windows\runner\main.cpp` 修**：`wWinMain` 里、引擎起来之前探一次默认 temp，建不出文件就把 TMP/TEMP 指到 `<根>\.run\tmp`（建得出就什么都不做）。实测受限会话里直接 `flutter run -d windows`：`Syncing files to device Windows... 109ms`、热重载可用、DevFS 目录落在 `.run\tmp\viewer*`|
|系统给不出可投放的附件 ID|根因：内联图片的附件 id 从来没写进用户消息正文（工具描述让模型"见附件说明"，但没人写）。现在每条带内联附件的消息末尾都有"附件 id 对照"。`c5241a9`|
|未完成的对话不统计 token|失败 / 取消路径现在把已跑完轮次的 usage 落到助手消息上（UI 汇总读消息行）。`b2ba69c`|
|安卓启动显示 Windows 加载界面|非 Windows 平台直接跳过本地服务引导进主界面。`ab0553d`|
|手机连接桌面 timed out|根因：`server.ps1` 硬编码 `--host 127.0.0.1`，后端只听回环，真机永远连不上。现在 `COMFYHUB\_ALLOW\_REMOTE` 全链路（App 开关 → pwsh → server.ps1 → Rust main），设置页「本地服务」新开关"允许手机 / 局域网设备连接"（重启后端生效；远程请求仍受配对闸门约束）。`f29f8b6`|
|手机设置显示 Windows 专属项|非 Windows 隐藏「本地服务」「后端服务」「打开项目目录」。`ab0553d`|
|模型声明要重启客户端才生效|根因：改模型能力声明不动 provider 行，聊天页"清单变没变"判不出来。回到聊天页 / 打开模型选择器时重读模型目录。`5903a8f`|
|删除 provider 时密钥残留|删 provider 时把无人引用的托管密钥一并清掉（桌面 Rust + 手机桥两条路；env 来源与共用 ref 例外）。`65529b8`|

### 这一串"权限"问题的 git 起源（2026-09-28 复核）

**没有哪一次改动"引入"了它** —— 触发条件是环境（受限会话把"映像在工作区内"的进程关在工作区内），
带进来的是两条**一直就在**的依赖。可指认的起点：

| 症状 | 起点 |
|-|-|
| 偏好 `shared_preferences.json` errno 5 | **`2f24294`（2026-09-12 初始提交）**：`lib/core/settings_store.dart` 用 shared_preferences，默认就写 `%APPDATA%\com.example\viewer\`，从第一天起没变过 |
| MCP 工具 `Errno 13`（`secrets.json.lock`） | **`21fae51` + `8f0ca60`（2026-09-21）**：MCP 工具接进 App、comfy-cli/comfy-mcp 装进包内（本机装的是 **comfy-cli 1.20.0**，它的 `where.resolve_default` 无条件探测云凭据）；**`49dff64`（2026-09-26）** 把 MCP 改成"后端一启动就 warmup"，于是每次都撞 |
| `flutter run` DevFS 断连 | 不在仓库里（DSH 沙箱行为）；仓库里**最早记录**是 **`aa66962`（2026-09-27）**"受限会话里 flutter run 起不来"，当时只做了 workaround（dev-app.ps1 设 TMP/TEMP），没往 App 里修 —— 本轮才修进 `windows\runner\main.cpp` |

旁证：`%APPDATA%\...\shared_preferences.json` 的**最后一次成功写入是 2026-09-26 11:19**，
之后再没更新过（今天迁移时文件时间戳仍是那一刻）—— 与"09-26 起 App 都在受限会话里跑"吻合。





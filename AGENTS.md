# AGENTS.md

给在这个仓库里干活的 AI / 协作者看。**这里只写动手前必须知道的全局规则**；具体机制、历史事故与操作细节在 `docs/`，按第 8 节索引读对应那一份。功能与接口看 [README.md](README.md)，现象 → 原因 → 处理看 [docs/pitfalls.md](docs/pitfalls.md)。

> 旧节号对照（2026-11-13 重构，其他文档里还留着这些引用）：§0 / §3 / §6.x → `docs/ui-conventions.md`；§1 → 第 3 节；§2 / §5 / §7 / §13 → `docs/backend-runtime.md`；§4 / §8 / §9 / §9.1 → `docs/environment.md`；§10 / §10.1 → `docs/ai-tools-wiring.md`；§12 → `docs/confinement-integrity.md`。

## 1. 核心原则

违反任何一条都可能清零代码或丢用户数据。

- **批量 / 脚本化操作之前先落基线**：`git commit` 或 `git stash`。仓库长期挂着上百个未提交改动 = 没有回退点（2026-10-17 曾一次清零 14 个源文件）。
- **代码改动只走 `edit` / `write` 工具，不要用脚本写文件**：脚本写的既不在 harness 快照里，也容易一把清零（`.NET File.*` 用的是进程 CWD，不是 PowerShell 的 `Set-Location`）。脚本只做只读查询。
- **删除 / 移动之前先确认解析后的绝对路径**就是你要动的那个，不用算出来的、没核对过的路径。
- **不许静默吞掉关键错误**：判据只有一条 —— 这次失败会不会被当成成功。会的那几条必须如实说出来（缓存写 / 清、路径探测、后台轮询、回滚指针）；只有纯 best-effort（清残片、关客户端、删临时文件）才可以静默。
- **不许硬编码某台机器的路径或盘符**（`D:\ComfyUI`、`C:\Program Files\Java\jdk-25` 只允许出现在注释 / 用例夹具里）：运行期一律"现探"。
- **不许绕过审批与路径安全策略**：写入必须过 `ToolPolicy`，审批必须先 `ToolApprovalGate.open(callId)` 再发 `tool.requested`。
- **不许丢失 AI 已经输出的内容**（正文、工具卡、图片入口、标题缓冲）：失败、取消、重试都一样。
- **不许破坏数据迁移与持久化安全**（先停后端 → 备份 → 复制 → 校验 → 最后落指针；库合并只增不减、一个事务）。
- **不许为了精简删掉真正防回归的测试**：判据是"删了以后出错还能不能被抓住"。耗时和用例数别抄进文档。

## 2. 项目架构

`SQLite（单文件）+ 桌面 Rust（单文件 exe）+ 移动 Kotlin harness + Flutter（两端）`。

- **桌面后端只改 `server-rs/`（Rust）**；旧 Kotlin 后端已整体删除，要对照旧行为只能从 git 历史取。
- **不要引入 KMP**（`kotlin.multiplatform`）—— 与 AGP 9 内建 Kotlin 这个刻意设计正面冲突。
- **移动端 harness 用 Android Kotlin**，Flutter 通过 platform channel 调用；**手机端不许硬编码系统提示 / 工具描述 / 安全文案**，由桌面 `/api/ai/run-kit` 版本化下发。
- 红线、验收判据、回归防线归属见 [docs/implementation-master-plan.md](docs/implementation-master-plan.md)（**动手之前先读**）。

## 3. 开发与测试

- 只动 `lib/` 下的 Dart → `pwsh -File scripts\dev-app.ps1`（保证后端最新 + `flutter run -d windows --debug`），起来后按 `r` 热重载；**别为了看一个前端改动跑 release 构建**。
- 改完过 `flutter analyze` + `flutter test`（`PUB_HOSTED_URL=https://pub.dev`）；交互类改动跑对应用例，别只跑 `widget_test.dart`。
- **"我改了但界面没变"先查构建新鲜度**（Release 看 `data\app.so`，debug 看 `data\flutter_assets\kernel_blob.bin`），界面类修复必须重启一次再确认。
- **可能影响流畅度的组件改动必须量帧耗时**（新建 / 改写 widget、改列表与重建范围、改图片解码与缓存、改动画与滚动联动、高频 `setState`）：用 **Dart MCP Server** 量，判据是长帧而不是平均帧率，不许以"看起来不卡"收工。接法与回落路径见 [docs/perf-measurement.md](docs/perf-measurement.md)。

## 4. 前端规范

- **两端一起设计**：形态由 `kWideLayoutMinWidth = 900` 一个数字划开，最小窗口 360 逻辑像素，宽度阈值只有这一份真源。
- **列表一律用 `adaptive_layout.dart` 的多列组件**；长列表必须懒构建（`ListView.builder` / `Sliver*`，流式刷新每项包 `RepaintBoundary`）；高矮差大的列表用固定行高 + 编辑弹窗。
- **文案**：句子完整、不列提纲、不加从句和括号；用用户熟悉的名词；错误提示说人话。**先确认渲染器再写格式**（只有进 `MarkdownText` 的才解析 Markdown）。注释与界面用语同样要严谨、通俗、不口头化。
- **交互**：翻页两页同时过渡、动画结束才注销旧页；大标题随滚动在顶栏淡入；菜单只用 `AppMenuButton` / `showAppContextMenu`（别改回 `PopupMenuButton`）。
- 组件选型、接入方式、文案细则与回归用例见 [docs/ui-conventions.md](docs/ui-conventions.md)。

## 5. 后端规范

- **"谁起的谁关"**：正常关窗由 App kill 自己起的后端，被强杀靠 `--watch-stdin` 的 EOF 退出；本地服务必须静默启动（不弹黑框）。
- **路径解析只有一份真源**：`lib\core\backend_paths.dart`（`scripts\backend.ps1` 是它的镜像，改一边必须改另一边）；端口只从 `.run\api.json` 读。
- **数据目录可以被用户搬走**：先认库的身份、再谈路径；指针目录不在了就回运行根目录，绝不建空库，绝不按大小 / 时间 / 层级猜。
- **内容一律存在自身文件夹内**，不许散到 `%APPDATA%` / `%LOCALAPPDATA%` / `%TEMP%`。
- **写不进工作区外的真因是完整性标签（Low IL），不是 ACL**：先修映像标签或把整套放到工作区外；别建议换终端，也别建议拷到 `%LOCALAPPDATA%`。
- 落盘布局、迁移 / 合并纪律、端口与代理、Release 装配见 [docs/backend-runtime.md](docs/backend-runtime.md)；受限症状与处方见 [docs/confinement-integrity.md](docs/confinement-integrity.md)。

## 6. AI 与 ComfyUI

- **工具清单只有一个真源**：`server-rs/src/tools/tools.rs` 的 `catalog`（桌面）/ `mobile_catalog`（手机）；系统提示、`GET /api/ai/tools`、界面都从它派生。
- **写入必须过 `ToolPolicy.resolveWrite()`**，`.git` / `.mysql` / `.run` / `node_modules` 永远禁写；工具 handler 只从 `ToolContext` 拿策略 / Skills / 记忆。
- **需要审批的工具必须先开闸门再发事件**，超时与取消都算拒绝。**提问有两条别合并**：动手前缺信息用 `ask_user_question`（阻塞），说完了让用户挑用 `ask_follow_up`（非阻塞）。
- **工具结果与长期记忆都当不可信数据**，不拼进系统提示的指令区。
- **附件三层准入一条都不能少**（选完预检 → 发送前禁用按钮 → 创建 Run 前用库里事实再验）；发不了必须说出来，不许静默丢弃。
- **产物入口只认后端给的 `mediaIds`**；失败 / 取消时已经说出去的话一个字都不能丢。
- 提交与进度、标题、事件流、配方 / 蓝图、Skills、系统提示与前缀缓存、工具结果缓存的细则见 [docs/ai-tools-wiring.md](docs/ai-tools-wiring.md)；功能说明见 [docs/ai-tools-and-skills.md](docs/ai-tools-and-skills.md)。

## 7. 环境约束

- 不需要 JVM（只有跑 `android/` / Gradle 时要带 `jlink.exe` 的完整 JDK，用 `COMFYHUB_JDK_HOME` 指定）；pub 源必须 `https://pub.dev`；脚本一律 `pwsh`。
- **出网代理**：公网走代理（`127.0.0.1:7890`），本机 / 内网强制直连；Rust 侧统一走 `infra::outbound`。
- **Android 构建**：AGP 9.4 + Gradle 9.6 + AGP 内建 Kotlin；`dl.google.com` / Maven 必须绕过代理，`github.com` 必须走代理。
- 视频插件补丁、WebView 选型与关窗回归、MCP 运行时的完整约束见 [docs/environment.md](docs/environment.md)。

## 8. 文档索引

| 想了解 | 读 |
| --- | --- |
| 功能与接口 | [README.md](README.md) |
| 现象 → 原因 → 处理 | [docs/pitfalls.md](docs/pitfalls.md) |
| 总纲、红线、验收判据 | [docs/implementation-master-plan.md](docs/implementation-master-plan.md) |
| 界面规范（导航 / 列表 / 文案 / 标题联动 / 翻页） | [docs/ui-conventions.md](docs/ui-conventions.md) |
| 帧耗时测量 | [docs/perf-measurement.md](docs/perf-measurement.md) |
| 后端运行时（进程 / 路径 / 落盘 / 装配） | [docs/backend-runtime.md](docs/backend-runtime.md) |
| 受限环境与完整性标签 | [docs/confinement-integrity.md](docs/confinement-integrity.md) |
| 环境、依赖、MCP 运行时 | [docs/environment.md](docs/environment.md) |
| AI 工具接线纪律 | [docs/ai-tools-wiring.md](docs/ai-tools-wiring.md) |
| AI 工具与 Skills 功能 | [docs/ai-tools-and-skills.md](docs/ai-tools-and-skills.md) |
| 权限清点 | [docs/permission-audit.md](docs/permission-audit.md) |
| 工作流配方 / 蓝图设计 | [docs/workflow-recipe-design.md](docs/workflow-recipe-design.md) · [docs/workflow-blueprint-design.md](docs/workflow-blueprint-design.md) |
| 配对与跨设备同步 | [docs/pairing-design.md](docs/pairing-design.md) · [docs/cross-device-sync-design.md](docs/cross-device-sync-design.md) |
| 移动 harness | [docs/mobile-harness-design.md](docs/mobile-harness-design.md) |
| Android AGP 9 迁移 | [docs/android-agp9-builtin-kotlin-migration.md](docs/android-agp9-builtin-kotlin-migration.md) |

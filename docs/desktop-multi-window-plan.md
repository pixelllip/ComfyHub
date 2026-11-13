# 桌面多窗口：可拆位置与落地方案

2026-11-13 随 Flutter 3.50 升级调研。结论是**这次升级不提供可用的官方多窗口**，
但应用里已经有几处适合拆成独立窗口，本文记录拆哪里、怎么拆、先拆哪个。

## 1. 结论：3.50 的官方多窗口还不能用

Flutter 3.50 的博客把多窗口写成"继续成熟"，但 SDK 里的实现仍是实验 API：

- `packages/flutter/lib/src/widgets/_window.dart` 文件头写明 **不要在生产应用里 import**，
  且所有公开入口都带 `@internal`；
- 总开关是 `isWindowingEnabled = debugEnabledFeatureFlags.contains('windowing')`，
  只在 **debug 的 feature flag** 下为真；release 里调用直接抛 `UnsupportedError`；
- 错误文案明确要求"切到 main 渠道并打开 windowing 特性"，stable 上不成立；
- `Window` / `WindowManager` 没有从 `package:flutter/widgets.dart` 导出。

所以多窗口只能走应用层方案：多引擎（`desktop_multi_window`）或第二进程。
官方的 `Window` / `WindowManager` 等转正再迁，不作为当前落地方案。

## 2. 现状（拆之前必须知道的三件事）

1. **只有一个 `MaterialApp`**：`lib/main.dart` 创建 `SettingsStore` / `BackendLauncher`，
   `lib/app.dart` 里的 `ComfyHubApp` 用 `MultiProvider` 挂 `LibraryStore` /
   `AiWorkspaceStore` / `NsfwStore`，`home: HomeShell`。
   所有页面共享这一棵 Provider 树，`_ContentNavigator` 负责"宽屏只换右半边"。
2. **后端是单进程、单端口**：`BackendLauncher` 在首屏拉起
   `<运行根>\backend\comfyhub-server-rs.exe`，端口只从 `.run\api.json` 读；
   正常关窗由它 kill 自己起的那一个，被强杀靠 `--watch-stdin` 的 EOF 退出。
   多窗口后，"谁负责起 / 谁负责关"必须重新定义。
3. **主窗口带原生约束**：`windows\runner\main.cpp` 初始 1440×920；
   `windows\runner\win32_window.cpp` 的 `kMinWindowWidthLogical = 360` 与
   `WM_GETMINMAXINFO` 只覆盖当前这一个窗口。多窗口要逐窗口重做最小尺寸、DPI 与位置恢复。

## 3. 值得拆成独立窗口的位置

按"价值 / 改造成本"排序。

| 位置 | 现有入口 | 页面依赖 | 拆分价值 | 主要改造点 |
| --- | --- | --- | --- | --- |
| **媒体详情 / 大图查看器** | 画廊格子 `gallery_page.dart`、AI 产物卡 `ai_home_page.dart`、提示词详情 `prompt_detail_page.dart`、标签结果 `tag_results_page.dart`、卡片示例 `cards_page.dart` | 只吃 `mediaId`（可选 `siblings` / `siblingIds`），自己向后端取数；页面内左右翻页与缩放 | 最高：双屏时主窗看画廊、副窗看图 / 视频；视频与缩放本来就适合独占一块屏 | 收藏、标签、NSFW 标记属于写操作，副窗改完要让主窗刷新；`siblings` 传的是整组对象，跨窗口只能传 id 列表 |
| **提示词详情** | `prompts_page.dart`、画廊关联产物 `media_detail_page.dart`、标签结果 | 只吃 `promptId` + `showParams`；只读为主 | 高：写提示词时把参考提示词钉在另一块屏 | 复制正文走系统剪贴板即可；若后续加"从这里复制到编辑器"，用 id 回传 |
| **工作流蓝图** | 「查看工作流」弹窗 → `showWorkflowBlueprint()`（`lib/widgets/workflow_blueprint_page.dart`） | `title` / `raw` / `diagnose`（异步函数）/ `save`（回调） | 高：画布实测约 9600×2400，全屏或跨屏查看才有意义 | **函数不能跨引擎传**：`diagnose` / `save` 要改成"传 promptId 或原始 JSON，窗口自己调后端复检 / 保存"；编辑态与脏标记要在窗口内自洽 |
| **训练台 WebView** | AI 工作台 → `TrainPage` | `webview_all` 的 WebView2 平台视图 | 中高：ComfyUI WebUI 放大屏；主窗继续管提示词与画廊 | 风险最高：副引擎里的 WebView2、输入法、DPI、关闭顺序都要重验（关窗回归见 `docs/environment.md`）；不建议第一期做 |
| 画廊 / 提示词列表本身 | `HomeShell` 五个一级页 | 共享 `LibraryStore` / 导航状态 / 侧边栏 | 低：拆两个主窗口等于复制整套导航与状态，收益只在极端双屏场景 | 不做；用系统窗口并排或"副窗只读详情"覆盖 |

不建议拆的：设置、数据目录迁移、同步与配对、审批弹窗。
这些要么是模态流程、要么有"只能有一个"的副作用，开第二个窗口只会制造竞态。

## 4. 三条落地路线

### A. `desktop_multi_window`（推荐先试）

pub.dev 上的 `desktop_multi_window` 0.3.1（2026-08 发布，Dart >=3.5），
Windows / macOS / Linux 均有实现。一个进程内为每个窗口建一个 FlutterEngine，
窗口之间通过 MethodChannel 传消息。

- 优点：单进程、后端天然共享；窗口 API 是 Dart 层，接入成本比自写原生低。
- 代价：**每个窗口一个独立 isolate**，Provider / ChangeNotifier 不共享；
  副窗口要自己建 `MaterialApp`（同主题、同 `localizationsDelegates`）、自己取数。
- 必须验：副窗口里视频纹理、WebView、输入法、DPI、关闭主窗后副窗行为。

### B. 第二进程（`viewer.exe --window=media --id=42`）

`main()` 解析启动参数，副进程只跑一个精简 `MaterialApp`（详情页 / 提示词页）。

- 优点：隔离最彻底（崩溃互不影响），复用现有 `main` 与页面；不需要新插件与多引擎。
- 代价：多一份内存与启动延迟；**必须新增 attach 模式**：检测到 `.run\api.json` 可用时
  直接连已有后端、不启动第二个、退出也不 kill；只有主进程负责后端生命周期。
- 适合做"最小验证"：改 `main()` + 一个只读窗口壳，不动现有 `HomeShell`。

### C. 官方 `Window` / `WindowManager`

当前只能在 debug feature flag 下跑，release 抛错；等转正后重做，届时窗口管理会
上移到 `WidgetsApp` 之上，副窗口可以有独立导航栈。现在只保留观察，不写代码。

## 5. 无论走 A 还是 B，都要先做的架构改造

1. **抽 `AppServices`**：把 `SettingsStore`、`BackendLauncher`、各 store 的创建从
   `main()` / `ComfyHubApp` 里抽出来，主窗口与副窗口都能拿到同一份（A 路线下按窗口重建，
   B 路线下各进程一份，但都从后端取数）。
2. **窗口描述对象**：`{kind: media|prompt|blueprint, args: {...}, windowId}`。
   入口按钮（右键菜单 / 详情页菜单）只负责创建描述，不直接 `Navigator.push`。
3. **后端归属**：主窗口启动并持有后端；副窗口只读端口文件。
   关闭主窗口时若还有副窗口，要么把所有权交出去，要么全部关闭并提示；
   不能出现"主窗关了、副窗还连着死端口"。
4. **数据一致性**：副窗口写操作（收藏 / 标签 / NSFW）走同一个后端 API，
   完成后通过 MethodChannel（A）或重新拉取（B）让主窗口刷新；
   不要假设 `ChangeNotifier` 能跨窗口 / 跨进程。
5. **窗口几何**：每个窗口单独记尺寸与位置（偏好键带窗口类型），
   DPI 变化时按 `WM_DPICHANGED` 重算；最小宽度仍用 360 逻辑像素。
6. **测试**：`flutter test` 只能覆盖"窗口参数解析 + 副窗口页面构建 + 不允许的入口不出现"；
   真正的开窗 / 关窗 / 后端退出要 PowerShell 回归：枚举窗口、发 `WM_CLOSE`、
   查 System 日志 `Application Popup`（ID 26）为 0、查后端进程数归零。

## 6. 建议顺序

1. **媒体查看器只读副窗**（B 路线最小验证）：`--window=media --id=N`，attach 后端，
   验证双屏体验与关闭行为。
2. 体验达标后，再评估换 A 路线（统一窗口管理、支持从副窗再开下一个）。
3. **提示词详情副窗**：入口简单，顺带验证"副窗只读、主窗刷新"。
4. **工作流蓝图副窗**：先把 `diagnose` / `save` 从回调改成 id + 后端调用，再开窗。
5. **训练台 WebView 副窗**：最后做，先过 `webview_all` 的关窗回归与视频纹理回归。

## 7. 回归清单（多窗口版本）

- 关窗：`WM_CLOSE` 后 System 日志 `Application Popup`（ID 26）必须是 0 条；
- 后端：主窗关掉后后端进程必须退出；副窗关闭不能让主窗的后端退出；
- 视频：Windows 上 `video_player_win` 画面正常（不能只有声音）；
- WebView：训练台加载、输入、关闭都正常；
- 多显示器：窗口拖到不同 DPI 的屏幕后尺寸与字号正确；
- 数据：副窗收藏 / 打标签后，主窗画廊与详情页能看到新状态；
- 单实例：重复启动不会起第二个后端、不会抢端口。

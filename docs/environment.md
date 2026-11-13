# 环境、依赖与运行时约束

> 这是 `AGENTS.md` 第 7 节「环境约束」的细则。换机器后要重新确认这里每一条。

## 1. 本机环境硬约束

| 约束 | 说明 |
| --- | --- |
| **不需要 JVM** | 后端是静态链接的 Rust 单文件 exe、库是 SQLite 单文件。**只有跑 `android/` / Gradle 时才要完整 JDK（带 `jlink.exe`）**，用 `COMFYHUB_JDK_HOME` 指定；JDK 25 可用，JBR jcef 21 缺 jlink 会失败。MySQL / JRE / Kotlin jar 已退役，别去找 `mysql.ps1` 或 `.mysql\` |
| pub 源 | 必须 `https://pub.dev`（国内镜像对个别包返回 424） |
| 脚本 | 一律 `pwsh`；App 与后端的起停不走脚本（见 [`backend-runtime.md`](backend-runtime.md) §2） |
| 端口 | 后端 `--port 0`，真实端口只从 `.run\api.json` 读；ComfyUI `8188` |
| 出网代理 | 本机 `civitai.com` 这类直连不通、只有 `127.0.0.1:7890` 通；**公网走代理、本机 / 内网强制直连**（否则 `127.0.0.1:8188` 会被交给代理打死 ComfyUI）。Rust 侧统一走 `infra::outbound`（`COMFYHUB_HTTPS_PROXY` → 系统代理 → 直连；写 `direct` 可强制直连），别自己发明第三份选择器。系统代理那一档会读 `ProxyEnable` / `ProxyServer` / `ProxyOverride`（**例外表照办**；PAC 读不了会如实报）。要当场看结论或跑探针：`GET /api/system/outbound`（设置 → 本地服务 → 联网代理）—— "503 是不是没走代理"这件事别再靠猜 |
| Android 构建 | `android/` 走 AGP **9.4.0** + Gradle **9.6.0** + **AGP 内建 Kotlin**（`android.builtInKotlin=true`，任何模块都不 apply KGP）；`~/.gradle/gradle.properties` 里 `dl.google.com` / `repo.maven.apache.org` **必须绕过代理**、`github.com` **必须走代理**；pub 缓存在 C:、工程在 D: 会跨盘炸，所以关了 `kotlin.incremental`。动 `android/` 前先读 [`android-agp9-builtin-kotlin-migration.md`](android-agp9-builtin-kotlin-migration.md) |

## 2. MCP 运行时：可选能力，检测优先

唯一还会下载东西的是**可选能力** MCP 运行时（`scripts\mcp-runtime.ps1`，给 AI 添一批 ComfyUI 工具）。策略是**检测优先、安装只作兜底**，四档：① 已装好的（`state.json` / `venv`）→ ② 借已装 comfy-mcp + comfy-cli 的 Python（一个字节都不装）→ ③ 借系统 Python 建 venv（`<根>\storage\ai\mcp\venv`）→ ④ 才下 embeddable 便携版；用了哪一档记在 `state.json`。

- ⚠ **venv 的解释器就在 `Scripts\` 里**：一律 `父目录\Scripts` 会拼出 `venv\Scripts\Scripts`，表现是"明明装好了却报缺 comfy-mcp.exe"。
- **装不上不拦启动**：缺了只是 AI 少一批工具。

### 两条通用 PowerShell 教训

> **⚠ 参数改名**：非高级函数（没写 `[CmdletBinding()]`）收到不存在的命名参数**不报错**，而是丢进 `$args` —— 改名后内部一堆 `-Proxy $X` 会**静默失效**。改名必须同时改调用点，给命令行用的名字留 `[Alias('旧名')]`。
>
> **⚠ 别给变量起名 `$home` / `$input` / `$host` / `$pid` / `$profile` / `$args`**：自动变量**大小写不敏感**，`$home = …` 直接抛只读错误；更可怕的是漏改一处引用 —— 本仓库真写出过 `$javaHome = …`（定义）却仍用 `$home`（引用），于是 `Move-Item $home …` 去搬**整个用户目录**。**写任何 `Move-Item` / `Remove-Item -Recurse` 之前，先确认目标是"算出来的、可验证的"路径。**

## 3. 视频播放依赖一份打过补丁的插件副本

`pubspec.yaml` 里的 `video_player_win` 指向 **`third_party\video_player_win`**（上游 3.2.2 的本地副本），不是 pub.dev：上游的 `GpuSurfaceDescriptor` 少了 `visible_width / visible_height`，而 Flutter 3.4x 在 Windows 上默认用 **Impeller**、拿这两个字段当外部纹理尺寸 —— 结果是**只有声音、画面全黑**（日志里 `Could not create external texture`）。

- 改动点两行，搜 `ComfyHub 补丁` 定位（`windows\video_player_win_plugin.cpp` 的 `initTexture()`）。
- **别"顺手升级"回 pub.dev**，也别在 `flutter pub upgrade` 后不看 `pubspec.lock` —— 一升回去立刻复发。上游修好后再整体换回并删掉这一节。
- 排查顺序：① 控制台有没有 `Could not create external texture`；② 日志里是不是 `Using the Impeller rendering backend`；③ `--no-enable-impeller` 跑一遍对照（Skia 会容忍 0 尺寸，所以它在旧版本里"看起来是好的"）。

## 4. WebView 用 webview_all（**不要换回 flutter_inappwebview**）

训练台内嵌 WebUI 用 `webview_all: 1.4.12`（`lib\pages\train_page.dart`）。**2026-10-23 从 `flutter_inappwebview: 6.1.5` 换过来，原因是关窗必弹 `Unknown Hard Error`**：

- 根因在 `flutter_inappwebview_windows 0.6.0` 的 `InAppWebViewManager` **构造函数**（插件注册时就跑，哪怕一个 WebView 都没创建）：`RoInitialize` 后创建 `DispatcherQueueController` + `Compositor`（`graphics_context.cc`），收尾时按错误顺序释放 → csrss 报硬错误（事件日志 `Application Popup` ID 26；WER 里模块 `dcomp.dll`、异常码 `0xe0464645`）。上游 [issue #2512](https://github.com/pichillilorenzo/flutter_inappwebview/issues/2512) 关了没修。
- **判据**（别凭手感）：不注册 webview 插件 → 关窗 0 条事件；只注册它 → 关窗必现（当时的逐插件开关 `COMFYHUB_PLUGINS` 已随修复删除；改回来只需在 `FlutterWindow::OnCreate` 里按名字挑一条 `RegisterWithRegistrar`）。
- `webview_all` 是 `webview_flutter` 的跨平台实现（Windows/Android/iOS/macOS/Linux/Web），1.4.1 修过同类"退出即崩"（[#37](https://github.com/abandoft/webview_all/issues/37) / [#38](https://github.com/abandoft/webview_all/pull/38)）；Android 端支持 AGP 9 内建 Kotlin，所以换过来删掉了 Android 的 `third_party\flutter_inappwebview_android` + `dependency_overrides`。⚠ Windows 的 `<experimental/coroutine>` 逃生宏**仍然要留**（1.4.12 的 C++/WinRT 也没躲开 MSVC 14.51 的 C2338），见 `windows/CMakeLists.txt` 的注释。
- **版本钉死**：`pubspec.yaml` 写 `webview_all: 1.4.12`（2026-11-05 从 1.4.5 升上来）。**每次升级都要重跑关窗回归** —— 1.4.0 就出现过"应用退出即崩"，这类问题只在**关窗那一秒**出现，`flutter test` 抓不到。
- **关窗回归**（Windows）：`flutter build windows --debug` → 启动 `build\windows\x64\runner\Debug\viewer.exe` → 等窗口出来 → 发 `WM_CLOSE` → 查 `Get-WinEvent -FilterHashtable @{LogName='System'; Id=26; StartTime=...}` **必须为 0 条**（`Application Popup` 事件就是那个弹窗）。⚠ Debug 产物用的是源码树的后端，不经过 release 装配。
- Android 侧仍按老规矩 `flutter build apk --debug` 过一遍（JDK 见 §1）。
- WebView2 运行时是 Windows 的硬前提（Win10 1809+ / Win11 自带）。加载失败时 `onWebResourceError` 只在**主文档**失败时提示；子资源 404 不该把整页判死。

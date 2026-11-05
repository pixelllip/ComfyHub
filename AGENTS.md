# AGENTS.md

给在这个仓库里干活的 AI / 协作者看的注意事项。**功能与接口看 [README.md](README.md)，现象 → 原因 → 处理看 [docs/pitfalls.md](docs/pitfalls.md)**；这里只记"怎么做才不踩坑"。

> **⚠ 两条底线（2026-10-17 一次事故换来的，别再交学费）**
>
> 1. **批量 / 脚本化操作之前先落基线**：`git commit`（或 `git stash`）。仓库长期挂着上百个未提交改动 = 没有回退点 —— 那次 14 个源文件被清零后，唯一能秒回的恰恰是"与 HEAD 相同"的那 6 个，另外 8 个只能从会话日志重放。
> 2. **代码改动只走 `edit` / `write` 工具，不要用脚本写文件**。脚本写的既不在 harness 快照里，也容易一把清零：`.NET` 的 `File.*` 用的是**进程 CWD**，不是 PowerShell 的 `Set-Location`，所以"读失败"与"写成功"会同时发生。脚本只做只读查询。

---

## 1. 改前端用 debug + 热重载；测试只留能抓回归的

- 只动 `lib/` 下的 Dart → `pwsh -File scripts\dev-app.ps1`（保证后端二进制最新 + `flutter run -d windows --debug`），起来后按 `r` 热重载。
- **别为了看一个前端改动跑 `flutter build windows --release`**：改 `windows/` 原生代码、改依赖、要出正式产物时才需要（它末段自己 `cargo build --release` 并把后端装进 `backend\`）。
- 改完过 `flutter analyze` + `flutter test`（`PUB_HOSTED_URL=https://pub.dev`）。详情页大图 / 工作流弹窗这类交互改动跑对应用例（`test\zoomable_image_test.dart`、`test\workflow_viewer_test.dart`），别只跑 `widget_test.dart`。
- **测试的唯一标准是"删了以后出错还能不能被抓住"**：同一事实写两遍、为用掉 import 的占位用例，该删就删；真正的防线（零上游请求、审批顺序、越界写被拒、内联预算、滚动条与懒构建）一条都不能删。本机实测：`flutter test` ≈ 25s、`cargo test`（lib 630 + 集成 150）≈ 30s（热构建）、`e2e-ai-tools-test.ps1` ≈ 5~15s、`e2e-capture/submit-test.ps1` ≈ 20~35s。
- **Rust 用例要落盘只许 `testpaths::scratch()`**（落点 `<crate>\target\.scratch\`，`tests/` 里是 `comfyhub_server_rs::infra::testpaths::scratch()`），**别用 `std::env::temp_dir()`** —— 工作区里编出来的测试二进制带 Low 标签（§12），受限会话里必然 `os error 5`。要造"写不进去"就用**同名目录占位**，别改 ACL。
- 产物：debug / release 在 `build\windows\...\runner\{Debug,Release}\`。**debug 产物里没有 `backend\`**，用的是 `server-rs\target\{release,debug}\` 那份 —— 改了 Rust 只要 `cargo build`，不用重建 App。
- **"我改了但界面没变"先查构建新鲜度**，别怀疑代码：跑的可能还是旧产物（Release 看 `data\app.so`、debug 看 `data\flutter_assets\kernel_blob.bin`）。界面类修复必须 `flutter build windows --debug`（或 `dev-app.ps1`）重启一次再截图确认；后端看 `scripts\backend.ps1 status`。

## 2. 本地服务必须静默启动（不弹命令行窗口）

- 后端由 App 自己起（`lib\core\backend_launcher.dart` **直接执行 exe**）：Dart 的 `Process.start` 在 Windows 上**自带 `CREATE_NO_WINDOW`** —— 这就是"不弹黑框"的全部原因，别再引入 WMI / wscript 那一套。
- **"谁起的谁关"**：正常关窗 → `BackendLauncher.releaseOnExit()` kill 自己起的那一个；被任务管理器 / 崩溃强杀 → 后端启动时带的 **`--watch-stdin`** 生效（stdin 写端只有 App 持有，App 一死读到 EOF 优雅退出）。回归 `server-rs/tests/watch_stdin_test.rs`。
- 脚本旁路 `scripts\backend.ps1 start`（给 e2e / dev）用 `Start-Process -WindowStyle Hidden`，**不带 `--watch-stdin`**（Start-Process 给的不是管道，带了后端会当场退出），也不脱离进程树。
- **验证"没弹窗口"要枚举窗口**：控制台窗口属于 `conhost.exe`，`Get-Process cmd | Select MainWindowHandle` 永远是 0（假绿灯）。正确做法 `EnumWindows` + `IsWindowVisible` 按 ~50ms 轮询；**别用 `Start-Process` 起后端来测这条**（它本来就是"不保证无窗口"的那条路）。

## 3. 首页 / 导航顺序

`lib/app.dart` 的 `HomeShell` 默认落在「AI 工作台」，顺序 `AI 工作台 → 画廊 → 提示词 → 标签 → 设置`；`_destinations`（图标 / 文案）与 `pages` **必须同序**（`_index` 同时索引它们）。改完跑 `flutter test test\home_nav_test.dart`；`test\localization_test.dart` 也用首页做锚点。

一级导航**只有 5 项**：窄屏上第 6 个图标会挤成两行，所以新功能优先做成**次级页**或挂进已有页面（角色卡 / 画风卡的「卡片库」就是这么挂在「预设库」页的）。

## 4. 本机环境硬约束（换机器要重新确认）

| 约束 | 说明 |
| --- | --- |
| **不需要 JVM** | 后端是静态链接的 Rust 单文件 exe、库是 SQLite 单文件。**只有跑 `android/` / Gradle 时才要完整 JDK（带 `jlink.exe`）**（用 `COMFYHUB_JDK_HOME` 指定；本机实测 JDK 25 可用，JBR jcef 21 只有运行时、缺 jlink 会失败）。MySQL / JRE / Kotlin jar 都已退役 —— 别去找 `mysql.ps1`、老库搬迁或 `.mysql\` |
| pub 源 | 必须 `https://pub.dev`（国内镜像对个别包返回 424） |
| 脚本 | 一律 `pwsh`。**App 与后端的起停不走任何脚本**，脚本只剩构建 / 测试 / 可选能力 |
| 端口 | 后端 `--port 0` 让系统挑，真实端口只从 `.run\api.json` 读；ComfyUI `8188` |
| 出网代理 | 本机 `civitai.com` 这类直连不通、只有 `127.0.0.1:7890` 通；**公网走代理、本机 / 内网强制直连**（否则 `127.0.0.1:8188` 会被交给代理打死 ComfyUI）。Rust 侧统一走 `infra::outbound`（`COMFYHUB_HTTPS_PROXY` → 系统代理 → 直连；写 `direct` 可强制直连），别自己发明第三份选择器 |
| Android 构建 | `android/` 走 AGP **9.4.0** + Gradle **9.6.0** + **AGP 内建 Kotlin**（`android.builtInKotlin=true`，任何模块都不 apply KGP）；`~/.gradle/gradle.properties` 里 `dl.google.com` / `repo.maven.apache.org` **必须绕过代理**、`github.com` **必须走代理**；pub 缓存在 C:、工程在 D: 会跨盘炸，所以关了 `kotlin.incremental`。动 `android/` 前先读 `docs/android-agp9-builtin-kotlin-migration.md` |

## 5. 改脚本时的约定

- **App 的启动逻辑只有一份，在 Dart 里**（`lib\core\backend_launcher.dart` + `backend_paths.dart`）。起停后端不经过脚本，所以别在 `.ps1` 里另写一份"怎么起后端"。
- 新增脚本前先问一句"这件事能不能不进 `scripts\`"；真要加就用 `pwsh -File scripts\xxx.ps1 <动作>` 的形式，并在 README「脚本速查」里登记。
- **`scripts\backend.ps1` 是唯一允许碰后端的脚本**（给 e2e / dev），它是 `lib\core\backend_paths.dart` 的**镜像** —— 改一边必须改另一边（§7）。
- PowerShell：`& script.ps1` 的 **stdout 会成为表达式的返回值**，在 `$ok = & other.ps1 …` 这种地方必须 `| Out-Null`，否则返回值被输出数组污染、判断永远为真。

## 6. 改界面时的硬约定

### 6.0 两端一起设计

任何界面改动都要在**桌面端和移动端**各问一句"这好用吗"。形态由**一个数字**划开：`lib/app.dart` 的 `kWideLayoutMinWidth = 900`（逻辑像素）。

| | 桌面端（≥ 900） | 移动端（< 900） |
| --- | --- | --- |
| 导航 | 左侧 `NavigationRail`，**常驻** | 底部 `NavigationBar` |
| 次级页（详情 / 结果页…） | **只替换右边内容区**，侧边栏一个字不动 | **铺满整个界面**（底边栏一起盖住） |
| 卡片 | 宽了分列（见 6.1） | 窄了退回单列，**360 宽不许溢出** |

- **次级页的落点按宽度分**：`_ContentNavigator` —— 宽屏套一层内容区 `Navigator`，窄屏**不套**、让 `Navigator.of(context).push` 落到 `MaterialApp` 的根 Navigator 上。回归 `test\content_navigator_test.dart`（宽窄两组都在）。
- **窗口最小宽度 360 逻辑像素**（`windows\runner\win32_window.cpp` 的 `kMinWindowWidthLogical`），按 DPI 换算。
- **每个断点只有一份真源**：宽度阈值只有 `kWideLayoutMinWidth`，别再在页面里写 900 / 360。

### 6.1 列表与卡片

- **列表一律用 `lib/widgets/adaptive_layout.dart` 的多列组件**，别自己写单列 `ListView`：高度接近的用 `AdaptiveColumnList`，高度差几倍的用 `AdaptiveColumns`；列数 = 可用宽度 / 550（最多 4 列）。回归 `test/adaptive_layout_test.dart`。
- **卡片宽度按"里面装了多少东西"定**，别一律 550：内容少的（标签卡：名字 + 一行说明 + 计数 + 两个按钮）用 `minColumnWidth: kColumnWidth / 2`（275）。回归 `test\tags_layout_test.dart`（宽屏分列与窄屏单列两种形态都钉了）。
- **界面语言只能有一种**：App 文案本来就是中文，但文本框选择菜单、返回按钮 tooltip 这类**系统文案来自 `MaterialLocalizations`** —— `lib/app.dart` 必须留着 `locale: zh_CN` + `flutter_localizations` 三个 delegate，少一个就冒英文 "Copy / Select all"。用例 `test/localization_test.dart`。
- 弹出菜单统一用 `AppMenuButton`（按钮下拉）/ `showAppContextMenu`（右键），页面根部挂 `ContextMenuScope`（`lib/widgets/app_menu.dart` + `context_menu.dart`，`common.dart` 里转出）。**别改回 `PopupMenuButton` / `showMenu`**：它们铺的 `ModalBarrier` 会把整页滚轮和拖动全吃掉（上游 flutter/flutter#90223）。回归 `test/menu_scroll_test.dart`。传 `context` 要用**在 `ContextMenuScope` 里面**的那一个。
- 开关行别直接 `SwitchListTile(contentPadding: EdgeInsets.zero)` 贴卡片边缘：用 `settings_page.dart` 的 `_SwitchRow`（自带内边距 + Material 底色；底色不能用 `Container` 的 decoration，否则盖掉水波纹并触发断言）。
- **长列表必须懒构建**，别用 `children: [for (...) ...]`：模型卡片、聊天气泡这类一律 `ListView.builder`；流式刷新（逐字回包）时每项包 `RepaintBoundary`。`header` 只放**固定几条**，要按状态拼的用 `switch` / `if` 分支 —— 拼回 `children` 懒构建就失效了。
- **高矮差大的长列表用「固定行高 + 编辑弹窗」**，别让每行自己撑高：懒构建的 `ListView` / `SliverList` 只能用平均高度估 `maxScrollExtent`，滚动条滑块会越滚越短。正解是 `CustomScrollView` + `SliverFixedExtentList(itemExtent: 常数)`，行的详细编辑搬进 `showDialog`；代价是每行高度恒定，**别在行里塞会换行的 `Wrap`**。回归 `test\model_list_scroll_test.dart`。
- **大块状态不许占页顶**（用户原话："模型能力目录区域占用窗口区域太大"）：与当前选中项无关的大块内容做成折叠项或详情内的一节，别做常驻横幅。回归 `test\phone_layout_overflow_test.dart`（384 / 320dp 两档都不许溢出）。
- **工作台的模型选择器按 Provider 分组**：`SliverToBoxAdapter(来源标注)` + `SliverPadding(SliverList(模型))` 一组一组。数据源是 `AiWorkspaceStore.catalogByProvider`（打开选择器时才拉），**不是** `store.models`；换 Provider 走 `selectModelFrom(p, m)` —— 别拆成"先 `selectProvider` 再 `selectModel`"，那两步都是 `await`，中间那次选中会被 `_loadModels()` 收尾时的"取第一个启用的"覆盖。回归 `test\ai_model_picker_test.dart`。
- **内置模型目录那张卡与它的 Dart 客户端已删**：能力预填改由 **models.dev** 在线目录负责（`capability_refresh.rs`）。后端 `server-rs\src\provider\builtin.rs` 与 `/api/ai/builtin/{status,sync}` 还在（Rust 用例盯着），但界面上**没有任何入口** —— 别再把 `AiBuiltinCatalogStatus` / `builtinStatus()` 加回来。
- **手机「从电脑同步 Provider」是"用户显式点的一次导入"，不是自动合并**（`lib/core/provider_sync.dart`，契约 [`docs/pairing-design.md`](docs/pairing-design.md) §14.6）：桌面有 / 手机没有 → 新建；同 id → 桌面覆盖（提交的是**本机** `revision`，两端的库各算各的）；只在手机上的 → **保留不删**；模型目录**只在真的拿到时才写**（拉取失败绝不许回落到空目录）；`api` 不在手机那三种协议里的**跳过并如实说**。按钮**只在手机上渲染**。⚠ 这一页**不要自己去读配对令牌**（`PairingTokenStore`）—— `AiApiClient` 默认取进程内缓存即可；硬读法在真机之外的宿主上会悬着不返回（表现是"点了同步永远转圈"，2026-11-01 踩过）。回归 `test\provider_sync_test.dart`。
- **手机端的落盘缓存必须有上限，而且必须看得见、清得掉**（用户 2026-11-01 问"要不要设缓存上限" → 要）。现在唯一一个是 `lib/core/media_cache.dart`：缩略图 150MB、原件默认 1.5GB（设置页「产物缓存」可调 512MB~4GB），超过按**最久未用**淘汰到 80%（不是"到顶就不缓存"，那会变成随机看不了图）；打开详情时**超过 200MB 的视频 / 音频不自动下**，要存到手机是显式动作；保存进相册后大文件在离开页面时回收缓存副本。设置页那一节用 `lib/widgets/media_cache_card.dart`（独立 widget 才测得到 —— 它只在手机上渲染，`flutter test` 跑在 Windows 上），320 / 384dp 两个宽度都有用例。⚠ 落点在 `files/`（不是 `cacheDir`）：系统不会替你清，所以**预算 + 淘汰 + 清空入口**三件缺一不可；`android/app/src/main/res/xml/data_extraction_rules.xml` 把 `media-cache/` 排除出 Auto Backup（25MB 配额，注定失败）。回归 `test\media_cache_test.dart` + `test\media_cache_card_test.dart`。

### 6.2 文案：像人说话，不要像模型在列提纲

**2026-11-05 定下的硬约束**，界面文案（标题 / 副标题 / 空状态 / tooltip / SnackBar / 错误提示，以及后端回给界面的那一句人话）都按这个来：

- **不许出现 `a:b` / `c;d` 这种话术**：`集合详情：宽屏`、`当前模型：X`、`允许 = 直接执行`、`A；B` 都不行。能一句话说完就一句话；一句话装不下就拆成两句，别拿分号硬连。
- **能用一句人话说完的，不加从句、不套括号**：先给结论，再给条件；别写"当……时，如果……则……"。
- **用用户熟悉的名词，少用专有名词**：模型、图片、画廊、设置、服务、缓存；别把类名、字段名、内部代号（`run-kit`、`WidgetsBinding`、`AiWorkspaceStore`）直接甩到界面上。必须提的时候，先用一句人话解释它是什么。
- **错误提示同样说人话**：`保存失败，磁盘空间不够。`，不要 `保存失败：ENOSPC`；不要把异常栈 / 内部错误码当正文。
- **不支持 Markdown 的地方不许写 Markdown**：`Text` / `SelectableText` / `SnackBar` / `InputDecoration` 的 label / hint / helper 都不解析 Markdown，`**粗体**`、反引号、`# 标题`、`- 列表` 会原样显示。要强调就用 `Text.rich` / `TextSpan`（`fontWeight: FontWeight.bold`），要排版就用本来就会渲染 Markdown 的 `MarkdownText`；不要指望纯文本控件自己变粗。
- **先确认渲染器再写格式**：一个字符串会进 `MarkdownText`（聊天正文、蓝图详情）才可以用 Markdown；进 `Text` 的按纯文本写 —— 工具卡的结果区也是 `Text`，后端别往里塞 Markdown。后端回给界面的字符串同理，前端用 `Text` 接，它就不该带 Markdown。
- **例外只有字面量**：URL、比例（`16:9`）、代码 / 路径 / 键名，按原样写。
- **改文案要同步改测试断言**（不少用例直接 `find.text('……')`），改完跑 `flutter analyze` + `flutter test`；后端回给界面的那句也要一起改，别只改前半句。

### 6.3 大标题与顶栏标题的滚动联动

**2026-11-05 定下的硬约束**：页面顶部有**大标题**时（设置家族、画廊、预设、标签……），
这个标题属于**主体内容**；它随内容滚出可视区时，**同名标题要在顶栏里逐渐出现**，
滚回顶部时再退回去。两种做法都不允许：

1. 大标题滚走了、顶栏什么都不显示（用户不知道自己在哪一页）；
2. 顶栏标题一直挂着、主体里再放一个同名大标题（同一个标题出现两次）。

**按平台分两套**（用户 2026-11-05："手机端 UI 维持现状，电脑端的标题恢复原状
（始终在 appbar 里）"）：

- 判据是 `useCollapsingPageTitle`（`scroll_linked_title.dart`：Android / iOS 为真），
  **不是窗口宽度** —— 电脑窗口拉窄到 900 以下仍是电脑端，不该长成手机的样子；
- **电脑端（Windows / macOS / Linux）**：标题**始终在 AppBar 里**，正文不再放大标题，
  滚动也不做标题动画；
- **手机端（Android / iOS）**：才用大标题 + 空白 AppBar + 滚动淡入那一套。
- **返回键一律在 AppBar 里**（用户 2026-11-05："这种返回按钮应该待在 appbar"），
  用带外圈的 40×40 圆形 `BackButton`；正文的页头不再放返回键。

- 组件只有一份：`lib/widgets/scroll_linked_title.dart`
  - `ScrollTitleProgress`：进度的唯一真源（0 = 大标题完整可见，1 = 完全离开）；
  - `ScrollTitleListener`：把**最外层**滚动（`depth == 0`）换算成进度；
  - `CollapsingPageTitle`：主体里的大标题，随滚动上移 + 淡出；
  - `FadingAppBarTitle`：有 `AppBar` 的页面用它；进度为 0 时**不建**（避免两个同名标题）；
  - `FadingTopBar`：真正**没有 `AppBar`** 的页面才用它（浮层、不占布局高度）。
- **设置家族（`SettingsScaffold`）必须预留一条空白 AppBar**（用户 2026-11-05：
  "设置页应预留空白 appbar"）：大标题在正文里滚走，同名标题在这条 AppBar 里淡入，
  返回键也跟着一起出现；**不要用浮层压住正文**。
- 接入新页面时**只接这一套**，不要再手写 `Opacity` / `setState` 各一套。
- 标题块高度可以按页面微调（画廊 44，其余默认 56），但**判据只能是
  `ScrollTitleProgress`**，不要另算一套阈值。
- 回归：`test/scroll_linked_title_test.dart`（顶部只有大标题 / 滚动后顶栏出现 /
  滚回顶部收回）。

---

## 7. 两种运行布局：源码树 vs 发布包（改路径解析前必读）

"找东西"的逻辑只有一份真源：**`lib\core\backend_paths.dart`**；`scripts\backend.ps1` 是它在脚本世界的**镜像**（只服务 e2e / dev）。

| 找什么 | 源码树（开发） | Release 产物 / 发布包 |
| --- | --- | --- |
| 运行根目录 | 往上找 `server-rs\Cargo.toml`（或 `db\schema.sqlite.sql`、`backend\comfyhub-server-rs.exe`） | `viewer.exe` 所在目录本身 |
| 后端 exe | `<根>\server-rs\target\{release,debug}\comfyhub-server-rs.exe` | `<viewer.exe 同级>\backend\comfyhub-server-rs.exe` |
| `viewer.exe` | `build\windows\...\runner\{Debug,Release}\` | `<根>\viewer.exe` |
| 数据库 / 产物 / 端口 | `<根>\.db\` / `<根>\storage\` / `<根>\.run\api.json` | 同一份约定 |
| Java | **不需要** | **不随包携带** |

- **后端 exe 的候选顺序只有一个真源**：`serverExeCandidates()` = `$env:COMFYHUB_SERVER_EXE` > `<exe 同级>\backend\` > `<根>\backend\` > `<根>\server\`（旧布局） > `server-rs\target\{release,debug}\`。`COMFYHUB_SERVER_EXE` **永远最高优先**，它是"把后端映像挪出受限区"的唯一开关（§12）。
- **根目录判据必须同时认两种布局**（三种判据任一命中即可），否则发布包里会出现"文件明明在，它却说找不到"。
- **端口只有一个真源**：后端绑好之后自己写 `<根>\.run\api.json`（写者是 `api_port_file.rs`）；App（`settings_store.dart`）、脚本（`api-port.ps1`）、捕获扩展（`capture_core.py`）**都只读它**，谁也不许自己编端口。
- ⚠ **"要不要听用户的"只有一条判据**：`SettingsStore.isLocalBackendUrl()` —— **回环地址永远跟着端口文件走**（哪怕它先前被写进过偏好），只有**指向别的机器**的地址才算用户覆盖。**别退回"偏好里有值 = 用户手填过"**：端口是 `--port 0` 自动挑的（重启就换），那条判据会把自动发现的地址存成"用户配置"，从此界面对着死端口等满 60 秒超时。回归 `test\settings_base_url_test.dart`。
- **`--port 0` 是刻意的**：写死端口会撞"被占 / 落在 Windows 保留段"，而且真实端口要从 listener 读回来（`--port 0` 时那个 0 不是端口）。
- **`infra::db::open_or_create` 是"第一次运行能起来"的关键**：库缺就自己建（DDL 编译期 `include_str!` 嵌在二进制里）。别再退回"缺库就退出"。
- **发布包是便携式的**：可写数据（`.db` / `storage` / `.run`）都在包内，`packaging\manifest.json` 的 `runtimeLayout` 记着这份约定；改存放位置要先改它。
- **Release 产物默认"就地装配"**（`windows\CMakeLists.txt` + `windows\cmake\bundle_backend.cmake`），三个连带后果别踩：① `flutter clean` 会把它整个清空，重跑一次 release 构建即可；② `pack-release.ps1 -Clean` **不能**整个 `Remove-Item $OutDir`（会把 `viewer.exe` 一起删了），只清清单里的 target + 老组件（`server` / `jre` / `mysql`），运行期数据（`.db` / `storage` / `.run`）**不要动**；③ `flutter_release` 组件此时**源和目标是同一个目录**，必须跳过拷贝。
- **`cargo build --release` 在后端正在跑时必失败**（Windows 锁着运行中的 exe，报"拒绝访问 os error 5"）：先 `pwsh -File scripts\backend.ps1 stop`。
- 想要一份不随 `build\` 消失、又在**工作区外**的长期副本：`pwsh -File scripts\pack-release.ps1 -OutDir D:\dist\ComfyHub`（也是绕开写限制的正解，§12）。
- 旧 Kotlin 后端（`legacy/`）**已整体删除**，要对照行为只能从 git 历史取：`git log --diff-filter=D -- legacy` → `git show <那次>^:legacy/server/...`。

## 8. MCP 运行时：可选能力，检测优先

唯一还会下载东西的是**可选能力** MCP 运行时（`scripts\mcp-runtime.ps1`，给 AI 添一批 ComfyUI 工具）。策略是**检测优先、安装只作兜底**，四档：① 已装好的（`state.json` / `venv` / `python`）→ ② 借机器上已装 comfy-mcp + comfy-cli 的 Python（一个字节都不装）→ ③ 借机器上的 Python 建 venv（`<根>\storage\ai\mcp\venv`，不污染全局）→ ④ 真没有才下 embeddable 便携版。用了哪一档记在 `state.json`。

- ⚠ **venv 的解释器就在 `Scripts\` 里**：一律 `父目录\Scripts` 会拼出 `venv\Scripts\Scripts`，表现是"明明装好了却报缺 comfy-mcp.exe"。
- **装不上不拦启动**：缺了只是 AI 少一批工具。

两条**通用** PowerShell 教训：

> **⚠ 参数改名**：非高级函数（没写 `[CmdletBinding()]`）收到不存在的命名参数**不报错**，而是丢进 `$args` —— 改名后内部一堆 `-Proxy $X` 会**静默失效**。改名必须同时改调用点，给命令行用的名字留 `[Alias('旧名')]`。
>
> **⚠ 别给变量起名 `$home` / `$input` / `$host` / `$pid` / `$profile` / `$args`**：自动变量**大小写不敏感**，`$home = …` 直接抛只读错误；更可怕的是漏改一处引用 —— 本仓库真写出过 `$javaHome = …`（定义）却仍用 `$home`（引用），于是 `Move-Item $home …` 去搬**整个用户目录**。**写任何 `Move-Item` / `Remove-Item -Recurse` 之前，先确认目标是"算出来的、可验证的"路径。**

## 9. 视频播放依赖一份打过补丁的插件副本

`pubspec.yaml` 里的 `video_player_win` 指向 **`third_party\video_player_win`**（上游 3.2.2 的本地副本），不是 pub.dev：上游的 `GpuSurfaceDescriptor` 少了 `visible_width / visible_height`，而 Flutter 3.4x 在 Windows 上默认用 **Impeller**、拿这两个字段当外部纹理尺寸 —— 结果是**只有声音、画面全黑**（日志里 `Could not create external texture`）。

- 改动点两行，搜 `ComfyHub 补丁` 定位（`windows\video_player_win_plugin.cpp` 的 `initTexture()`）。
- **别"顺手升级"回 pub.dev**，也别在 `flutter pub upgrade` 后不看 `pubspec.lock` —— 一升回去立刻复发。上游修好后再整体换回并删掉这一节。
- 排查顺序：① 控制台有没有 `Could not create external texture`；② 日志里是不是 `Using the Impeller rendering backend`；③ `--no-enable-impeller` 跑一遍对照（Skia 会容忍 0 尺寸，所以它在旧版本里"看起来是好的"）。

## 9.1 WebView 用 webview_all（**不要换回 flutter_inappwebview**）

训练台内嵌 WebUI 用 `webview_all: 1.4.5`（`lib\pages\train_page.dart`）。**2026-10-23 从 `flutter_inappwebview: 6.1.5` 换过来，原因是关窗必弹 `Unknown Hard Error`**：

- 根因在 `flutter_inappwebview_windows 0.6.0` 的 `InAppWebViewManager` **构造函数**（= 插件注册时，哪怕一个 WebView 都没创建）：它 `RoInitialize` 后创建 `DispatcherQueueController` + `Windows.UI.Composition.Compositor`（`graphics_context.cc`），进程收尾时按错误顺序释放 → csrss 报硬错误（事件日志 `Application Popup` ID 26；WER 里模块 `dcomp.dll`、异常码 `0xe0464645`）。上游 [issue #2512](https://github.com/pichillilorenzo/flutter_inappwebview/issues/2512) 关了没修。
- **判据**（别凭手感）：不注册 webview 插件 → 关窗 0 条事件；只注册它 → 关窗必现（当时的逐插件开关 `COMFYHUB_PLUGINS` 已随修复删除；改回来只需在 `FlutterWindow::OnCreate` 里按名字挑一条 `RegisterWithRegistrar`）。
- `webview_all` 是 `webview_flutter` 的跨平台实现（Windows/Android/iOS/macOS/Linux/Web），1.4.1 修过同类"退出即崩"（[#37](https://github.com/abandoft/webview_all/issues/37) / [#38](https://github.com/abandoft/webview_all/pull/38)）；Android 端支持 AGP 9 内建 Kotlin，所以换过来删掉了 Android 的 `third_party\flutter_inappwebview_android` + `dependency_overrides`。⚠ Windows 的 `<experimental/coroutine>` 逃生宏**仍然要留**（1.4.5 的 C++/WinRT 也没躲开 MSVC 14.51 的 C2338），见 `windows/CMakeLists.txt` 的注释。
- **版本钉死**：`pubspec.yaml` 写 `webview_all: 1.4.5`。升级前必须重跑关窗回归 —— 1.4.0 就出现过"应用退出即崩"，这类问题只在**关窗那一秒**出现，`flutter test` 抓不到。
- **关窗回归**（Windows）：`flutter build windows --debug` → 启动 `build\windows\x64\runner\Debug\viewer.exe` → 等窗口出来 → 发 `WM_CLOSE` → 查 `Get-WinEvent -FilterHashtable @{LogName='System'; Id=26; StartTime=...}` **必须为 0 条**（`Application Popup` 事件就是那个弹窗）。⚠ Debug 产物用的后端是源码树 `server-rs\target\{release,debug}\` 那份，不经过 release 装配。
- Android 侧仍按老规矩 `flutter build apk --debug` 过一遍；需要带 `jlink.exe` 的完整 JDK（JBR jcef 没有 jlink，本机 `C:\Program Files\Java\jdk-25` 可以）。
- WebView2 运行时是 Windows 的硬前提（Win10 1809+ / Win11 自带）。加载失败时 `onWebResourceError` 只在**主文档**失败时提示；子资源 404 不该把整页判死。

## 10. AI 工具与 Skills 的接线约定

**功能说明见 [`docs/ai-tools-and-skills.md`](docs/ai-tools-and-skills.md)**（含"DSH 哪些工具抄了、哪些没抄"）。这里只记改动时容易踩的规矩。

- 工具清单**只有一个真源**：`server-rs/src/tools/tools.rs` 的 `catalog`（桌面全量）与 `mobile_catalog`（手机 7 个）。系统提示、`GET /api/ai/tools`、界面都从它派生 —— 加工具就在这里加一项 + `ToolRegistryTest` 补用例，**别在别处再维护一份名单**。
- **出图 / LoRA 验收走 comfy 工作流，不写推理脚本**（2026-10-20 用户口径）：ComfyUI 没起先 `comfy_start`；然后 `comfy_list_workflows` / `comfy_find_workflow` 找本机工作流 → `comfy_load_workflow` → `comfy_check_inputs`（缺输入）/ `comfy_fit_workflow`（缺模型）→ `comfy_submit`。验收 LoRA 只需在工作流里换 `LoraLoader` 的 `lora_name`、prompt 加触发词；**别为了看一眼效果另写 `inference.py` 之类的推理旁路**（那条只给引擎自己的回归用）。
- **读白名单会自动加上本机 ComfyUI 的目录**（`ComfyRoots` 每次现探、不落库）：用户的 ComfyUI 不在项目里，只按 `<根>\comfyui` 判会让 `read_file` 读不了他自己的工作流。**这只放宽读** —— 写仍然只认 `<根>\comfyui`。判定是"认特征文件"（`comfy_dirs::looks_like_comfy`），搜索有界（深度 ≤3、只往名字带 comfy 的分支下钻）。
- **本机 ComfyUI 的工作流文件是"库"之外的第二来源**：`prompts` 库只收"捕获过的运行"与"手动读进来的文件"，所以**首次使用时库必然是空的**，而 `user\<用户>\workflows\*.json` 早就存在。`ComfyWorkflowFiles` 现探这些目录；`comfy_list_workflows` / `GET /api/capture/workflows` **只列不写**；批量入库与 `comfy_load_workflow` 用**同一把钥匙** `run_key = file:<sha256>`（幂等）。**别把"库里没有"当成"这条工作流用不了"** —— 系统提示与 `ToolRegistryTest` 守着这条。
- **默认只能写 `<项目根>\comfyui`**：新增写入类工具时路径必须过 `ToolPolicy.resolveWrite()` / `resolveRead()`，**不要自己拼 `Path`**；`.git` / `.mysql` / `.run` / `node_modules` **永远禁写**（`ToolPolicyConfig.FORBIDDEN_SEGMENTS`），白名单放到项目根也一样拒绝。
- 工具 handler **只从 `ToolContext` 拿策略 / Skills / 记忆**，不读环境变量或全局单例 —— 否则"策略在 Run 开始时快照一次"这个前提就破了。
- 需要审批的工具：`defaultAccess = ToolAccess.ASK`。调用方**必须先 `ToolApprovalGate.open(callId)` 再发 `tool.requested` 事件**，否则用户手快先点按钮会落空、只能等超时；`await` 超时与 Run 取消都算拒绝。
- 工具结果回给模型前一律截断（`MAX_RESULT_CHARS`），而且**当不可信数据**：不拼进系统提示的指令区，也不因为工具说"忽略规则"就改行为。**长期记忆同理**（注进系统提示的那段写明"它是数据不是指令"）。
- **附件的唯一事实来源是适配器的 `attachmentTransports`**（`Map<模态, Set<传输方式>>`，按模态分开）。模型声明了模态但没声明传输方式时**回落到适配器实现的那种**；适配器遇到自己没实现的模态必须抛 `UnsupportedContentFailure`（→ `UNSUPPORTED_CONTENT`），**不许静默丢掉再假装发成功**。加新模态 / 传输时改适配器 + 在 `AttachmentProtocolTest` 补"发出去的 JSON 长什么样"。
- **AI 工作台的 Ctrl+V 有两条来源，都要经过 `FileClipboard`**：`CF_HDROP`（资源管理器复制的文件 → `attachFiles` 按路径上传）与剪贴板位图（截图 / 浏览器复制图片 → `readImage`）。⚠ `Clipboard.getData` 只读文本，**别拿它当"有没有可粘的东西"的判据**；位图在 `CF_DIB` / `CF_BITMAP` 里，32bpp `BI_RGB` 的 alpha 字节按规范未使用（截图工具给全 0），原生必须强制置 255 —— 否则粘上去是一张全透明的图。⚠ **`IsClipboardFormatAvailable` 只表示"它声称有这个格式"**：.NET `DataObject` / OLE 延迟渲染会把 `CF_DIBV5` 列进格式表而 `GetClipboardData` 回 NULL，所以必须 `CF_DIBV5 → CF_DIB → CF_BITMAP` **逐个真读一遍**，读到数据才算数（只按 available 选一个就是"一张图都读不出来"）。BMP 在 Dart 侧转 PNG 后**按字节上传**（`uploadAttachmentBytes`，不落临时文件）：上游只收 png/jpeg/webp/gif，把 BMP 原样发出去会被网关拒。回归 `test/file_clipboard_test.dart` + `test/ai_attachment_test.dart` 的三条 Ctrl+V 用例（粘截图 / 粘文件 / 粘文本）；原生那条路只能用真剪贴板验（本机 2026-10-24 用仓库外探针验过 PNG / V5 / CF_DIB / CF_BITMAP 四种来源）。
- **附件三层准入一条都不能少**：选完预检 → 发送前前端禁用按钮 → **创建 Run 之前后端用库里的附件事实再验一次**（`POST /runs` 里、append 用户消息之前）。失败必须是 `400 UNSUPPORTED_CONTENT` 且**上游请求数为 0**；防线是 `scripts\e2e-ai-tools-test.ps1` 第 9 幕。
- **历史里的图片也属于上下文**：`turnsFromHistory` 会把带 `attachment` 块的图内联，但当前模型 / 协议发不了或超预算（单图 8MB / 合计 20MB，见 `InlineBudget`）时，必须在那一轮正文后附一句"（以下附件未随本次请求发送：…）" —— 既不静默丢弃，也不因为历史里有张图就把请求打成 400。原件在 `storage/ai-attachments`、缩略图在 `storage/ai-thumbs`，**与画廊产物分开存**。
- Skills 正文真源是磁盘（`<storage>\ai\skills` 与 `<项目根>\skills\builtin`），**刻意不做缓存**（"注册完下一次回复就能用、删掉立刻消失"是需求）。
- **装 skill 只有一条路：投放口**。后端启动与 `POST /api/ai/skills/rescan` 都跑 `SkillStore.autoRegister()`：给**没有 frontmatter** 的文件补 `name` / `description`（正文不动），已有 frontmatter 的**一个字节都不许改**。路径真源是后端（`GET /api/ai/skills/roots`），**别在前端拼这个路径**。`description: |` 这类块标量必须能读。frontmatter 非法**不静默忽略**：照样列进界面并带 `validationError`，但**不进系统提示、不能被 `load_skill` 加载**。
- 改系统提示词**必须 bump `SystemPrompt.VERSION`**（= `ai/nsfw_prompt.rs` 的 `PROMPT_VERSION`；Run 里记 `promptVersion`）。⚠ bump 的判据是"**模型看到的规则变没变**"，不是"改的是哪个文件" —— 改 `system_prompt.rs` 的拼装同样要 bump（它进 `run_kit` 的签名，手机因此才拿得到新 kit）。
- **系统提示是前缀缓存的地基，前部不许有"每次请求都变"的东西**。它后面紧跟整段对话历史，任何抖动都会把历史缓存一起作废：一个 Run 之内没事（`render` 每 Run 只调一次），**跨 Run 全废**（用户再发一条就是新毫秒串）。所以那一行只写**本地日期**（`- 当前日期: YYYY-MM-DD`，用 `run_kit::local_date` 取，别用 UTC），且**一个 Run 里只取一次**。头部剩下会抖的几处（有无图片附件、自动允许档、装 / 删 Skill、改记忆或白名单）都是"用户刚做了某动作"才变，频率与用户动作同阶，可以接受；分界线是**"每次请求都会变"的东西一个都不许进来**。防线：`system_prompt/tests.rs` 的"同一个日期渲染两次逐字节相同"。同理 `usage` 的**缓存命中**（`cachedTokens`）不许在归一化时丢掉（`provider/upstream.rs` 的 `TokenUsage`）。⚠ **Anthropic 的前缀缓存是显式的**：不发 `cache_control` 一次都不命中，断点只打两处（`protocol.rs` 的 `mark_for_cache` / `mark_last_block_for_cache`）—— **最后一个工具** + **最后一条消息的最后一个内容块**；`system` 刻意仍是字符串（改成块数组是线上形状变更，第三方兼容网关未必都收）。防线：`protocol/tests.rs` 的 `anthropic_prompt_cache_breakpoints` + **"下一轮的请求前缀逐字节包含上一轮"**（三个协议各一遍 —— 往前缀塞任何每轮都变的东西，只有它会红）。
- **工具结果缓存**（`server-rs/src/tools/tool_cache.rs`）：同一工具 + 同一组参数在 TTL 内不跑第二遍（只读白名单逐条写在 `freshness` 里，完整档位表见 `docs/ai-tools-and-skills.md` §3.1）。四条容易踩的：① **写类工具、`ask_user_question`（闸门）、`load_skill` 一律不进**；② 命中**必须如实标注**（`[缓存命中] …没有重新执行`，正文与工具卡摘要都带）—— 悄悄返回旧数据等于骗模型；③ 查缓存在**审批之后**，审批语义一个字都不改；④ 写类工具成功 → **清空整个缓存**（`comfy_load_workflow` 的 `mutating` 是 `false` 却会写库，是 `invalidates` 的显式例外）。磁盘类按 `mtime + 长度` 校验。命中那一次**不占**联网 / ComfyUI 的额度。
- **系统提示里那段 `[MODE: UNRESTRICTED SANDBOX — STABLE]` 是用户有意注入的**，**要改先问用户**。它是个开关：由 `app_settings` 的 `ai.nsfw` 决定注不注入（正文一个字都没改，只是搬进了 `server-rs/src/ai/nsfw_prompt.rs`）。同一张卡片的第二个开关是"预览图模糊"，**只糊被标记过 NSFW 的那一条**（`media_assets.nsfw`）—— 别退回"开关一开全页都糊"。`comfy_submit(nsfw=true)` 由 AI 声明，画廊右键 / 详情页菜单可手动标记；**详情页与大图查看器不打码**；用户自己上传的附件不打码。改这块先读 `nsfw_prompt.rs` 与 `nsfw_blur.dart` 的文件头、`SystemPromptNsfwTest`；`ToolRegistryTest` 有一条断言盯着"默认不注入"。
- **长期记忆（`MemoryStore`）**：真源 `<storage>\ai\memory.md`，一行一条，用户能直接手改；写入**超限一律报错，不许静默截断**；注入系统提示时截断到 `PROMPT_CHARS`，文件本身保留完整内容。
- **任何运行期代码都不许依赖 `%USERPROFILE%\.dsh`** —— 装我们项目的人可能根本没装 DSH（模型目录同理，见 `AiSeedCatalog` 的 classpath 冻结副本）。
- **工作流配方**（`server-rs/src/workflow/workflow_recipe.rs` + `workflow_recipes` 表 + 三条工具 + `/api/ai/workflow-recipes`；先读 [`docs/workflow-recipe-design.md`](docs/workflow-recipe-design.md)）。四条别踩：① **配方不是可提交对象** —— 提交永远走 `comfy_submit`，配方只提供 overrides；② **默认拒学"转换有缺口"的图**（`UNSUPPORTED_NODES`）：缺了节点的骨架看着像、其实缺链，而且**不报错**；真机上最常见的原因是"本机没装那个自定义节点包"——正确动作是让用户启停 / 换实例，**不是改他的工作流**；③ **绑定只报不改**：只有 `exact` 能自动写键，其余四档各有处置（设计 §4）；④ 学习与绑定**只有一份实现**（`learn_and_save` / `bind_and_report`，工具与端点共用）；表**不进同步白名单**。端到端 `scripts\e2e-recipe-test.ps1`。
- **工作流蓝图 + 接线体检**（`server-rs/src/workflow/workflow_blueprint.rs` + `lib/widgets/workflow_blueprint_{page,painter}.dart` + `lib/core/workflow_blueprint.dart` + 工具 `comfy_diagnose_workflow`；先读 [`docs/workflow-blueprint-design.md`](docs/workflow-blueprint-design.md)）。六条别踩：① **判据只有后端那一份**（要 ComfyUI 的 `/object_info` 才判得出必填与类型），前端**只画不算**、AI 工具也调同一个 `build()`，别在别处再写一套解析；② **"缺口"只算必填** —— `Converted.required_open_inputs` 是配方 / 蓝图 / 提交三处共用的唯一判据，`open_inputs` 把可选插口也列进去了（拿它当缺口会让整份工作流学不下来）；③ **"哪些节点不参与出图"要按转换之后的图算** —— 界面图里广播节点的下游看不出连线，直接算会把真正干活的那一串全误报；④ **编辑只改原文里那一个字段**（按节点 id 定位）：前端 only 节点 / 坐标 / 分组因此天然不丢，改完**必须**重新序列化 `_raw`（忘了这一步的表现是"界面改了、复检与复制还是旧的"，一个错都不报）；⑤ **保存的落点由 `ToolPolicy::resolve_write` 决定**（`<根>\comfyui\workflows\`），别为"写进用户的 ComfyUI 目录"另开一条写路径；⑥ 加工具按 R12 走（`desktop_tools` 清单 + `IMPLEMENTED` + `execute` 分支 + **bump `PROMPT_VERSION`**：工具清单是从 catalog 派生的，签名会变）。
- **2D 布局模板**（`server-rs/src/ai/layout_proposal.rs` + `lib/models/layout_embed.dart` + `lib/widgets/layout_templates_card.dart`；先读 [`docs/layout-workspace-design.md`](docs/layout-workspace-design.md)）。五条别踩：① **工作台是主入口，画布只是编辑面** —— 改完「提交回工作台」= 发一条普通用户消息（`LayoutResubmit`），别另建会话 / 表 / 端点；② 后端必须**夹边、补 id、清坏关系并逐条记账**（`issues`），别把模型的原样坐标直接交给 `LayoutSpec.fromJson`（它刻意宽容，越界框会画到画布外）；③ `layoutTemplates` **必须显式落进 `tool_result` 块**（`executor.rs` 的 `push_tool_result` 是白名单），漏了的表现是"当轮卡片在、重开 App 整张卡消失"；④ 改系统提示**第 25 条**或工具清单必须 **bump `PROMPT_VERSION`**（当前 v58），并同步 `run_kit/tests.rs` 与 `ai_run_kit.rs` 的版本 / 清单断言；⑤ **3D 路线已搁置**（2026-11-03），`docs/layout-3d-design.md` 只作评估，别照着它开工。
- **有三件事只有前端会做**（结论来自 2026-10-17 真机 `/history` 里前端**真正发出去的** API 图；当时用来复现的两份 krea2 模板已随 2026-10-28「仓库不再随包带模板」删除，判据仍然成立）：① `Anything Everywhere` 在本机被注册成"零输出桩"，`workflow_frontend::emulate` 要照样改写（**只放宽广播型**，显示型不许动 —— 丢掉 `PreviewImage` 会把"只想看一眼"变成"自动补 `SaveImage` 落盘"）；② `Fast Bypasser (rgthree)` 是 1 进 1 出的**直通**（`FRONTEND_PASSTHROUGH`），不处理就整份进不来；③ `TextGenerate.sampling_mode` 是动态下拉框，子项名字带点。**仓库不再带模板工作流**：模板一律用各模型在 ComfyUI 模板库里的标准工作流（或用户自己存的那份）；`comfy_load_workflow` 默认开 `emulateFrontend=true` 处理这三件事，防线是 `workflow_frontend` / `workflow_convert` 的构造型用例（原来的活体回归 `krea2_templates.rs` 已删）。

### 10.1 提交任务与实时进度

- **`comfy_submit` 只能提交 API 格式节点图**，那份图在 `capture_runs.raw` 里（`comfy::capture::api_graph_of`），**不是** `prompts.workflow_json`（那是界面格式，`widgets_values` 只有位置没有参数名，转出来一定是错的）。没有 API 图的老数据报 `NO_API_GRAPH`，别猜着转。
- 参数覆盖走 `workflow::workflow_submit::WorkflowEdit::apply_overrides`：键是 `节点id.输入名`，**按原值类型转换**，字段不存在 / 是连线数组一律抛错，**不许静默忽略**。
- 提交后的产物必须走 `comfy::capture_ingest` 入库（与轮询同一条路，幂等靠 `prompt_id` + 文件 SHA-256）；绕开它自己写库就会出现"AI 出的图没进画廊"或者重复入库。
- 等待有上限（`DEFAULT_WAIT_SECONDS` / `MAX_WAIT_SECONDS`），超时记 `timeout` 并如实说明，**不许假装完成，也不许假装失败**。
- **界面上的产物入口只认后端给的 `mediaIds`**（`tool_result.jsonPayload.mediaIds` / `tool.completed.result.mediaIds`）：模型在正文里说"我生成了 xx"不算数。详情页 / 画廊入口卡跳转用 `MediaDetailPage(mediaId:)`；别在前端自己拼媒体 URL（`thumbUrlFor` / `fileUrlFor` 走后端算好的路径）。
- **会话标题是"模型在正文最前面输出 `[标题]…[/标题]`，后端摘掉"**（`ai/title_strip.rs` 的 `TitleStripper`）。三条纪律：**绝不吞内容**（标记不闭合就把缓冲当正文放出去）、**绝不重复内容**、**摘到标题之后还要继续盯回声**（后面的轮次会再吐一遍，同样摘掉，但**不许改**已定下的会话名）。`ConversationTitleTest` 盯这三条。另外那条"起标题"指令**只能按轮带**（`runRound(askTitle=…)`），**绝不能塞进常驻 `turns`**；**位置必须在系统提示之后、历史之前（第 2 条），不能挂末尾** —— 挂末尾会挡住"最后一条消息是工具结果"这个事实（模型靠它收口、假网关靠它分流），`e2e-submit-test.ps1` 会一直重复调 `comfy_submit` 最后卡在审批闸门上。**改了 `HarnessRunner` 的上下文拼装，三条 e2e 全跑一遍。**
- **ComfyUI 位置解析有两份实现**（`comfy_dirs.rs` 与 `scripts\comfy-path.ps1`），判据必须一致（源码形态 = `main.py` + `comfy/`；任何形态 = `output/` + `models/` 或 `input/`），改一边必须改另一边。探测**只读**，写配置必须由用户点「使用这个目录」（`POST /api/capture/locate/apply`）。
- **`capture_runs` 的 `empty` / `error` 是可重试状态**（`capture_ingest::can_reclaim`）：`/history` 先出现记录、产物文件晚到是常态，把 `empty` 当终态就会出现"AI 说生成了、画廊里却没有"。只有 `success` 才是终态；改这段前先看 `CaptureRunRetryTest`。
- **失败 / 取消时"已经说出去的话"一个字都不能丢**。三条纪律：① **`message.text` 里有的内容，parts 里也必须有** —— 前端 `_orderedBody` 在 parts 非空时**只渲染 parts**，只写正文不落块 = 用户看不见；被 429 / 断流打断的那一轮必须由失败分支补落（`ai/executor.rs` 的失败尾巴与正文落块是同一条路）；② **标题摘取缓冲**（`TitleStripper` 压住的开头，最多 ~200 字符）失败 / 取消时也要 `flush()` 放出来；③ **"就地重置"只能用在空壳上**：`POST /runs` 重试的判据是"正文空 **且** `assistant_has_parts` 为假"（`ai/run_lifecycle.rs`），有内容的失败回复**原样留着**、重试另起一条助手气泡。**别退回"只要 failed 就重置"** —— 那会把工具卡和图片入口卡一起删掉。防线：`e2e-ai-tools-test.ps1` 第 13 幕 + `test/ai_retry_keep_content_test.dart`。写假网关场景时先给会话定好标题，把"起标题"链路从断言里摘出去。
- 改完这块跑两条端到端（都不出网、不花钱）：`pwsh -File scripts\e2e-submit-test.ps1`（6 幕）、`pwsh -File scripts\e2e-capture-test.ps1`。它们会临时改自动捕获配置、建 loopback Provider，跑完自己还原清理；`-KeepData` 可留数据。**别在跑完前 Ctrl+C**：清理在 `finally` 里，中断会留下 `e2e-submit-*` 记录，把下一次捕获 e2e 的 SHA-256 判重弄乱（表现为"新导入 0 个"）。`e2e-submit-test.ps1` 会自己把权限档临时钉成 `ask` 再还原。
- **事件流是"批写 + 只从库里读"，所以必须有人 flush**：`RunEventBus::emit` 攒够 `EVENT_BATCH = 200` 条才开事务落库，而 `SseStream::poll_next` 是按 `?after=` **从库里读**的。守住两条：① `poll_next` 每轮读库**之前先 `bus.flush`**（空批不开事务）—— 去掉就是"开头几条事件要等下一个闸门类事件才现形"；② **闸门类事件**（等审批 / 等回答）发完**立刻 flush**。防线 `run_lifecycle.rs::sse_sees_events_without_waiting_for_a_full_batch`。⚠ **测试侧同样会被绊到**：`wait_for_run` 只等"Run 状态变终态"，而事件**可能还没落库** —— 读 `ai_run_events` 一律走 `common.rs` 的 `event_types` / `event_payload`（两者都先 `bus.flush`），**别在用例里自己 `query_row(..).unwrap()`**（对"还没落库"与"落了两条"都会 panic）。
- **环节标记与关键步骤折叠**：模型用 `[步骤]环节名[/步骤]` 声明环节，六个名字的真源是 `server-rs/src/ai/step_mark.rs` 的 `PHASES`，**系统提示第 21 条与它必须逐字一致**（不一致的表现是"名字被摘掉、界面上什么都不出现"，且不报错）。① 摘取**绝不吞内容**（不闭合 / 撑爆 / flush 三条兜底都要把原文放出去）；② 枚举外的名字**摘掉但不记环节**；③ 有序块类型是 `step`、实时事件是 `step.started`（契约文档 §2 一起改）；④ 界面按"**连续**的工具卡"成组，正文与工具卡的先后**永远等于到达的先后**，同一环节夹了正文就另开一组标「续」；⑤ 组里有等批准 / 失败 / 被拒的调用时**不许折叠**。防线：`step_mark.rs` 6 例 + `test/ai_tools_ui_test.dart` 的 (e)(f)(g)。

---

## 11. ⚠ 目标架构

**一句话**：`SQLite（单文件）+ 桌面 Rust（单文件 exe）+ 移动 Kotlin harness + Flutter（两端）`。**当前状态、14 条红线、验收判据、回归防线归属**全在 [`docs/implementation-master-plan.md`](docs/implementation-master-plan.md)（**动手之前先读它**）。

1. **桌面后端只改 `server-rs/`（Rust）**：旧 Kotlin 已不在仓库里，要参照旧行为从 git 历史取。
2. **不要引入 KMP**（`kotlin.multiplatform`）—— 与 AGP 9 内建 Kotlin 这个刻意设计正面冲突（理由见 `docs/mobile-harness-design.md` §2.1）。
3. **移动端 harness 用 Android Kotlin**（Flutter 通过 platform channel 调用），不是 Dart、不是 Rust。**手机端不许硬编码系统提示 / 工具描述 / 安全文案** —— 由桌面 `/api/ai/run-kit` 版本化下发（R13）。
4. **发布包**：`mysql/`（304MB）、`jre/`（~40MB）、Kotlin jar（~20MB）都已去掉，装配实测约 **14MB**。改 `packaging\manifest.json` 与 §7 的布局前**先看总纲**；`runtimeRequirements` 的 `required` 决定"缺了算不算问题"。

---

## 12. ⚠ 写不进工作区外：真因是完整性标签（Low IL）+ `min(父进程, 映像)`

`comfy_start` 报 `COMFY_WRITE_BLOCKED`、`comfy_use_media` 报 `COMFY_INPUT_FAILED … os error 5`、MCP 工具报 `UnexpectedToolError`、`cargo test` 一批 `PermissionDenied` —— **同一批症状、同一个根因**。

**机理**：① 代码沙箱给**工作区目录树**打 `Mandatory Label\Low Mandatory Level:(OI)(CI)(NW)`；② 在工作区里构建出来的 exe（`comfyhub-server-rs.exe`、`viewer.exe`、测试二进制…）**继承了 Low 标签**；③ Windows 创建进程取 **`min(父进程令牌, 映像标签)`**；④ **Low 写不了 Medium 对象**（工作区外默认无标签 = Medium）⇒ `拒绝访问 (os error 5)`。

两条推论：**只把 exe 的标签清回 Medium（位置不动）→ 后端立刻哪儿都能写**；**链路上任何一环是 Low，子进程照样掉回 Low** —— 所以 `viewer.exe` 也是 Low 时，App 自己就是 Low，它起的后端一定是 Low，**连"修标签"这一步都做不动**。

（已排除的怀疑对象：目录 ACL、路径配错、用户身份、Defender、从哪个会话 / 终端起、spawn 档位 —— 决定权只在**映像标签**。）

**三条处方**：

1. **构建时归一标签**：`windows\cmake\bundle_backend.cmake` 在 release 装配末尾对 `viewer.exe` 与 `backend\comfyhub-server-rs.exe` 各跑一次 `icacls <exe> /setintegritylevel Medium`（失败只警告、不中断构建）。
2. **把整套 ComfyHub 放到工作区外**（终解）：`pwsh -File scripts\pack-release.ps1 -OutDir D:\dist\ComfyHub`，然后运行 `D:\dist\ComfyHub\viewer.exe`。那边的文件没有 Low 标签可继承 —— 既不受限，内容也仍全在它自己的文件夹里（§13）。
3. **运行时就地修**：App 起后端前 best-effort 修一次（`backend_launcher.dart` 的 `_normalizeIntegrity`），`scripts\backend.ps1 start` 也会先修（`Repair-LaunchImages`）。这条**只能救"App 是 Medium、只有后端 exe 还带 Low 标签"那一半**。

⚠ 两条边界：**"换个终端重起"没用**（映像标签没变）；**别建议往 `%LOCALAPPDATA%` / `%TEMP%` 拷后端**（违反 §13，而且只是碰巧绕开标签）。

**四条纪律**：

1. **闸门留着**（`comfy::comfy_launch::ensure_writable`）：白起一个 100% CPU 卡死的实例，比如实拒绝糟得多。但**文案必须说对** —— 先讲真因，再给上面三条处方；**不许出现"权限 / ACL / 换个终端 / 双击 ComfyUI Desktop"**（`infra::confinement::remedy_text()`、`comfy_launch`、`image_edit` 三处同一纪律，用例 `修法文案指向发布包而不是让用户改_acl` 盯着）。
2. **别把探针的 `Err` 吞成 `bool`**（`infra::confinement::probe_dir` 返回 `io::Result` 就是为此）：只报"哪个目录"不报"报什么错"，就会重演"现象对、处方错"。用例里也不许拿"这台机器哪个目录不可写"当判据 —— 用**同名目录占位**造确定性的 `os error 5`。
3. **看得见**（`server-rs\src\infra\confinement.rs`）：后端启动自查 → 受限才写 `.run\confinement.json`、**正常就删**（免得过期标记一直亮着），标记带 `pid`。`GET /api/system/confinement` 把结论**连判据一起**交给界面（`serverImage` / `imageInsideProjectRoot` / `blockedPaths` / `remedy`），受限时 App 顶部常驻横幅（`lib\widgets\confinement_banner.dart`）。⚠ 字段名只能说"**落在运行根目录里**"，别说"在工作区内"（发布包里根目录就是包目录，那里 `restricted=false`）。**界面上看不见的现象，用户和模型就只能靠猜。**
4. **写 input 的地方必须共用同一个入口**（`comfy::image_edit::write_or_defer`）：投放原图 / 遮罩 / 摆好的人物图三处曾经只有两处有 HTTP 退路，于是"摆好的那份没落地"。直写被拒一律改让 **ComfyUI 自己写**（`/upload/image`），并按它回的名字回填结果字段。逐条清点见 [`docs/permission-audit.md`](docs/permission-audit.md)。**"写哪个 input"也只有一个真源**：起实例的 argv 由 `comfy::comfy_launch::start` 落档（`.run\comfyui-<port>.launch.json`），投放走 `comfy::image_edit::target_input_dir` —— **先问实例**（`/object_info/LoadImage` 的名单 × 候选目录），问不到才按"留档 → 配置 output 兄弟 → Desktop `settings.json` 的 `inputDir` → 盘探"回落。⚠ 别拿**全名**判"调用方写过哪些参数"：`--input` 是 `--input-directory` 的 argparse 缩写、同一个 `dest`、**后写的赢**（用 `tools::mcp::specified_flag`，并认 `--flag=value`）。两个环境变量名也别漂：`COMFYHUB_COMFY_OUTPUT`（脚本与老版本）与 `COMFYHUB_COMFY_OUTPUT_DIR` 都要认（`comfy::capture::comfy_output_env`）。

**连带影响**：受限会话里 `cargo test` 会红一片（测试二进制就在工作区里），所以用例只许用 `testpaths::scratch()`（§1）。

## 13. ⚠ 内容一律存在自身文件夹内

> **ComfyHub 自己产生的内容，不许散到 `%APPDATA%` / `%LOCALAPPDATA%` / `%TEMP%`。**

它同时是落盘布局的总原则，也是 §12 那条不变式的解法：把整套 ComfyHub 放到工作区外（发布包），既不受限、布局又与源码树一致。

| 内容 | 落点 | 一次性导入 |
| --- | --- | --- |
| 数据库 | `<根>\.db\comfyhub.db` | 无 |
| 产物 / 缩略图 / 附件 | `<根>\storage\` | 无 |
| 偏好 | `<根>\storage\app-support` | `%APPDATA%\…\shared_preferences.json` |
| MCP 运行时（便携 python + venv） | `<根>\storage\ai\mcp` | 无 |
| comfy-cli 凭据 / 缓存 | `<根>\storage\ai\mcp\comfy-cli-home` | `%LOCALAPPDATA%\comfy-cli\{secrets.json,config.ini}` |
| 临时文件 / DevFS | `<根>\.run\tmp` | 无 |

1. **别再用"探系统位置 → 写不进才退回"这个模式**：判据不可靠（能不能写取决于进程映像位置，§12），方向本身也错 —— 静默换地方正是"哪份才是真的要看机器上装过什么"的来源。真写不进去就**如实报错**，让用户把包 / 项目放到可写的地方。
2. **导入是只读复制、且只做一次**（目标已存在就不动，否则会用旧文件盖掉用户新存的值）。受限进程**读**项目外是允许的，所以 errno 5 的现场导入也做得成。
3. **代价要如实说**：这些不再跨安装 / 跨机器共享 —— 但 db 与 storage 本来就在各自文件夹里，这与"发布包是便携的"一致，别当回归。

改落盘位置时，`server-rs\src\tools\mcp.rs` 与 `scripts\mcp-runtime.ps1` **必须一起改**（`discover_runtime` 与 `Get-McpRuntimeDir` 是同一真源的两份实现）。判据：装完发布包后从包内启动一次 App（或 `scripts\backend.ps1 start`），比对 `%APPDATA%\com.example`、`%LOCALAPPDATA%\comfy-cli`、`%LOCALAPPDATA%\ComfyHub` 的文件数 —— **跑前后都不变**才算过。

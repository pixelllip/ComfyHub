# AGENTS.md

给在这个仓库里干活的 AI / 自动化协作者看的注意事项。**功能说明、接口清单、排错表都在 [README.md](README.md)**，这里只记"怎么做才不踩坑"。

---

## 1. 只改前端时：用 debug 版 + 热重载，别每次完整构建

- **只动 `lib/` 下的 Dart 代码**（页面 / 组件 / 主题 / 状态）→ 用 debug 版，跑起来后按 `r` 热重载，1~2 秒见效：

  ```powershell
  pwsh -File scripts\dev-app.ps1        # ① 后端二进制缺/旧就先 cargo build --release ② flutter run -d windows --debug
  ```

  **后端由 App 自己起**（`lib\core\backend_launcher.dart` 直接执行 exe，2026-09-30 起不再经过 pwsh）——
  `dev-app.ps1` 只负责"保证那份二进制存在且是最新的"，起停不归它管。
  想自己来也可以直接 `flutter run -d windows --debug`（记得先 `$env:PUB_HOSTED_URL='https://pub.dev'`，
  并且 `server-rs\target\{release,debug}\` 里得先有一份编好的后端）。

- **别**为了看一个前端改动去跑 `flutter build windows --release` ——
  那是 Release 完整构建，几分钟起步（改 `windows/` 原生代码、改 `pubspec.yaml` 依赖、
  要出正式产物时才需要）。它也**不需要**任何 `.ps1`：构建末段 CMake 会自己
  `cargo build --release` 并把后端装进 `backend\`。
- 改完前端至少过一遍：`flutter analyze` + `flutter test`（`PUB_HOSTED_URL=https://pub.dev`）。
  详情页大图、工作流弹窗这类交互改动，回归用例分别在 `test\zoomable_image_test.dart` 和
  `test\workflow_viewer_test.dart`，别只跑 `widget_test.dart`。
- **测试不是越多越好，但"能不能抓住回归"是唯一标准**：同一个事实写两遍（e2e 里
  "收到 run.completed" 每幕都断言一次、"parts 里有 tool_call" 又与后面"落库 parts 有序"重叠）、
  为了用掉 import 而写的占位用例、同一行为的两种写法 —— 这类**该删就删**（用户提过"减少一些
  已经通过、不太重要的测试项"）。判断方法只有一句：**删了以后出错还能不能被测试抓住**。
  真正的防线（零上游请求、审批顺序、越界写被拒、内联预算、滚动条与懒构建…）一条都不能删。
  本机实测的耗时（别凭感觉说"测试太慢"）：`flutter test` 407 例 ≈ 25s、
  `cargo test`（后端 lib 613 例 + 集成）≈ 30s（热构建；首跑要编译）、
  `e2e-ai-tools-test.ps1` 111 项 ≈ 5~15s、`e2e-capture-test.ps1` / `e2e-submit-test.ps1` 各 ≈ 20~35s。
- **Rust 用例要落盘，只许用 `crate::testpaths::scratch()`**（`tests/` 里是
  `comfyhub_server_rs::testpaths::scratch()`），落点是 `<crate>\target\.scratch\`。
  **别写 `std::env::temp_dir()`** —— 它在工作区外，而工作区里构建出来的测试二进制带着
  Low 完整性标签（见**第 12 节**），受限会话里必然 `PermissionDenied (os error 5)`，
  与被测代码无关（2026-09-29 那次迁移就是把 47 处 `temp_dir()` 换掉，现在全绿）。
  同理，**要造"写不进去"的场景就用"在目标文件名上放一个同名目录"**，别去改 ACL ——
  Windows 上那必然回 `ERROR_ACCESS_DENIED`，与受限现场同一个错码，且不依赖本机 ACL。
- 产物目录：debug 在 `build\windows\...\runner\Debug\`、Release 在 `...\Release\`。
  **debug 产物里没有 `backend\`**（只有 release 构建才装它），所以 debug 版用的是
  `server-rs\target\{release,debug}\` 里那份 —— 改了 Rust 只要 `cargo build`，不用重建 App。
- **"我改了但界面没变"先查构建新鲜度，别急着怀疑代码**：App 是直接双击 / `Start-Process`
  拉起旧产物时，跑的还是上一次编的 Dart（Release 看 `data\app.so`、debug 看
  `data\flutter_assets\kernel_blob.bin`）。2026-09-17 "画廊/提示词里的视频预览图不显示"
  就是这么丢的：源码 20:11 改好，用户手里跑的是 18:50 编的内核 —— 只跑 `flutter test`
  永远发现不了（测试跑的是源码，用户看的是旧二进制）。所以**界面类修复必须
  `flutter build windows --debug`（或 `dev-app.ps1`）重启一次，再截图确认**。
  后端同理：`scripts\backend.ps1 build` 会在源码变新时重编，`status` 里能看到跑的是哪一份。

## 2. 本地服务必须"静默"启动（不弹命令行窗口）—— 现在由 Dart 保证

后端由 App 自己启动（**数据库是 SQLite 文件，P2 起不用起数据库服务**），
**不允许出现任何 cmd / 控制台窗口**。2026-09-30 之后的结论很干脆：

1. **不需要任何技巧**：Dart 的 `Process.start`（attached 模式）在 Windows 上**自带
   `CREATE_NO_WINDOW`** —— 这正是它源码注释里写的那个场景（"for gui applications
   (flutter on windows) console might be absent, will create by CreateProcessW"）。
   所以 `silent-process.ps1` 那套（WMI `Win32_Process.Create` + `ShowWindow=0` /
   `wscript` + `.vbs` / `direct-hidden` 三档）**连同那个脚本一起删了**，
   `COMFYHUB_SPAWN_MODE` 也不再存在。
2. **"谁起的谁关"由进程句柄 + stdin 管道保证**：
   * 正常关窗口 → `BackendLauncher.releaseOnExit()` 直接 `kill` 自己起的那个进程；
   * 被任务管理器 / 崩溃强杀 → 后端启动时带的 `--watch-stdin` 生效：stdin 写端只有 App 持有，
     App 一死管道即关，后端读到 EOF **优雅退出**（取代了 `watch-owner.ps1` 守护进程）。
   回归：`server-rs/tests/watch_stdin_test.rs`（丢写端 → 必须自己退；管道活着 → 不许退）。
3. **脚本那条路（`scripts\backend.ps1 start`）用 `Start-Process -WindowStyle Hidden`**：
   它是给 e2e / dev 用的旁路，**不带 `--watch-stdin`**（Start-Process 给的不是管道，
   带了后端会当场退出）。那条路**不脱离进程树**——这是刻意的：脚本活着它就活着，
   脚本的 `finally` 负责收尾。
4. **验证"没弹窗口"要看枚举窗口，不能看 `MainWindowHandle`**：控制台窗口属于 `conhost.exe`，
   `Get-Process cmd | Select MainWindowHandle` 永远是 0，会给你假绿灯。
   正确做法是 `EnumWindows` + `IsWindowVisible` 枚举可见顶层窗口，并以 ~50ms 轮询。
   （`check-silent-start.ps1` 已随新链路删除；要复测就照上面两句现写一个盯屏脚本 ——
   注意**别用 `Start-Process` 起后端来做这个测试**，它本来就是"不保证无窗口"的那条路。）
## 3. 首页 / 导航顺序

`lib/app.dart` 的 `HomeShell`：**默认落在「AI 工作台」页**（AIH-001），
顺序是 `AI 工作台 → 画廊 → 提示词 → 标签 → 设置`。
`_destinations`（图标 / 文案）和 `pages`（页面）两个列表**必须同序**，`_index` 同时索引它们。
改完跑一遍 `flutter test test\home_nav_test.dart`（断言落地页和顺序）；
`test\localization_test.dart` 也用首页做锚点，改动落地页要一起改。

> 历史约定是"默认落在画廊"，2026-09-15 按需求文档 `docs/ai-home-requirements-v0.1.xlsx`
> 的 DEC-001 改为 AI 工作台；README 第 2 节、`lib/pages/ai_home_page.dart` 同步。

## 4. 本机环境的硬约束（换机器要重新确认）

| 约束 | 说明 |
| --- | --- |
| JDK | **21~23**，但**只有回退旧 Kotlin 后端 / 跑 Gradle（含 `android/`）时才要**；Rust 后端不需要 JVM（发布包也不带 JRE）。要指定用 `COMFYHUB_JDK_HOME` |
| MySQL | **已退役（2026-09-29）**：数据库自 P2 起是 SQLite 单文件 `<根>\.db\comfyhub.db`，`mysql.ps1` 与老库搬迁都不存在了；老机器上残留的 `D:\tools\mysql\`、`<项目>\.mysql\` 只是历史目录，不再被读取 |
| pub 源 | 必须 `https://pub.dev`（国内镜像对个别包返回 424） |
| 脚本 | 一律 `pwsh`（PowerShell 7）。**App 与后端启动已经不需要 pwsh**（App 直接执行 exe），脚本只剩构建 / 测试 / 可选能力（`video-poster.ps1`）在用 |
| 端口 | 后端**自动挑**（`--port 0`，真实端口在后端自己写的 `.run\api.json` 里）、ComfyUI `8188`（原 MySQL `3307` 已随 2026-09-29 MySQL 退役） |
| Android 构建 | `android/` 走 AGP **9.4.0** + Gradle **9.6.0** + **AGP 内建 Kotlin**（`android.builtInKotlin=true`，`org.jetbrains.kotlin.android` 只钉版本、不 apply）；动 `android/` 之前先读 `docs/android-agp9-builtin-kotlin-migration.md` |
| Gradle 代理 | `~/.gradle/gradle.properties` 配了本机代理 `127.0.0.1:7890`：`dl.google.com` / `repo.maven.apache.org` **必须绕过代理**（`systemProp.http.nonProxyHosts`），而 `github.com` 直连不通、**必须走代理** |
| 后端出网代理 | **JVM 默认无视 Windows 系统代理**（一律 DIRECT）：本机 `civitai.com` 这类目标直连不通、只有 `127.0.0.1:7890` 通，所以 `server/build.gradle.kts` 的 `applicationDefaultJvmArgs` 带着 `-Djava.net.useSystemProxies=true`，出网客户端再统一挂 `com.comfyhub.ai.OutboundProxy` 的选择器（**公网走代理、本机/内网强制直连**——打开系统代理后 Java 会把 `127.0.0.1:8188` 也交给代理，会打死 ComfyUI）。自己用 `java -jar` 起后端时要手动补这个参数，或设 `COMFYHUB_HTTPS_PROXY` |
| Kotlin 增量编译 | 本机 pub 缓存在 C:、工程在 D:，跨盘会炸（`this and base files have different roots`），所以 `android/gradle.properties` 里关掉了 `kotlin.incremental` |

> **踩过的具体坑（50+ 条：现象 → 原因 → 处理）统一记在 [`docs/pitfalls.md`](docs/pitfalls.md)**。
> 它原来是 README 第 12 节，2026-09-17 按用户要求单独拆出来（表太长，把 README 的"怎么用"挤没了）。
> 撞到"明明改了却没生效""脚本手敲行、跑起来不行"这类问题，先去那张表里搜现象。

## 5. 改脚本时的约定

- **App 的启动逻辑只有一份，在 Dart 里**（`lib\core\backend_launcher.dart` + `backend_paths.dart`）：
  起停后端**不再经过任何脚本**，所以别在 `.ps1` 里另写一份"怎么起后端"——
  2026-09-30 之前那套 `comfyhub.ps1 → server.ps1 → silent-process.ps1` 就是这么一层层堆起来的，已整体删除。
- **只留构建脚本与必要脚本**：新增脚本前先问一句"这件事能不能不进 `scripts\`"。
  真要加就放进 `scripts\`、用 `pwsh -File scripts\xxx.ps1 <动作>` 的形式，并在 README「脚本速查」里登记。
- **`scripts\backend.ps1` 是唯一允许碰后端的脚本**（给 e2e / dev 用），它的候选顺序是
  `lib\core\backend_paths.dart` 的镜像 —— 改一边必须改另一边（第 7 节的纪律）。
- PowerShell 坑：`& script.ps1` 的 **stdout 会成为表达式的返回值**，
  在 `$ok = & other.ps1 …` 这种地方必须 `| Out-Null`，否则返回值被输出数组污染、判断永远为真。

## 6. 改界面时的两条硬约定

- **列表一律用 `lib/widgets/adaptive_layout.dart` 里的多列组件**，不要自己写单列 `ListView`：
  提示词 / 标签这种高度接近的用 `AdaptiveColumnList`，设置页这种卡片高度差几倍的用
  `AdaptiveColumns`。规则是**列数 = 可用宽度 / 550**（最多 4 列，窄了退回单列），
  对应回归用例 `test/adaptive_layout_test.dart`。
- **界面语言只能有一种**：App 自己的文案本来就是中文，但文本框选择菜单（复制 / 全选）、
  返回按钮 tooltip 这类**系统文案来自 `MaterialLocalizations`** ——
  `lib/app.dart` 里必须留着 `locale: zh_CN` + `flutter_localizations` 的三个 delegate，
  少一个就会在中文界面里冒出英文的 "Copy / Select all"（用例 `test/localization_test.dart`）。
- 弹出菜单统一用 `AppMenuButton`（按钮下拉）/ `showAppContextMenu`（右键），页面根部挂一个
  `ContextMenuScope`（`lib/widgets/app_menu.dart` + `lib/widgets/context_menu.dart`，
  `common.dart` 里转出）。**别改回 `PopupMenuButton` / `showMenu`**：它们铺的 `ModalBarrier`
  会把整页的滚轮和拖动全吃掉（用户 bug ②，上游 flutter/flutter#90223 至今未修），
  回归用例 `test/menu_scroll_test.dart`。传 `context` 时要用**在 `ContextMenuScope` 里面**的那一个
  （页面自己的 `State.context` 在它外面）。
- 开关行别直接用 `SwitchListTile(contentPadding: EdgeInsets.zero)` 贴卡片边缘：
  用 `settings_page.dart` 里的 `_SwitchRow`（自带内边距 + Material 底色，
  底色不能用 `Container` 的 decoration，否则会盖掉水波纹并触发断言）。
- **长列表必须懒构建，别用 `children: [for (...) ...]`**：模型卡片（每张 6 个 FilterChip +
  一个下拉框）、聊天气泡这类"一屏装不下"的列表一律 `ListView.builder`；流式刷新（聊天逐字回包）
  时每项还要包 `RepaintBoundary`。这两条就是"滑动长列表卡顿"的解药，改列表时别退回去。
- `ListView.builder` 的头部是**固定几条**（`header` 列表）；要按状态拼的用 `switch`/`if` 明确分支，
  别把 `children` 拼回去——一旦拼回去，懒构建就失效了。
- **高矮差别大的长列表，用「固定行高 + 编辑弹窗」，别让每行自己撑高**：懒构建的
  `ListView`/`SliverList` 只能用"已布局子项的平均高度"估算 `maxScrollExtent`，
  于是滚动条滑块会**越滚越短**（用户报的"滚动条不能反映当前位置"）。
  「AI 模型与凭据」页 69 个模型实测：页顶 3512 → 滚到底 21318，滑块 17.9% → 3.5%。
  正确做法是 `CustomScrollView` + `SliverFixedExtentList(itemExtent: 常数)`（extent 精确、滚动条不跳），
  行的详细编辑搬进 `showDialog`；回归用例 `test\model_list_scroll_test.dart`。
  代价是每行高度是常数，**别在行里塞会换行的 `Wrap`**（那会让常数失效）。
- **目录卡不许占页顶**（2026-10 用户原话："模型能力目录区域占用窗口区域太大，改成嵌在具体
  provider 信息里"）。「AI 模型与凭据」页现在页顶**一张常驻卡都没有**：Provider 列表 + 详情
  两栏，能力目录 `ExpansionTile`（默认折叠）挂在**选中那条**的详情末尾。
  凡是"与当前选中项无关"的大块状态，优先做成折叠项或详情内的一节，别做成常驻横幅。
  回归用例 `test\phone_layout_overflow_test.dart`（384/320dp 两档都不许溢出）。
- **内置模型目录那张卡与它的 Dart 客户端已删**（2026-10："模型能力不再依赖内置目录"）：
  它抄自 `%USERPROFILE%\.dsh\settings.yaml`，而第 10 节已经定了"运行期不许依赖 `.dsh`"。
  能力预填改由 **models.dev** 在线目录负责（`capability_refresh.rs`）。
  后端 `server-rs\src\builtin.rs` 与 `/api/ai/builtin/{status,sync}` 仍在（Rust 用例盯着），
  但界面上**没有任何入口** —— 别再把 `AiBuiltinCatalogStatus` / `builtinStatus()` 加回来。
- **工作台的模型选择器按 Provider 分组**（2026-10 用户原话："来自不同 provider 的模型要套在
  一个 padding 里面区分，padding 外标注模型来源"）。布局是硬要求：
  `SliverToBoxAdapter(来源标注)` + `SliverPadding(SliverList(模型))` 一组一组，
  回归用例 `test\ai_model_picker_test.dart`。
  数据源是 `AiWorkspaceStore.catalogByProvider`（`providerId → 模型`，打开选择器时才拉），
  **不是** `store.models`（那只是当前那一条的）—— 换 Provider 走 `selectModelFrom(p, m)`，
  别拆成"先 `selectProvider` 再 `selectModel`"：那两步都是 `await` 的，
  中间那次选中会被 `_loadModels()` 收尾时的"取第一个启用的"覆盖掉。

---

## 7. 两种运行布局：源码树 vs 发布包（改路径解析前必读）

**App 自己直接起后端**（2026-09-30 起），所以"找东西"的逻辑只有一份真源：
**`lib\core\backend_paths.dart`**。改路径解析 = 改它；`scripts\backend.ps1` 是它在脚本世界的
**镜像**（只服务 e2e / dev），**改一边必须改另一边**。

| 找什么 | 源码树（开发） | Release 产物 / 发布包 |
| --- | --- | --- |
| 运行根目录 | 往上找 `server-rs\Cargo.toml`（或 `db\schema.sqlite.sql`、`backend\comfyhub-server-rs.exe`） | `viewer.exe` 所在目录本身 |
| 后端 exe | `<根>\server-rs\target\{release,debug}\comfyhub-server-rs.exe` | `<viewer.exe 同级>\backend\comfyhub-server-rs.exe` |
| `viewer.exe` | `build\windows\...\runner\{Debug,Release}\` | `<根>\viewer.exe` |
| 数据库 / 产物 / 端口 | `<根>\.db\` / `<根>\storage\` / `<根>\.run\api.json` | 同一份约定（可写数据都在根目录内） |
| Java | **不需要**（Rust 后端） | **不随包携带** |

踩过的坑：

- **后端 exe 的候选顺序只有一个真源**：`serverExeCandidates()`，顺序是
  `$env:COMFYHUB_SERVER_EXE` > `<exe 同级>\backend\` > `<根>\backend\` > `<根>\server\`（旧布局兼容）
  > `server-rs\target\release\` > `server-rs\target\debug\`。
  `$env:COMFYHUB_SERVER_EXE` **永远最高优先**，它是"把后端映像挪出受限区"的唯一开关（第 12 节）。
- **根目录判据必须同时认两种布局**：只按源码树的路径写，发布包里就会出现"文件明明在，它却说找不到"。
  反之亦然。三种判据任一命中即可（`server-rs\Cargo.toml` / `db\schema.sqlite.sql` /
  `backend\comfyhub-server-rs.exe`）。
- **端口只有一个真源**：后端绑好之后自己写 `<根>\.run\api.json`。
  App（`settings_store.dart`）、脚本（`api-port.ps1`）、ComfyUI 捕获扩展（`capture_core.py`）
  **都只读它** —— 谁也不许自己编一个端口写进去（`api_port_file.rs` 是那个写者）。
- **`--port 0` 是刻意的**：让操作系统挑空闲端口。写死端口就会撞"被占 / 落在 Windows 保留段"，
  而且真实端口要从 listener 读回来（`--port 0` 时那个 0 不是端口）。
- **P4 收尾（2026-09-26）起 Kotlin 已退役**：源码在 `legacy\server\`（`.gitignore` 里
  `/legacy/server/build`、`/legacy/server/storage` 照样忽略），发布包不带它、也不带 `jre`。
  要回退旧后端得先把 `legacy\server` 放回原位自己构建 —— 这条路只在灰度期有意义。
- **`db::open_or_create` 是"第一次运行能起来"的关键**：以前缺库直接 `exit 3`（"先让 Kotlin 起一次库"），
  在发布包里等于**永远起不来**。现在库缺就自己建（DDL 编译期 `include_str!` 嵌在二进制里）。
- **发布包是便携式的**：可写数据（数据库 `<根>\.db`、产物 `<根>\storage`、日志 `<根>\.run`）
  都在包内，`packaging\manifest.json` 里 `runtimeLayout` 记着这份约定；改存放位置要先改那里和 README 9.2。
- **Release 产物默认"就地装配"**：`flutter build windows --release` 自己就把后端装进 `backend\`
  （`windows\CMakeLists.txt` + `windows\cmake\bundle_backend.cmake`），装完
  `build\windows\x64\runner\Release\` 就是完整发布包。三个连带后果别踩：
  ① `flutter clean` / 重新构建会把它整个清空（又变回只有 App），重跑一次 release 构建即可；
  ② `pack-release.ps1 -Clean` **不能**整个 `Remove-Item $OutDir`，否则把 `viewer.exe` 一起删了 ——
     只能清清单里那些 target（`backend` / `scripts` / `db` / `comfyui` / `skills` …）
     + 老组件（`server` / `jre` / `mysql`）+ `packaging` / `BUILD-INFO.txt`；
     运行期数据（`.db` / `storage` / `.run`）**不要动**；
  ③ `flutter_release` 组件此时**源和目标是同一个目录**，必须跳过拷贝
     （`Copy-Item` 会报 "Cannot copy item to itself"）。
- **`install(SCRIPT)` 里读不到 configure 期的变量**（2026-09-30 踩到）：`CMAKE_CURRENT_SOURCE_DIR`
  与"展开后的 `CMAKE_INSTALL_PREFIX`"都要由 `install(CODE ...)` **提前 `set()` 进去**——
  后者的真值是生成器表达式 `$<TARGET_FILE_DIR:viewer>`，install 阶段直接读会拿到
  **没展开的原字符串**（报 `file cannot create directory: ...\$<TARGET_FILE_DIR:viewer>\backend`）。
- **`cargo build --release` 会在后端正在跑时失败**：Windows 锁着运行中的 exe，
  报的是 `failed to remove file …comfyhub-server-rs.exe / 拒绝访问 (os error 5)` ——
  先 `pwsh -File scripts\backend.ps1 stop` 再构建。
- 想要一份不随 `build\` 消失、又**在工作区外**的长期副本，用
  `pwsh -File scripts\pack-release.ps1 -OutDir D:\dist\ComfyHub`（既是长期留存，也是绕开写限制的正解）。

## 8. 运行时自动补齐：**已经不需要了**（保留两条 PowerShell 血泪教训）

2026-09-30 起 Java / pwsh / VC++ 三样运行时**都不再需要**（后端是静态链接的 Rust 单文件 exe，
数据库是 SQLite 单文件），所以 `ensure-runtime.ps1` / `runtime-deps.ps1` 已删除。
唯一还会下载东西的是**可选能力** MCP 运行时（`scripts\mcp-runtime.ps1`），它的策略仍然是：
**检测优先，安装只作兜底**（2026-09-21 用户定："环境有 python 你的脚本就不用再装了，同理 jdk、mcp 也是"）：

- 四档：① 已装好的（`state.json` / `venv` / `python`）→ ② 借机器上**已经装了 comfy-mcp + comfy-cli**
  的 Python（一个字节都不装）→ ③ 借机器上的 Python 建 **venv**（`<根>\storage\ai\mcp\venv`，
  **不污染用户的全局 site-packages**）→ ④ 真的没有才下 embeddable 便携版。
  用了哪一档记在 `state.json` 的 `pythonSource`。
- ⚠ **venv 的解释器就在 `Scripts\` 里**，所以"pip 启动器在哪个目录"必须分两种情况
  （`Get-PythonScriptsDir` / Kotlin 的 `scriptsDirOf`）—— 一律 `父目录\Scripts` 会拼出
  `venv\Scripts\Scripts`，表现是"明明装好了却报缺 comfy-mcp.exe"。
- **装不上不拦启动**：它是可选能力，缺了只是 AI 少一批 ComfyUI 工具。

> **⚠ 参数改名的坑（2026-09-21 踩到，仍然有效）：非高级函数（没写 `[CmdletBinding()]`）收到不存在的
> 命名参数时不报错，而是丢进 `$args`。** `mcp-runtime.ps1` 把参数从 `$Proxy`/`$Force` 改成
> `$McpProxy`/`$McpForce`（为了躲 dot-source 的变量覆盖，见该文件头的注释）之后，内部一堆
> `-Proxy $McpProxy` / `-Force:$McpForce` 就**静默失效**了 —— 没报错、没异常，只是代理没传过去、
> 强制重装没生效。所以：改名后**必须把调用点一起改**，而且给命令行用的名字要留
> `[Alias('Proxy')]` / `[Alias('Force')]`。

> **⚠ 变量的坑：永远不要给变量起名 `$home`、`$input`、`$host`、`$pid`、`$profile`、`$args`。**
> PowerShell 里自动变量**大小写不敏感**，`$home = ...` 会直接抛
> "Cannot overwrite variable HOME because it is read-only or constant"；
> 而更可怕的是**漏改了一处引用**：本仓库真的写出过
> `$javaHome = ...`（定义）却仍用 `$home`（引用）的代码，于是
> `Move-Item $home -Destination <解压目标>` 去搬 **`C:\Users\<用户>\` 整个用户目录**，
> 把用户主目录下的 10 个文件（`.gitconfig` / `.npmrc` / `.claude.json` …）搬进了发布包
> （幸好目录被占用才没造成更大破坏；出事的那个脚本已随"运行时自动补齐"一起删除，
> **但教训对所有 `Move-Item` / `Remove-Item -Recurse` 都成立**）。
> 写任何搬移 / 递归删除之前，先确认目标是"算出来的、可验证的"路径
> （当时的修法是加一条安全闸：只允许搬运 staging 目录内的路径，否则当场抛异常）。

## 9. 视频播放依赖一份打过补丁的插件副本

`pubspec.yaml` 里的 `video_player_win` 指向 **`third_party\video_player_win`**（上游 3.2.2 的本地副本），
不是 pub.dev。原因：上游的 `GpuSurfaceDescriptor` 少了 `visible_width / visible_height` 两个字段，
而 Flutter 3.4x 在 Windows 上**默认用 Impeller**、偏偏拿这两个字段当外部纹理尺寸 ——
结果是**只有声音、画面全黑**（引擎日志里能看到 `Could not create external texture`）。

- 改动点一共两行，搜 `ComfyHub 补丁` 就能定位（`windows\video_player_win_plugin.cpp` 的 `initTexture()`）。
- **别把它"顺手升级"回 pub.dev**，也别在 `flutter pub upgrade` 后不看 `pubspec.lock`：
  一升回去就立刻复发。上游修好后再整体换回去，并删掉这一节。
- 排查这类问题的顺序：① 看 `flutter run` 的控制台有没有 `Could not create external texture`；
  ② 确认日志里是 `Using the Impeller rendering backend`；③ 用 `--no-enable-impeller` 跑一遍对照
  （Skia 会容忍 0 尺寸，所以它在旧版本里"看起来是好的"）。
- 相关回归手段：`.run\videoprobe\ui.ps1` 是当时写的窗口截图 / 点击小工具（**不在版本库里**，按需重建），
  `scripts\dev-app.ps1` 起 debug 版后按 `r` 热重载最快。

## 10. AI 工具与 Skills（M4 / M5）的接线约定

**功能说明见 [`docs/ai-tools-and-skills.md`](docs/ai-tools-and-skills.md)**（含"DSH 哪些工具抄了、哪些没抄"的对照表）。
这里只记改动时容易踩的规矩。

- 工具清单**只有一个真源**：`server-rs/src/tools.rs` 的 `catalog`（桌面全量）与 `mobile_catalog`（手机 7 个）。旧 Kotlin 那份 `legacy/server/src/main/kotlin/com/comfyhub/ai/tools/ToolRegistry.kt` 只作**行为参照**。
  系统提示、`GET /api/ai/tools`、界面都从它派生 —— 加工具就在这里加一项 + 在 `ToolRegistryTest` 补用例，
  **别在别处再维护一份"可用工具"名单**。
- **读白名单里会自动加上本机 ComfyUI 的目录**（`ComfyRoots` 每次现探、不落库，见用户 bug ④）：
  用户的 ComfyUI 不在项目里（发布包解压到哪、ComfyUI 装在哪，两者无关），只按 `<根>\comfyui` 判
  会让 `read_file` 读不了用户自己的工作流文件。**这只放宽读** —— 写仍然只认 `<根>\comfyui`，
  `comfy_use_attachment` 往 ComfyUI `input\` 投放那条专用通道不受影响。
  判定仍是"认特征文件"（复用 `ComfyLocator.looksLikeComfy`），搜索有界（深度 ≤3、只往名字带 comfy 的分支下钻）。
- **本机 ComfyUI 的工作流文件是"库"之外的第二来源**（用户 bug ⑤，2026-09-18）：`prompts` 库只收
  "捕获过的运行"与"手动读进来的文件"，所以**首次使用时库必然是空的** —— 而
  `user\<用户>\workflows\*.json` 早就存在。`ComfyWorkflowFiles` 现探这些目录（配置的输出目录父目录 →
  `ComfyLocator` → `ComfyRoots`），`comfy_list_workflows` / `GET /api/capture/workflows` **只列不写**；
  批量入库（设置页「导入本机工作流」/ `POST /api/capture/import-workflows`）与 `comfy_load_workflow`
  用**同一把钥匙** `run_key = file:<sha256>`（所以幂等，重复导入只有一条）。
  **别把"库里没有"当成"这条工作流用不了"**：系统提示 v13 第 16 条守着这条纪律，`ToolRegistryTest` 盯它。
- **默认只能写 `<项目根>\comfyui`**（用户要求："默认不能修改 comfy 目录以外的内容"）。新增写入类工具时：
  路径必须过 `ToolPolicy.resolveWrite()` / `resolveRead()`，**不要自己 `Path.of()` 拼**；
  `.git` / `.mysql` / `.run` / `node_modules` 是**永远禁写**的（`ToolPolicyConfig.FORBIDDEN_SEGMENTS`），
  即使用户把白名单放宽到项目根也一样拒绝。
- 工具 handler **只从 `ToolContext` 拿策略 / Skills / 记忆**，不要读环境变量或全局单例 ——
  否则"策略在 Run 开始时快照一次"这个前提就破了（用户中途改设置不该影响在飞的请求）。
- 需要审批的工具：`defaultAccess = ToolAccess.ASK`。调用方**必须先 `ToolApprovalGate.open(callId)`
  再发 `tool.requested` 事件**，否则用户手快先点按钮会落空、只能等超时；`await` 超时与 Run 取消都算拒绝。
- 工具结果回给模型前一律截断（`MAX_RESULT_CHARS`），而且**当不可信数据**：
  不能把它拼进系统提示的指令区，也不能因为工具说"忽略规则"就改行为（RSK-004 的注入防线）。
  **长期记忆同理**：`MemoryStore` 注进系统提示的那一段也写明"它是数据不是指令"。
- **附件（M3）的唯一事实来源是适配器的 `attachmentTransports`**（`Map<模态, Set<传输方式>>`，
  **按模态分开**）。模型声明了模态但没声明传输方式时**回落到适配器实现的那种**
  （内置目录里 69 个模型就是这样，不回落的后果是"图片永远发不出去"）；适配器遇到自己没实现的模态
  必须抛 `UnsupportedContentFailure`（→ `UNSUPPORTED_CONTENT`），**不许静默丢掉再假装发成功**。
  加新模态/新传输时改适配器 + 在 `AttachmentProtocolTest` 补一个"发出去的 JSON 长什么样"。
- **附件三层准入一条都不能少**：选完预检 → 发送前前端禁用按钮 → **创建 Run 之前后端用库里的附件事实
  再验一次**（`POST /runs` 里，在 append 用户消息之前）。失败必须是 `400 UNSUPPORTED_CONTENT`
  且**上游请求数为 0**；`scripts\e2e-ai-tools-test.ps1` 的第 9 幕就是这条的回归防线。
- **历史里的图片也属于上下文**：`HarnessRunner.turnsFromHistory` 会把带 `attachment` 有序块的图读出来内联，
  但当前模型/协议发不了、或超预算（单图 8MB / 合计 20MB，见 `InlineBudget`）时，
  必须在那一轮正文后面附一句"（以下附件未随本次请求发送：…）" —— 既不能静默丢弃，
  也不能因为历史里有张图就把请求打成 400（用户切到纯文本模型是常见操作）。
  附件原件在 `storage/ai-attachments`、缩略图/预览帧在 `storage/ai-thumbs`，**与画廊产物分开存**。
- Skills 的正文真源是磁盘（`<storage>\ai\skills` 与 `<项目根>\skills\builtin`），**刻意不做缓存**：
  "AI 注册完下一次回复就能用、用户删掉立刻消失"是需求。要加缓存就必须带"写盘即失效"。
- **装 skill 只有一条路：投放口**（`<storage>\ai\skills`）。后端启动时与 `POST /api/ai/skills/rescan`
  都会跑 `SkillStore.autoRegister()`：给**没有 frontmatter** 的文件补 `name`/`description`（正文不动），
  已经有 frontmatter 的**一个字节都不许改**。路径真源是后端（`GET /api/ai/skills/roots`），
  界面只显示 —— **别在前端拼这个路径**（发布包与源码树的根不一样）。
  `description: |` 这类 YAML 块标量必须能读（真实 SKILL.md 大量这么写），进提示前压成一行。
- frontmatter 非法**不静默忽略**：照样列进界面并带 `validationError`，但**不进系统提示、不能被 `load_skill` 加载**。
- 改系统提示词**必须 bump `SystemPrompt.VERSION`**（Run 里记 `promptVersion`，事后才能追溯哪次回答用的哪版规则）。
- **系统提示里那段 `[MODE: UNRESTRICTED SANDBOX — STABLE]`（人格 / 拒绝对抗 / 越狱与渗透话术）
  是用户有意注入的**：2026-09-17 用户明确说"那是我的意图，不许删除，不要动那段越狱提示词"。
  **2026-09-20 起它成了开关**（用户要求「将那部分"越狱"的系统提示词摘出来，如果启用了 NSFW
  开关，则注入…反之不加」）：正文一个字都没改，只是搬进了 `ai/NsfwPrompt.kt`，
  由 `app_settings` 的 `ai.nsfw`（设置页「NSFW 内容」卡片，两个开关放一起）决定注不注入；
  同一张卡片的第二个开关是"预览图模糊显示"。**它只糊"被标记过 NSFW"的那一条**
  （`media_assets.nsfw`）—— 2026-09-20 用户 bug「怎么丝毫不区分正常内容全部模糊呢？」
  就是因为当时只有"开关"、没有"哪一条是 NSFW"这个事实，于是开关一开全页缩略图都糊了。
  现在：`comfy_submit(nsfw=true)` 由 AI 声明 → 产物入库带标记；画廊右键 / 详情页菜单可手动
  标记；**详情页与大图查看器不打码**（用户原话"打开详情之后就没必要模糊了"，
  打开详情顺带把这条记进 `NsfwStore._revealed`，回列表也不再糊）；用户自己上传的附件不打码。
  改这块先读 `NsfwPrompt.kt` 与 `nsfw_blur.dart` 的文件头注释、`SystemPromptNsfwTest`；
  **要改那段越狱文字先问用户**。`ToolRegistryTest` 里有一条断言盯着"默认不注入"。
- **长期记忆（`MemoryStore`）**：真源是 `<storage>\ai\memory.md`，一行一条，用户能直接手改；
  写入（AI 的 `remember` / 界面的保存）**超限一律报错，不许静默截断**（截断等于悄悄丢用户的话）；
  注入系统提示时截断到 `PROMPT_CHARS`，文件本身保留完整内容。
- **任何运行期代码都不许依赖 `%USERPROFILE%\.dsh`** —— 装我们项目的人可能根本没装 DSH
  （模型目录同理，见 `AiSeedCatalog` 的 classpath 冻结副本）。这条以前是"只在点导入时读一次"，
  现在连那个按钮都删了：DSH 与本项目再无运行期关系。
- **工作流配方（2026-09-26）**：`workflow_recipes` 表 + 三条工具（`comfy_learn_workflow` /
  `comfy_list_recipes` / `comfy_bind_recipe`）+ `/api/ai/workflow-recipes`。改这块先读
  [`docs/workflow-recipe-design.md`](docs/workflow-recipe-design.md) 与 `WorkflowRecipe.kt` 的文件头。四条别踩：
  ① **配方不是可提交对象** —— 提交永远走 `comfy_submit`（红线 R10 一个字不改），配方只提供 overrides；
  ② **默认拒学"转换有缺口"的图**（`UNSUPPORTED_NODES`）：缺了节点的骨架看着像、其实缺链，而且**不报错**；
     真机上最常见的原因是"本机这个实例没装那个自定义节点包"——正确动作是让用户启停/换实例，**不是改他的工作流**；
  ③ **绑定只报不改**：只有 `exact`（同名且实例能加载）能自动写键，其余四档各有各的处置（见设计 §4）；
  ④ 学习与绑定**只有一份实现**（`Application.kt` 里那两个 lambda，工具与端点共用）；
  表**不进同步白名单**（手机提交走 `comfy_queue_submit`）。端到端：`scripts\e2e-recipe-test.ps1`。

### 10.1 提交任务与实时进度（2026-09-17 起）

- **`comfy_submit` 只能提交 API 格式节点图**，那份图在 `capture_runs.raw` 里
  （`ComfyCapture.apiGraphOf`），**不是** `prompts.workflow_json`（那是界面格式，
  `widgets_values` 只有位置没有参数名，转换出来一定是错的）。
  没有 API 图的老数据要报 `NO_API_GRAPH`，别猜着转。
- 参数覆盖走 `ComfySubmitter.WorkflowEdit.applyOverrides`：键是 `节点id.输入名`，
  **按原值类型转换**，字段不存在 / 是连线数组一律抛错。**不许静默忽略**。
- 提交后的产物必须走 `capture.captureRun()` 入库（与轮询捕获同一条路，幂等靠
  `prompt_id` + 文件 SHA-256）；绕开它自己写库就会出现"AI 出的图没进画廊"或者重复入库。
- 等待有上限（`DEFAULT_WAIT_SECONDS` / `MAX_WAIT_SECONDS`），超时记 `timeout` 并如实说明，
  **不许假装完成，也不许假装失败**。
- **界面上的产物入口只认后端给的 `mediaIds`**（`tool_result.jsonPayload.mediaIds` /
  `tool.completed.result.mediaIds`）：模型在正文里说"我生成了 xx"不算数。
- 详情页 / 画廊入口卡的跳转用 `MediaDetailPage(mediaId:)`；别在前端自己拼媒体 URL
  （`thumbUrlFor` / `fileUrlFor` 走的是后端算好的路径）。
- **会话标题是"模型在正文最前面输出 `[标题]…[/标题]`，后端摘掉"**（`TitleStripper`）。
  改这段代码时守住三条：**绝不吞内容**（标记不闭合就把缓冲当正文放出去）、
  **绝不重复内容**（寒暄被逐字流式送达时不能放两遍）、
  **摘到标题之后还要继续盯**（模型在后面的轮次里会把标记再吐一遍 —— 那叫回声，
  同样得摘掉，但**不许改**已经定下来的会话名）。`ConversationTitleTest` 盯的就是这三条。
  另外：那条"起标题"的附加指令**只能按轮带**（`runRound(askTitle=…)`），
  **绝不能塞进常驻的 `turns`** —— 常驻会让后面的每一轮都再提醒模型输出 `[标题]`，
  标记就漏进正文（2026-09-19 用户报的"回答首尾被截断"里的"首"就是这个）。
  **位置也必须插在系统提示之后、历史之前（第 2 条），不能挂末尾**：挂末尾会挡住
  "最后一条消息是工具结果"这个事实 —— 模型靠它判断"工具跑完了、该收口"，
  假网关靠它分流。实测代价：`e2e-submit-test.ps1` 的提交流程会一直重复调 `comfy_submit`、
  最后卡在审批闸门上（Run 永远 `running`），而第 10 轮"没动提交链路所以没重跑"，红了整整一轮
  （2026-09-19 深夜修）。**教训：改了 `HarnessRunner` 的上下文拼装，三条 e2e 全跑一遍。**
- **ComfyUI 位置解析有两份实现**（`ComfyLocator.kt` 与 `scripts\comfy-path.ps1`），判据必须一致：
  源码形态 = `main.py` + `comfy/`；任何形态 = `output/` + `models/` 或 `input/`。
  改判据要同时改两边，否则会出现"App 说找到了、脚本说找不到"。
  探测**只读**，写配置必须由用户点「使用这个目录」（`POST /api/capture/locate/apply`）。
- **`capture_runs` 的 `empty` / `error` 是可重试状态**（`CaptureRepo.canReclaim`）：
  ComfyUI 的 `/history` 先出现记录、产物文件晚到是常态，把 `empty` 当终态就会出现
  "AI 说生成了、画廊里却没有"。只有 `success` 才是"不再重复收"的终态，改这段前先看
  `CaptureRunRetryTest`。
- **失败 / 取消时"已经说出去的话"一个字都不能丢**（用户 bug，2026-09-19 的 429 现场：会话
  `9b6d01af` 的 seq12 —— 那次 Run 跑了 4 分钟、3 次 `comfy_submit` 全成功，最后一轮撞 429；
  用户点「重试」后那条消息变成 `text=''` + 零个 part，全没了）。三条纪律：
  ① **`message.text` 里有的内容，parts 里也必须有** —— 前端 `_orderedBody` 在 parts 非空时
  **只渲染 parts**，只写正文不落块 = 用户看不见。落块时机是"一轮跑完"，所以被 429 / 断流打断的
  那一轮必须由失败分支补落（`PartSink.flushInflight()`，挂在 `rescueAlreadySaid()` 上）；
  ② **标题摘取缓冲**（`TitleStripper` 还没确认不是标记时压住的开头，最多 ~200 字符）
  失败 / 取消时也要 `flush()` 放出来 —— 第一条回复刚开口就断时，那是整段话；
  ③ **"就地重置"只能用在空壳上**：`POST /runs` 重试时判据是
  `AiMessageDto.isEmptyReply()`（正文空 **且** `AiConversationRepo.hasParts()` 为假），
  有内容的失败回复**原样留着**、重试另起一条助手气泡（前端按返回的 `assistantMessageId`
  是否变化决定"就地换占位 / 末尾追加"）。**别退回"只要 failed 就 resetAssistantForRetry"**——
  那会把工具卡和已经生成的图片入口卡一起删掉。
  防线：`e2e-ai-tools-test.ps1` 第 13 幕 15 项（假网关 `failkeep_*` / `break_*` 两个场景）
  + `test/ai_retry_keep_content_test.dart` 2 例。**写假网关场景时**：给会话先定一个标题，
  把"起标题"那条链路从断言里摘出去（历史上那条指令排在最后一条 system，把
  `last.role == "tool"` 这类分流挡住过 —— 现在它固定插在第 2 条，见上面那条）。
- 改完这一块请跑两条端到端（都不出网、不花钱）：
  `pwsh -File scripts\e2e-submit-test.ps1`（提交链路，6 幕：审批闸门 / 真提交 / 参数覆盖 /
  工作流文件直接提交 / 前端节点缺口补线 / 本机工作流列出来并一键入库）与
  `pwsh -File scripts\e2e-capture-test.ps1`（捕获链路）。它们会临时改自动捕获配置、
  建一个 loopback Provider，跑完自己还原并清理数据；`-KeepData` 可以留着看效果。
  **别在跑完前 Ctrl+C**：清理在 finally 里，中断会留下 `e2e-submit-*` 的运行记录，
  而那会把下一次捕获 e2e 的 SHA-256 判重弄乱（实测踩过：表现为"新导入 0 个"）。
  `e2e-submit-test.ps1` **会自己把权限档临时钉成 `ask`**（第 1 幕验的就是"没人批准就不提交"，
  用户平时可能切在「自动允许（无需批准）」档，靠机器当前设置会让这一幕必然红），跑完还原。
- **事件流是"批写 + 只从库里读"，所以必须有人 flush**（2026-09-28 用户 bug：
  "回复的首段思考要等到第一个工具调用才显示"）：`RunEventBus::emit` 攒够 `EVENT_BATCH = 200`
  条才开事务落库，而 `SseStream::poll_next` 是按 `?after=` **从库里读**的。改动这块时守住两条：
  ① `poll_next` 每轮读库**之前先 `bus.flush`**（空批不开事务，代价可忽略）——去掉它就是
  "开头几条事件要等下一个闸门类事件才现形"；② 新的**闸门类事件**（等审批 / 等回答那种）
  发完要**立刻 `flush`**（`tool.requested` 与 `user.question` 就是这么做的）。
  防线：`run_lifecycle.rs::sse_sees_events_without_waiting_for_a_full_batch`。
- **环节标记（第 21 条）与关键步骤折叠**（2026-09-28）：模型用 `[步骤]环节名[/步骤]` 声明环节，
  六个固定名字的真源是 `server-rs/src/step_mark.rs` 的 `PHASES`，**系统提示第 21 条与它必须逐字一致**
  （不一致的表现是"模型写的名字被摘掉、界面上什么都不出现"，而且不报错）。改这块记住：
  ① 摘取**绝不吞内容**（不闭合 / 撑爆 / flush 三条兜底都要把原文放出去）；② 枚举外的名字
  **摘掉但不记环节**；③ 有序块类型是 `step`、实时事件是 `step.started`（契约文档 §2 要一起改）；
  ④ 界面按"**连续**的工具卡"成组，正文与工具卡的先后**永远等于到达的先后**（bug⑥ 的纪律），
  同一环节夹了正文就另开一组标「续」；⑤ 组里有等批准 / 失败 / 被拒的调用时**不许折叠**。
  防线：`step_mark.rs` 6 例 + `test/ai_tools_ui_test.dart` 的 (e)(f)(g) + `stripStepMarkers` 那条。

---

## 11. ⚠ 目标架构（P0/P0.5/P1/M0/M1 ✅、P2 ✅、**P4 桌面 Rust 切流完成 2026-09-26**；剩 M3+/M4 的移动端）

**2026-09-23 敲定**：`SQLite + 桌面 Rust + 移动 Kotlin + Flutter（两端）`。
**2026-09-26 现状**：**桌面后端已是 Rust**（`server-rs/`，单文件 exe，无 JVM / 无 MySQL），
脚本与发布包都切过去了，四条 e2e + `flutter test` 对着 Rust 全绿；旧 Kotlin 源码在 `legacy\server\`（留档）。
第 1~10 节里凡是提到 **Kotlin 后端 / `gradle` / `installDist` / `jre` / `ToolRegistry.kt` / MySQL 搬迁**
的地方，都按**历史（或"回退旧后端"）**理解 —— 运行期真源是 Rust。

**动手之前先读 [`docs/implementation-master-plan.md`](docs/implementation-master-plan.md)**
（实施总纲：冻结的接口契约、**14 条红线**、分阶段验收判据、回归防线归属、文档地图）。

四条最容易踩的：

1. **桌面后端只改 `server-rs/`（Rust）**：别在 `legacy\server\`（Kotlin）里继续堆东西 ——
   它已经不在运行路径上，改了也没有任何效果（除了灰度回退那一次）。旧代码只当**行为参照**读。
2. **不要引入 KMP**（`kotlin.multiplatform`）。本工程已迁到 **AGP 9 内建 Kotlin**
   （`android.builtInKotlin=true`，任何模块都不 apply KGP），KMP 与这个刻意设计正面冲突 ——
   理由见 `docs/mobile-harness-design.md` §2.1。动 `android/` 前照旧先读
   `docs/android-agp9-builtin-kotlin-migration.md`。
3. **移动端 harness 用 Android Kotlin**（Flutter UI 通过 platform channel 调用），
   不是 Dart、不是 Rust。**手机端不许硬编码系统提示 / 工具描述 / 安全文案** ——
   它们由桌面的 `/api/ai/run-kit` 版本化下发。
4. **发布包与运行时**：`mysql/`（304MB）、`jre/`（~40MB）与 Kotlin jar（~20MB）**都已经去掉**，
   装配实测约 **14MB**。`packaging\manifest.json` 与第 7 节的"两套运行布局"改之前**先看总纲**；
   `runtimeRequirements` 里的 `required` 字段决定"缺了算不算问题"。

---

## 12. ⚠ 写不进工作区外：真因是**完整性标签（Low IL）** + `min(父进程, 映像)`（2026-09-30 定案）

`comfy_start` 报 `COMFY_WRITE_BLOCKED`、`comfy_use_media` 报 `COMFY_INPUT_FAILED … os error 5`、
MCP 任何工具报 `UnexpectedToolError`、`cargo test` 一批 `PermissionDenied (os error 5)` ——
**同一批症状、同一个根因**。动手改之前先读完这一节：它推翻过 2026-09-28 与 2026-09-29 的**两版处方**
（现象每次都量对了，解释与修法都错）。

### 机理（用户 2026-09-30 实测钉死）

1. 代码沙箱把**工作区目录树**打上 `Mandatory Label\Low Mandatory Level:(OI)(CI)(NW)`
   （`(OI)(CI)` = 子目录/文件继承，`NW` = no-write-up）；
2. 在工作区里构建出来的 exe（`comfyhub-server-rs.exe`、`viewer.exe`、测试二进制…）
   **继承了 Low 标签**；
3. Windows 创建进程时取 **`min(父进程令牌, 映像标签)`** —— 映像标签是 Low，进程令牌就是 Low；
4. **Low 写不了 Medium 对象**（工作区外默认没有标签 = Medium）⇒ `拒绝访问 (os error 5)`。

推论两条，别记错：

* **只把 exe 的标签清回 Medium（位置一个字节不动）→ 后端立刻哪儿都能写**（实测）；
* **链路上任何一环是 Low，子进程照样掉回 Low** —— 所以"只清后端 exe"不够：
  `viewer.exe` 也是工作区里构建的、也带 Low 标签，那 App 自己就是 Low，
  它起的后端一定是 Low，而且**这时连"修标签"这一步都做不动**（Low 改不了向上的标签）。

### 量出来的判据（哪些怀疑对象被排除）

| 怀疑对象 | 实测 | 结论 |
| --- | --- | --- |
| 目录 ACL | 诊断脚本报 `NOT_THIS_CLASS`（无 deny ACE、无 package ACE，`WRITE_DAC`/`WRITE_OWNER` 都在） | 无关 |
| 路径配错 / 用户身份 / Defender | 同用户同目录 `powershell` 写得进；Defender 无受控文件夹规则 | 无关 |
| **映像上的 Low 标签** | **同一二进制逐字节相同：工作区内 → 写 `%TEMP%`、`D:\Comfy-Desktop\…` 全 `os error 5`；`copy` 到 `%TEMP%`（那里没有 Low 标签可继承）→ 哪儿都能写** | ✅ 就是它 |
| 从哪个会话 / 终端起 | 双击一样被限；WMI / 计划任务 / Explorer 逐条量过都一样 | 无关（决定权在**映像标签**） |
| spawn 档位 | `wmi-hidden` / `wmi-vbs` / `direct-hidden` 三档写同一个探针**都是 OK** | 无关（见第 2 节） |
| 父进程是谁 | 高完整性父进程 + Low 映像 → **仍然受限**（`min` 取小的那个） | 只有父进程**也**是 Low 时才更糟 |

### 三条处方（仓库里已经做好的就是这三件）

1. **发布/构建时把两个 exe 的标签都归一为 Medium**：
   `windows\cmake\bundle_backend.cmake` 在 release 装配末尾对
   `viewer.exe` 与 `backend\comfyhub-server-rs.exe` 各跑一次
   `icacls <exe> /setintegritylevel Medium`（**失败只警告不中断构建** ——
   在受限会话里构建时必然失败，那时只能走第 2 条）。
2. **把整套 ComfyHub 放到工作区外**（终解，也是最稳的一条）：

   ```powershell
   pwsh -File scripts\pack-release.ps1 -OutDir D:\dist\ComfyHub
   ```

   然后运行 `D:\dist\ComfyHub\viewer.exe`（App 自己起 `backend\` 里的后端）。
   那边的文件没有 Low 标签可继承 —— **既不受限，内容也仍然全在它自己的文件夹里**（第 13 节）。
3. **运行时就地修**：App 起后端之前会 best-effort 跑一次
   `icacls <后端 exe> /setintegritylevel Medium`（`backend_launcher.dart` 的 `_normalizeIntegrity`）；
   `scripts\backend.ps1 start` 也会先修（`Repair-LaunchImages`）。
   这一条**只能救"App 自己是 Medium、只有后端 exe 还带 Low 标签"那一半**（实测存在这种现场），
   App 自己也是 Low 时它必然失败，且日志里会如实说。

⚠ **两条边界**：**"换个终端重起"没用**（映像标签没变，进程照样是 Low）；
**别建议往 `%LOCALAPPDATA%` / `%TEMP%` 之类拷后端**（违反第 13 节，而且那只是碰巧绕开标签）。

### 四条纪律

1. **闸门留着**（`comfy_launch::ensure_writable`）：白起一个 100% CPU 卡死的实例，比如实拒绝糟得多。
   但**文案必须说对**：先讲真因（Low 完整性标签 + `min(父进程, 映像)`），再给上面那三条处方。
   **别再写"没有退路 / 只能让用户双击 ComfyUI Desktop"**（双击确实能 work，但不是唯一那条）；
   **别写"换个终端重起就行"**；**别建议拷贝到系统目录**。
   `confinement::remedy_text()` / `comfy_launch` / `image_edit` 三处文案是同一条纪律，用例
   `修法文案指向发布包而不是让用户改_acl` 盯着它们**不许出现"权限 / ACL / 换个终端 / 双击 ComfyUI"**。
2. **别把探针的 `Err` 吞成 `bool`**（`confinement::probe_dir` 返回 `io::Result` 就是为此）：
   只报"哪个目录"不报"报什么错"，就会重演"现象对、处方错"。
   用例里也不许拿"这台机器哪个目录不可写"当判据 —— 用**同名目录占位**造出确定性的 `os error 5`。
3. **看得见**（`server-rs\src\confinement.rs`）：后端启动自查 → 受限才写 `.run\confinement.json`、
   **正常就删**（免得过期标记一直亮着）；标记带 `pid`。
   `GET /api/system/confinement` 把同一份结论**连判据一起**交给界面
   （`serverImage` / `imageInsideProjectRoot` / `blockedPaths` / `remedy`），
   受限时 App 顶部常驻一条横幅（`lib\widgets\confinement_banner.dart`）——
   ⚠ 字段名只能说"**落在运行根目录里**"，别说"在工作区内"：发布包里根目录就等于包目录、
   并不是被框住的地方（那里 `restricted=false`）。
   这条沿用第 151/154 条的纪律：**界面上看不见的现象，用户和模型就只能靠猜**。
4. **写 input 的地方必须共用同一个入口**（`image_edit::write_or_defer`）：投放原图 / 遮罩 /
   **摆好的人物图**三处曾经只有前两处有 HTTP 退路，于是"人物图投进去了、摆好的那份没落地"，
   阶段二卡在"没拿到 personFilename"（2026-09-29 用户 bug）。
   直写被拒一律改让 **ComfyUI 自己写**（`/upload/image`），并按它回的名字回填结果字段
   （`LoadImage.image` 填的必须是它认得的那一串）。逐条清点见
   **[docs/permission-audit.md](docs/permission-audit.md)**（12 处写工作区外的点，逐条标注退路/闸门）。
   **同理，"写哪个 input"也只有一个真源**（2026-09-30 用户 bug：阶段二重启后仍被 `400 Invalid
   image file` 拒 —— 底图与人物图躺在**安装目录**的 `input` 里，实例读的却是**共享**那份）：
   起实例的 argv 由 `comfy_launch::start` 落档（`.run\comfyui-<port>.launch.json`），
   投放三件走 `image_edit::target_input_dir` —— **先问实例**（`/object_info/LoadImage` 的名单 ×
   候选目录），问不到才按"留档 → 配置 output 兄弟 → Desktop `settings.json` 的 `inputDir` → 盘探"回落。
   ⚠ 别拿**全名**判"调用方写过哪些参数"：`--input` 是 `--input-directory` 的 argparse 缩写、
   同一个 `dest`、**后写的赢** —— 比全名就会把我们补的那条排在用户那条后面、把它静默顶掉
   （用 `mcp::specified_flag`，并认 `--flag=value`）。两个环境变量名也别漂：
   脚本与老版本用 `COMFYHUB_COMFY_OUTPUT`，别再只认 `COMFYHUB_COMFY_OUTPUT_DIR`（`capture::comfy_output_env`）。

### 一个必须知道的连带影响

**受限会话里 `cargo test` 会红一片**（`os error 5`），因为测试二进制就在工作区里、带着 Low 标签。
所以用例**只许用 `crate::testpaths::scratch()`**（落在 `target\.scratch\`，工作区内），
**别写 `std::env::temp_dir()`**（第 1 节）。要复现"写不进去"就用同名目录占位，别改 ACL。

## 13. ⚠ 内容一律存在**自身文件夹**内（2026-09-29 用户定的总原则）

> **ComfyHub 自己产生的内容，不许散到 `%APPDATA%` / `%LOCALAPPDATA%` / `%TEMP%`。**

它同时是**落盘布局的总原则**，也是第 12 节那条不变式的**解法**：把整套 ComfyHub 放到工作区外
（发布包），既不受限、布局又和源码树完全一致。

| 内容 | 落点 | 一次性导入 |
| --- | --- | --- |
| 数据库 | `<根>\.db\comfyhub.db` | 无（曾是"零动作升级搬迁老库"，2026-09-29 随 MySQL 退役删除） |
| 产物 / 缩略图 / 附件 | `<根>\storage\` | 无 |
| 偏好 | `<根>\storage\app-support` | `%APPDATA%\…\shared_preferences.json` |
| MCP 运行时（便携 python + venv，几百 MB） | `<根>\storage\ai\mcp` | 无（**删掉了 `%LOCALAPPDATA%\ComfyHub\mcp` 那一档**） |
| comfy-cli 凭据 / 缓存 | `<根>\storage\ai\mcp\comfy-cli-home` | `%LOCALAPPDATA%\comfy-cli\{secrets.json,config.ini}` |
| 临时文件 / DevFS | `<根>\.run\tmp` | 无 |

**三条别踩**：

1. **别再用"探系统位置 → 写不进才退回"这个模式**（2026-09-28 那批修法全是这个形状）。
   判据不可靠（能不能写取决于进程映像位置，见第 12 节），而且方向本身就错了 ——
   静默换地方正是"哪份才是真的要看机器上装过什么"的来源。
   `Get-McpRuntimeDir` 现在**没有**"根不可写就退到 `%LOCALAPPDATA%`"那一档了；
   真写不进去就**如实报错**，让用户把包/项目放到可写的地方。
2. **导入是只读复制、且只做一次**（目标已存在就不动，否则会用旧文件盖掉用户新存的值）。
   受限进程**读**项目外是允许的，所以即便 errno 5 的现场导入也做得成。
3. **代价要如实说**：这些不再跨 ComfyHub 安装/跨机器共享。但 db 与 storage 本来就在各自文件夹里，
   这与"发布包是便携的"本来就是一致的 —— 别把它当回归。

改落盘位置时，`server-rs\src\mcp.rs` 与 `scripts\mcp-runtime.ps1` **必须一起改**：
`discover_runtime` 与 `Get-McpRuntimeDir` 是同一个真源的两份实现（AGENTS §7 的纪律）。
判据：装完发布包后从包内启动一次 App（或 `scripts\backend.ps1 start`），比对 `%APPDATA%\com.example`、`%LOCALAPPDATA\comfy-cli`、
`%LOCALAPPDATA\ComfyHub` 三个目录的文件数 —— **跑前后都不变**才算过。





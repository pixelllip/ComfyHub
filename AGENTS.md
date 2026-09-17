# AGENTS.md

给在这个仓库里干活的 AI / 自动化协作者看的注意事项。**功能说明、接口清单、排错表都在 [README.md](README.md)**，这里只记"怎么做才不踩坑"。

---

## 1. 只改前端时：用 debug 版 + 热重载，别每次完整构建

- **只动 `lib/` 下的 Dart 代码**（页面 / 组件 / 主题 / 状态）→ 用 debug 版，跑起来后按 `r` 热重载，1~2 秒见效：

  ```powershell
  pwsh -File scripts\dev-app.ps1        # 确保 MySQL + 后端在跑 → flutter run -d windows --debug
  ```

  它会先 `comfyhub.ps1 up -SkipBuild`，再 `flutter run -d windows --debug`；
  想自己来也可以直接 `flutter run -d windows --debug`（记得先 `$env:PUB_HOSTED_URL='https://pub.dev'`）。

- **别**为了看一个前端改动去跑 `scripts\autorun-app.ps1` / `flutter build windows` ——
  那是 Release 完整构建，几分钟起步（改 `windows/` 原生代码、改 `pubspec.yaml` 依赖、
  要出正式产物时才需要）。
- 改完前端至少过一遍：`flutter analyze` + `flutter test`（`PUB_HOSTED_URL=https://pub.dev`）。
  详情页大图、工作流弹窗这类交互改动，回归用例分别在 `test\zoomable_image_test.dart` 和
  `test\workflow_viewer_test.dart`，别只跑 `widget_test.dart`。
- **测试不是越多越好，但"能不能抓住回归"是唯一标准**：同一个事实写两遍（e2e 里
  "收到 run.completed" 每幕都断言一次、"parts 里有 tool_call" 又与后面"落库 parts 有序"重叠）、
  为了用掉 import 而写的占位用例、同一行为的两种写法 —— 这类**该删就删**（用户提过"减少一些
  已经通过、不太重要的测试项"）。判断方法只有一句：**删了以后出错还能不能被测试抓住**。
  真正的防线（零上游请求、审批顺序、越界写被拒、内联预算、滚动条与懒构建…）一条都不能删。
  本机实测的耗时（别凭感觉说"测试太慢"，2026-09-17 重新量过）：`flutter test` 168 例 ≈ 11~15s、
  `gradle test` 285 例 ≈ 5~11s（首跑要编译，稳定后更快）、`e2e-ai-tools-test.ps1` 64 项 ≈ 40s、
  `e2e-capture-test.ps1` / `e2e-submit-test.ps1` 各 ≈ 20~30s。
- 产物目录：debug 在 `build\windows\...\runner\Debug\`、Release 在 `...\Release\`；
  `autorun-app.ps1` / `comfyhub.ps1 up -WithApp` 启动 App 时**优先挑 Release**。

## 2. 本地服务必须"静默"启动（不弹命令行窗口）

MySQL + 后端由 App 启动时自动拉起，**不允许出现任何 cmd / 控制台窗口**。三件事都要记住：

1. **不能用 `Start-Process`** 起 mysqld / 后端：进程属于当前 PowerShell 的进程树，
   命令行一结束就可能被一起回收。要用 WMI `Win32_Process.Create` 脱离进程树。
2. **WMI 默认会给控制台程序分配一个可见的控制台窗口**（就是那个 cmd 黑框），
   所以必须传 `Win32_ProcessStartup{ ShowWindow = 0 }`（SW_HIDE）。
   公共实现是 `scripts\silent-process.ps1`，`mysql.ps1` / `server.ps1` 都 dot-source 它。
   - `CreateFlags = CREATE_NO_WINDOW(0x08000000)` 会被 WMI 拒绝（`ReturnValue=21`），不要用；
   - 传给 WMI 的 `CurrentDirectory` 绝不能是空串（`[string]` 参数没传值就是空串，不是 `$null`），
     否则同样 `ReturnValue=21` —— 这就是"手敲行、脚本不行"的那个坑；
   - `[wmiclass]`（System.Management）不可用时自动退到 `wscript` + 临时 `.vbs`：
     命令行先落成 UTF-16 文件，VBS 读出来交给 `cmd.exe /c "…"`（**不能**让 `Run` 直接跑
     "带引号的完整路径 + 参数"，实测 mysqld 会被悄悄丢掉），最后才退回裸 WMI（弹窗但服务能起）。
3. **验证"没弹窗口"要看枚举窗口，不能看 `MainWindowHandle`**：控制台窗口属于 `conhost.exe`，
   `Get-Process cmd | Select MainWindowHandle` 永远是 0，会给你假绿灯。
   正确做法是用 `EnumWindows` + `IsWindowVisible` 枚举可见顶层窗口，
   并且**边启动边以 ~50ms 轮询**，否则"闪一下就没"的窗口抓不到。
   现成的一条命令（返回 0 才算通过）：

   ```powershell
   pwsh -File scripts\check-silent-start.ps1 -Restart      # 从零走一遍 down → up
   ```

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
| JDK | **21~23**（后端用的 Gradle 8.12 不支持 24/25），`server.ps1` 会自己挑；可用 `COMFYHUB_JDK_HOME` 指定 |
| MySQL | 免安装 zip 版，默认 `D:\tools\mysql\mysql-8.4.3-winx64`；实例目录默认 `<项目>\.mysql`，可用 `COMFYHUB_MYSQL_DIR` / `mysql.ps1 move` 换 |
| pub 源 | 必须 `https://pub.dev`（国内镜像对个别包返回 424） |
| 脚本 | 一律 `pwsh`（PowerShell 7）；脚本里的 WMI / CIM 调用依赖 Windows |
| 端口 | MySQL `3307`、后端 `8080`、ComfyUI `8188` |
| Android 构建 | `android/` 走 AGP **9.4.0** + Gradle **9.6.0** + **AGP 内建 Kotlin**（`android.builtInKotlin=true`，`org.jetbrains.kotlin.android` 只钉版本、不 apply）；动 `android/` 之前先读 `docs/android-agp9-builtin-kotlin-migration.md` |
| Gradle 代理 | `~/.gradle/gradle.properties` 配了本机代理 `127.0.0.1:7890`：`dl.google.com` / `repo.maven.apache.org` **必须绕过代理**（`systemProp.http.nonProxyHosts`），而 `github.com` 直连不通、**必须走代理** |
| Kotlin 增量编译 | 本机 pub 缓存在 C:、工程在 D:，跨盘会炸（`this and base files have different roots`），所以 `android/gradle.properties` 里关掉了 `kotlin.incremental` |

## 5. 改脚本时的约定

- 脚本是**唯一入口**：App 的「启动 / 修复 / 重启 / 停止」按钮调的就是 `scripts\comfyhub.ps1`，
  命令行和 App 行为必须一致，不要在 Dart 侧另写一份启动逻辑。
- 新增脚本请放进 `scripts\`、用 `pwsh -File scripts\xxx.ps1 <动作>` 的形式，
  并在 README 的「脚本速查」表里登记。
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

---

## 7. 两套运行布局：源码树 vs 发布包（改路径解析前必读）

同一个脚本/App 要能在**两种布局**下跑，任何"找东西"的逻辑都得两边都认 ——
只按源码树的路径写，发布包（或者反过来）就会出现"文件明明在，它却说找不到"。

| | 源码树（开发） | 发布包（`scripts\pack-release.ps1` 的产物） |
| --- | --- | --- |
| 根目录怎么定 | 往上找 `scripts\comfyhub.ps1` | 同上（发布包的 `viewer.exe` 就铺在根目录，深度 0 就命中） |
| 后端启动脚本 | `server\build\install\comfy-hub-server\bin\` | `<根>\server\bin\` |
| `mysqld.exe` / `mysqladmin.exe` | `D:\tools\mysql\...` | `<根>\mysql\bin\` |
| Java | 本机 JDK 21~23 | `<根>\jre` |
| `viewer.exe` | `build\windows\...\runner\Release\` | `<根>\viewer.exe` |

踩过的坑：

- **发布包里必须跳过 Gradle**：`server.ps1` 看到 `<根>\server\bin\comfy-hub-server.bat` 就直接用它，
  不再 `gradle installDist`（包里既没有源码也没有 gradle）。Dart 侧同理，
  `backend_launcher.dart` 的 `_hasBuiltServer()` 要认两种路径，否则会给脚本加 `-SkipBuild` 加错。
- **发布包自带的 MySQL / JRE 必须排在"开发机绝对路径"前面**：`mysql.ps1` 的 `Resolve-MySqlHome`、
  `comfyhub.ps1` 的 `Resolve-MysqlBin`、`server.ps1` 的 `Resolve-MysqlAdmin` 和 `Resolve-Jdk`
  都是同一个顺序（显式参数 → 环境变量 → `<根>\...` → 本机 `D:\tools\...`）。
  漏掉一处，换台机器就会出现"库在跑但报数据库不可用"或者"找不到 mysqld.exe"。
- **`comfyhub.ps1 doctor` 会打印当前是哪种布局**（`运行布局: 发布包（便携式）/ 源码树`），
  路径类问题先看它。
- **发布包是便携式的**：可写数据（数据库 `<根>\.mysql`、产物 `<根>\storage`、日志 `<根>\.run`）
  都在包内，`packaging\manifest.json` 里 `runtimeLayout` 记着这份约定；改存放位置要先改那里和 README 9.2。
- **发布包默认"就地装配"**：`pack-release.ps1` 的 `-OutDir` 默认就是
  `build\windows\x64\runner\Release`，装完那个目录本身就是完整发布包（App 认根目录时往上找
  `scripts\comfyhub.ps1`，正好命中它自己的那一份）。三个连带后果别踩：
  ① `flutter clean` / 重新构建会把它整个清空（又变回只有 App），要重跑脚本；
  ② `-Clean` **不能**整个 `Remove-Item $OutDir`，否则把 `viewer.exe` 一起删了 ——
     只能清清单里那些 target（`server` / `mysql` / `jre` / `scripts` / `db` / `comfyui` …）+
     `packaging` / `BUILD-INFO.txt`；运行期数据（`.mysql` / `storage` / `.run`）**不要动**；
  ③ `flutter_release` 组件此时**源和目标是同一个目录**，必须跳过拷贝
     （`Copy-Item` 会报 "Cannot copy item to itself"）。
- 想要一份不随 `build\` 消失的长期副本，用 `-OutDir D:\dist\ComfyHub`。
- MySQL 分发目录**接近 1GB**，其中 `bin\mysqld.pdb` 一个就 368MB —— 裁剪规则写在清单的
  `prune` 字段里（glob 支持 `**`），拷贝约 400MB。改裁剪规则务必带上 `-Clean` 重装并看自检。
- **发布包只能带走 Java 和 MySQL 本体**：`pwsh` 和 VC++ 运行时带不走。缺 VC++ 运行时的时候
  `mysqld.exe` 只是起不来，原因只写进 `mysql-error.log`，命令行上只看到"启动超时"，非常难查。
  所以有 `scripts\runtime-deps.ps1`：`comfyhub.ps1` dot-source 它（跟 `silent-process.ps1` 一样），
  `up` 失败时按需提示、`doctor` 里逐项列出；清单在 `packaging\manifest.json` 的
  `runtimeRequirements`。**改启动流程时别把这块提示丢了。**
- **首次运行必须能自动建库**：`mysql.ps1` 的 `Do-Start` 发现 `<实例目录>\data\mysql` 不存在时，
  会先跑一次 `Do-Init -SkipSeed` 再启动（发布包解压出来没有数据目录，少了这一步首次启动必然失败）。
  自动初始化**故意不灌演示数据**（不能往用户库里塞演示提示词），只有手敲 `mysql.ps1 init` 才灌。

## 8. 运行时自动补齐 + 一个血泪教训

`scripts\ensure-runtime.ps1`（实现都在 `scripts\runtime-deps.ps1`，被 `comfyhub.ps1` dot-source）
负责"缺运行时自动装"：

- **Java**：`Ensure-JavaRuntime` 全自动 —— 下便携版塞进 `<根>\jre`，免安装免管理员，带 sha256。
  源顺序：Adoptium 官方(47MB,有官方 sha256) → 华为云 OpenJDK 21.0.2(190MB,sha256 硬编码在代码里)
  → Adoptium 重定向 → Microsoft OpenJDK。每个源先 `Test-UrlReachable`(Range 取 1KB) 探测再下。
  `comfyhub.ps1 up` 会调它；`COMFYHUB_NO_DOWNLOAD=1` 关闭。
- **pwsh / VC++**：系统级安装，走 winget；`up` 阶段不碰（免得启动弹 UAC），只由 `ensure-runtime.ps1` 显式装。

> **⚠ 变量的坑：永远不要给变量起名 `$home`、`$input`、`$host`、`$pid`、`$profile`、`$args`。**
> PowerShell 里自动变量**大小写不敏感**，`$home = ...` 会直接抛
> "Cannot overwrite variable HOME because it is read-only or constant"；
> 而更可怕的是**漏改了一处引用**：本仓库真的写出过
> `$javaHome = ...`（定义）却仍用 `$home`（引用）的代码，于是
> `Move-Item $home -Destination <解压目标>` 去搬 **`C:\Users\<用户>\` 整个用户目录**，
> 把用户主目录下的 10 个文件（`.gitconfig` / `.npmrc` / `.claude.json` …）搬进了发布包。
> 幸好目录被占用才没造成更大破坏。
> 所以 `Expand-JavaArchive` 里加了一条**安全闸**：只允许搬运 staging 目录内的路径，
> 否则当场抛异常（`$javaHome.StartsWith($stage, OrdinalIgnoreCase)`）。
> 写任何 `Move-Item` / `Remove-Item -Recurse` 之前，先确认目标是"算出来的、可验证的"路径。

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

- 工具清单**只有一个真源**：`server/src/main/kotlin/com/comfyhub/ai/tools/ToolRegistry.kt` 的 `tools` 列表。
  系统提示、`GET /api/ai/tools`、界面都从它派生 —— 加工具就在这里加一项 + 在 `ToolRegistryTest` 补用例，
  **别在别处再维护一份"可用工具"名单**。
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
- **长期记忆（`MemoryStore`）**：真源是 `<storage>\ai\memory.md`，一行一条，用户能直接手改；
  写入（AI 的 `remember` / 界面的保存）**超限一律报错，不许静默截断**（截断等于悄悄丢用户的话）；
  注入系统提示时截断到 `PROMPT_CHARS`，文件本身保留完整内容。
- **任何运行期代码都不许依赖 `%USERPROFILE%\.dsh`** —— 装我们项目的人可能根本没装 DSH
  （模型目录同理，见 `AiSeedCatalog` 的 classpath 冻结副本）。这条以前是"只在点导入时读一次"，
  现在连那个按钮都删了：DSH 与本项目再无运行期关系。

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
  改这段代码时守住两条：**绝不吞内容**（标记不闭合就把缓冲当正文放出去）、
  **绝不重复内容**（寒暄被逐字流式送达时不能放两遍）。`ConversationTitleTest` 盯的就是这两条。
- **ComfyUI 位置解析有两份实现**（`ComfyLocator.kt` 与 `scripts\comfy-path.ps1`），判据必须一致：
  源码形态 = `main.py` + `comfy/`；任何形态 = `output/` + `models/` 或 `input/`。
  改判据要同时改两边，否则会出现"App 说找到了、脚本说找不到"。
  探测**只读**，写配置必须由用户点「使用这个目录」（`POST /api/capture/locate/apply`）。
- **`capture_runs` 的 `empty` / `error` 是可重试状态**（`CaptureRepo.canReclaim`）：
  ComfyUI 的 `/history` 先出现记录、产物文件晚到是常态，把 `empty` 当终态就会出现
  "AI 说生成了、画廊里却没有"。只有 `success` 才是"不再重复收"的终态，改这段前先看
  `CaptureRunRetryTest`。
- 改完这一块请跑两条端到端（都不出网、不花钱）：
  `pwsh -File scripts\e2e-submit-test.ps1`（提交链路）与
  `pwsh -File scripts\e2e-capture-test.ps1`（捕获链路）。它们会临时改自动捕获配置、
  建一个 loopback Provider，跑完自己还原并清理数据；`-KeepData` 可以留着看效果。
  **别在跑完前 Ctrl+C**：清理在 finally 里，中断会留下 `e2e-submit-*` 的运行记录，
  而那会把下一次捕获 e2e 的 SHA-256 判重弄乱（实测踩过：表现为"新导入 0 个"）。

# AGENTS.md

给在这个仓库里干活的 AI / 自动化协作者看的注意事项。**功能说明、接口清单、排错表都在 [README.md](README.md)**，这里只记"怎么做才不踩坑"。

---

## 1. 只改前端时：用 debug 版 + 热重载，别每次完整构建

- **只动 `lib/` 下的 Dart 代码**（页面 / 组件 / 主题 / 状态）→ 用 debug 版，跑起来后按 `r` 热重载，1~2 秒见效：

  ```powershell
  pwsh -File scripts\dev-app.ps1        # 确保后端在跑（数据库是本地 SQLite，P2 起不用起库）→ flutter run -d windows --debug
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
  本机实测的耗时（别凭感觉说"测试太慢"，2026-09-23 重新量过）：`flutter test` 196 例 ≈ 23s、
  `gradle test` 597 例 ≈ 25s（热构建；首跑要编译）、`e2e-ai-tools-test.ps1` 111 项 ≈ 5~15s、
  `e2e-capture-test.ps1` / `e2e-submit-test.ps1` 各 ≈ 20~35s（后者含"工作流文件直接提交"那一幕）。
- 产物目录：debug 在 `build\windows\...\runner\Debug\`、Release 在 `...\Release\`；
  `autorun-app.ps1` / `comfyhub.ps1 up -WithApp` 启动 App 时**优先挑 Release**。
- **"我改了但界面没变"先查构建新鲜度，别急着怀疑代码**：App 是直接双击 / `Start-Process`
  拉起旧产物时，跑的还是上一次编的 Dart（Release 看 `data\app.so`、debug 看
  `data\flutter_assets\kernel_blob.bin`）。2026-09-17 "画廊/提示词里的视频预览图不显示"
  就是这么丢的：源码 20:11 改好，用户手里跑的是 18:50 编的内核 —— 只跑 `flutter test`
  永远发现不了（测试跑的是源码，用户看的是旧二进制）。所以**界面类修复必须
  `flutter build windows --debug`（或 `dev-app.ps1`）重启一次，再截图确认**；
  `comfyhub.ps1 doctor` 会分别报"该启动的 App"和"正在跑的 App"是不是旧构建，
  `up -WithApp` 选中/接管到旧构建时也会当场警告。

## 2. 本地服务必须"静默"启动（不弹命令行窗口）

后端由 App 启动时自动拉起（**数据库是 SQLite 文件，P2 起不用起数据库服务**），
**不允许出现任何 cmd / 控制台窗口**。三件事都要记住：

1. **不能用 `Start-Process`** 起后端：进程属于当前 PowerShell 的进程树，
   命令行一结束就可能被一起回收。要用 WMI `Win32_Process.Create` 脱离进程树。
   - ⚠ **唯一的例外（2026-09-28 新增的第 3 档 `direct-hidden`）**：**WMI 那一下被系统拒绝**时
     （实测现场就是用户报的 `flutter run -d windows` 拉不起后端：报
     `Invoke-CimMethod … Win32_Process -MethodName Create` **拒绝访问**；而**同一个进程树里**
     直接 `CreateProcess` 是通的 —— 判据是
     `node -e "require('child_process').spawnSync('cmd',['/c','echo OK'],{encoding:'utf8'})"`
     回 `0 OK`，所以被拒的是 WMI 本身，不是"整棵树被关住"），
     `silent-process.ps1` 会自动退到 `Start-Process -WindowStyle Hidden`
     （= .NET `Process.Start` → 直接 `CreateProcessW` + `STARTF_USESHOWWINDOW/SW_HIDE`，
     与 `ShowWindow=0` 效果一致，且不经过 WmiPrvSE）。
     **代价必须记住：它不脱离作业对象** —— 所以顺序永远是"能 WMI 就 WMI"，
     只有 WMI 被拒才退这一档；`Start-SilentProcess` **如实返回实际用的是哪一档**
     （`wmi-hidden` / `wmi-vbs` / `direct-hidden` / `wmi-plain`）。
     `$env:COMFYHUB_SPAWN_MODE = 'direct' | 'wmi'` 可强制走某一档（诊断用；也让这一档可测）。
     回归就是现成那条（实测：全程 0 个新窗口）：
     `$env:COMFYHUB_SPAWN_MODE='direct'; pwsh -File scripts\check-silent-start.ps1 -Restart`
   - ⚠⚠ **2026-09-29 定案：「WMI 被系统拒绝」不是机器的属性**（逐条量过：`wmi-hidden` /
     `wmi-vbs` / `direct-hidden` **三档都能起**，写同一个探针**全都是 OK** → spawn 档位与写权限**无关**）。
     当时那批"拒绝访问"的现场，成因是**后端映像在工作区内**（见**第 12 节**），WMI 被拒只是它的**表现**。
     **别再为"绕过 WMI"加新档位**，改这块之前先读第 12 节。
2. **WMI 默认会给控制台程序分配一个可见的控制台窗口**（就是那个 cmd 黑框），
   所以必须传 `Win32_ProcessStartup{ ShowWindow = 0 }`（SW_HIDE）。
   公共实现是 `scripts\silent-process.ps1`，`server.ps1` 等脚本都 dot-source 它。
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
| JDK | **21~23**，但**只有回退旧 Kotlin 后端 / 跑 Gradle（含 `android/`）时才要**；Rust 后端不需要 JVM。`server.ps1` 只在真要用它时才解析（懒解析），可用 `COMFYHUB_JDK_HOME` 指定 |
| MySQL | **已退役（2026-09-29）**：数据库自 P2 起是 SQLite 单文件 `<根>\.db\comfyhub.db`，`mysql.ps1` 与老库搬迁都不存在了；老机器上残留的 `D:\tools\mysql\`、`<项目>\.mysql\` 只是历史目录，不再被读取 |
| pub 源 | 必须 `https://pub.dev`（国内镜像对个别包返回 424） |
| 脚本 | 一律 `pwsh`（PowerShell 7）；脚本里的 WMI / CIM 调用依赖 Windows |
| 端口 | 后端 `8080`、ComfyUI `8188`（原 MySQL `3307` 已随 2026-09-29 MySQL 退役，不再有任何用途） |
| Android 构建 | `android/` 走 AGP **9.4.0** + Gradle **9.6.0** + **AGP 内建 Kotlin**（`android.builtInKotlin=true`，`org.jetbrains.kotlin.android` 只钉版本、不 apply）；动 `android/` 之前先读 `docs/android-agp9-builtin-kotlin-migration.md` |
| Gradle 代理 | `~/.gradle/gradle.properties` 配了本机代理 `127.0.0.1:7890`：`dl.google.com` / `repo.maven.apache.org` **必须绕过代理**（`systemProp.http.nonProxyHosts`），而 `github.com` 直连不通、**必须走代理** |
| 后端出网代理 | **JVM 默认无视 Windows 系统代理**（一律 DIRECT）：本机 `civitai.com` 这类目标直连不通、只有 `127.0.0.1:7890` 通，所以 `server/build.gradle.kts` 的 `applicationDefaultJvmArgs` 带着 `-Djava.net.useSystemProxies=true`，出网客户端再统一挂 `com.comfyhub.ai.OutboundProxy` 的选择器（**公网走代理、本机/内网强制直连**——打开系统代理后 Java 会把 `127.0.0.1:8188` 也交给代理，会打死 ComfyUI）。自己用 `java -jar` 起后端时要手动补这个参数，或设 `COMFYHUB_HTTPS_PROXY` |
| Kotlin 增量编译 | 本机 pub 缓存在 C:、工程在 D:，跨盘会炸（`this and base files have different roots`），所以 `android/gradle.properties` 里关掉了 `kotlin.incremental` |

> **踩过的具体坑（50+ 条：现象 → 原因 → 处理）统一记在 [`docs/pitfalls.md`](docs/pitfalls.md)**。
> 它原来是 README 第 12 节，2026-09-17 按用户要求单独拆出来（表太长，把 README 的"怎么用"挤没了）。
> 撞到"明明改了却没生效""脚本手敲行、跑起来不行"这类问题，先去那张表里搜现象。

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
| 后端二进制 | `server-rs\target\release\comfyhub-server-rs.exe` | `<根>\server\comfyhub-server-rs.exe` |
| 数据库 | `<根>\.db\comfyhub.db`（SQLite 单文件 + 它的 `-wal` / `-shm`） | 同一份约定（`<根>\.db\`，装配时预建空目录） |
| Java | **只有回退旧 Kotlin 后端时才要**（JDK 21~23） | **不再随包携带**（P4 收尾起后端是 Rust，不需要 JVM） |
| 旧 Kotlin 后端（已退役） | `legacy\server\`（留档） | 不装 |
| `viewer.exe` | `build\windows\...\runner\Release\` | `<根>\viewer.exe` |

踩过的坑：

- **"用哪个后端"只有一个真源**：`scripts\backend.ps1` 的 `Resolve-Backend`
  （`-Backend auto|rust|kotlin` > `COMFYHUB_BACKEND` > auto；auto 只看 Rust 二进制在不在）。
  `server.ps1` 与 `comfyhub.ps1 doctor` **都读它那一份** —— 各写一份就会出现
  "doctor 说 Kotlin 在跑、server.ps1 去停 Rust"（2026-09-26 切流时踩过）。
- **JDK 必须懒解析**（`Get-Jdk`）：脚本开头就 `Resolve-Jdk` 会在**没装 JDK 的机器上直接抛**，
  而那种机器正是发布包要支持的 —— Rust 后端根本不需要 JVM。
- **P4 收尾（2026-09-26）起 Kotlin 已退役**：源码在 `legacy\server\`（`.gitignore` 里
  `/legacy/server/build`、`/legacy/server/storage` 照样忽略），`pack-release.ps1` 不再有 `jre` / jar 组件。
  要回退旧后端得先把 `legacy\server` 放回原位自己构建 —— 这条路只在灰度期有意义。
- **`db::open_or_create` 是"第一次运行能起来"的关键**：以前缺库直接 `exit 3`（"先让 Kotlin 起一次库"），
  在发布包里等于**永远起不来**。现在库缺就自己建（DDL 编译期 `include_str!` 嵌在二进制里）。
- **`comfyhub.ps1 doctor` 会打印当前是哪种布局**（`运行布局: 发布包（便携式）/ 源码树`）
  与**后端实现/构建新鲜度**，路径类问题先看它。
- **发布包是便携式的**：可写数据（数据库 `<根>\.db`、产物 `<根>\storage`、日志 `<根>\.run`）
  都在包内，`packaging\manifest.json` 里 `runtimeLayout` 记着这份约定；改存放位置要先改那里和 README 9.2。
- **发布包默认"就地装配"**：`pack-release.ps1` 的 `-OutDir` 默认就是
  `build\windows\x64\runner\Release`，装完那个目录本身就是完整发布包（App 认根目录时往上找
  `scripts\comfyhub.ps1`，正好命中它自己的那一份）。三个连带后果别踩：
  ① `flutter clean` / 重新构建会把它整个清空（又变回只有 App），要重跑脚本；
  ② `-Clean` **不能**整个 `Remove-Item $OutDir`，否则把 `viewer.exe` 一起删了 ——
     只能清清单里那些 target（`server` / `scripts` / `db` / `comfyui` …）+ `jre` / `mysql`（P4/P2 之前的老组件）
     + `packaging` / `BUILD-INFO.txt`；运行期数据（`.db` / `storage` / `.run`）**不要动**；
  ③ `flutter_release` 组件此时**源和目标是同一个目录**，必须跳过拷贝
     （`Copy-Item` 会报 "Cannot copy item to itself"）。
- 想要一份不随 `build\` 消失的长期副本，用 `-OutDir D:\dist\ComfyHub`。
- **P2 起发布包不再携带 MySQL**（原来 `mysql/` 约 400MB，裁剪规则在清单的 `prune` 字段里）：
  数据库是 SQLite，驱动自带引擎。**2026-09-29 起 MySQL 彻底退役**：连"搬一次旧数据"的搬迁
  通道与 `mysql.ps1` 都删了，`runtime-deps.ps1` 的 MySQL 与 **VC++ 运行时**体检项也一并删除
  （VC++ 当初只为 `mysqld.exe` 存在；实测后端 Rust exe 不引用 `vcruntime140.dll`、CRT 是静态链接，
  只用 Win10/11 自带的 `api-ms-win-crt`，所以整项删掉是安全的）。
- **`runtime-deps.ps1` 的 `Required` 字段决定"缺了算不算问题"**：`Show-RuntimeReport` /
  `Show-RuntimeFailureHints` 只把 **必需项** 缺失当红。
  改启动流程时别把这块提示丢了（`comfyhub.ps1` dot-source 它，跟 `silent-process.ps1` 一样）。
- **首次运行必须能自动建库**（P2 起是后端自己建 SQLite 文件，`db::open_or_create` 负责）。
  **"零动作升级搬迁老库"已于 2026-09-29 随 MySQL 彻底退役而删除** —— `up` 不再读 `.mysql\`，
  `mysql.ps1` / `server.ps1 migrate` 这条路不存在了；老库目录原样留着（确认不要了再自己删），
  搬迁链路的老规则（逐表核对行数与行哈希、失败不拦 up）随通道一起成为历史。
  自动建的库**故意不灌演示数据**（不能往用户库里塞演示提示词）。

## 8. 运行时自动补齐 + 一个血泪教训

`scripts\ensure-runtime.ps1`（实现都在 `scripts\runtime-deps.ps1`，被 `comfyhub.ps1` dot-source）
负责"缺运行时自动装"：

> **总原则（2026-09-21 用户定）：检测优先，安装只作兜底。** "环境有 python 你的脚本就不用再装了，
> 同理 jdk、mcp 也是。" —— 所以三条链都是**先找机器上已有的**，找不到才下便携版；
> 判据与来源都要**能看出是哪一档**（`doctor` 会打出来）。改这三条链时别把顺序倒回去。

- **Java**：`Ensure-JavaRuntime` —— 先用机器上已有的 ≥21（显式参数 → `COMFYHUB_JDK_HOME` →
  包内 `<根>\jre` → **`JAVA_HOME`** → 常见 JDK 安装位置 → `PATH`），**没有才**下便携版塞进 `<根>\jre`，
  免安装免管理员，带 sha256。
  源顺序：Adoptium 官方(47MB,有官方 sha256) → 华为云 OpenJDK 21.0.2(190MB,sha256 硬编码在代码里)
  → Adoptium 重定向 → Microsoft OpenJDK。每个源先 `Test-UrlReachable`(Range 取 1KB) 探测再下。
  `comfyhub.ps1 up` 会调它；`COMFYHUB_NO_DOWNLOAD=1` 关闭（**这条现在在 `Ensure-JavaRuntime` 内部也认**，
  不只是 `up` 里那一处）。
- **MCP（Python）**：`scripts\mcp-runtime.ps1` 四档 —— ① 已装好的（`state.json` / `venv` / `python`）→
  ② 借机器上**已经装了 comfy-mcp + comfy-cli** 的 Python（一个字节都不装）→ ③ 借机器上的 Python 建
  **venv**（`<根>\storage\ai\mcp\venv`，**不污染用户的全局 site-packages**）→ ④ 真的没有才下 embeddable
  便携版。用了哪一档记在 `state.json` 的 `pythonSource`，`doctor` 显示 `system` / `venv` / `portable`。
  ⚠ **venv 的解释器就在 `Scripts\` 里**，所以"pip 启动器在哪个目录"必须分两种情况
  （`Get-PythonScriptsDir` / Kotlin 的 `scriptsDirOf`）—— 一律 `父目录\Scripts` 会拼出
  `venv\Scripts\Scripts`，表现是"明明装好了却报缺 comfy-mcp.exe"。
- **pwsh**：系统级安装，走 winget；`up` 阶段不碰（免得启动弹 UAC），只由 `ensure-runtime.ps1` 显式装。
  **VC++ 与 MySQL 体检项已于 2026-09-29 随 MySQL 彻底退役而删除**（VC++ 当初只为 `mysqld.exe` 存在，
  实测后端 Rust exe 不引用 `vcruntime140.dll`、CRT 是静态链接，只用系统自带的 `api-ms-win-crt`）——
  `Get-RuntimeChecks` 现在只剩 pwsh（必需）与 Java 21（`Required = $false`，只对回退旧 Kotlin 后端有意义）。

> **⚠ 参数改名的坑（2026-09-21 踩到）：非高级函数（没写 `[CmdletBinding()]`）收到不存在的命名参数时
> 不报错，而是丢进 `$args`。** `mcp-runtime.ps1` 把参数从 `$Proxy`/`$Force` 改成 `$McpProxy`/`$McpForce`
> （为了躲 dot-source 的变量覆盖，见该文件头的注释）之后，内部一堆 `-Proxy $McpProxy` / `-Force:$McpForce`
> 就**静默失效**了 —— 没报错、没异常，只是代理没传过去、强制重装没生效。所以：改名后**必须把调用点一起改**，
> 而且给命令行用的名字要留 `[Alias('Proxy')]` / `[Alias('Force')]`。

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

## 12. ⚠ 写不进工作区外：限制**按映像位置 + 继承**判（2026-09-29 定案）

`comfy_start` 报 `COMFY_WRITE_BLOCKED`、`comfy_use_media` 报 `COMFY_INPUT_FAILED … os error 5`、
MCP 任何工具报 `UnexpectedToolError` —— 同一批症状、同一个根因。**动手改之前先读完这一节**，
它推翻过 2026-09-28 的两处结论（现象没错，**处方错了**）。

### 量出来的判据

| 怀疑对象 | 实测 | 结论 |
| --- | --- | --- |
| 目录 ACL | 诊断脚本报 `NOT_THIS_CLASS`（无 deny ACE、无 package ACE，`WRITE_DAC`/`WRITE_OWNER` 都在） | 无关 |
| 路径配错 / 用户身份 / Defender | 同用户同目录 `powershell` 写得进；Defender 无受控文件夹规则 | 无关 |
| **映像在工作区内** | **同一个二进制逐字节相同：工作区内 → 写 `%TEMP%`、`D:\Comfy-Desktop\…` 全 `os error 5`；`copy` 到 `%TEMP%` → 哪儿都能写** | ✅ 就是它 |
| 从哪个会话 / 终端起 | 双击一样被限 | 无关（限制在令牌里） |
| spawn 档位 | `wmi-hidden` / `wmi-vbs` / `direct-hidden` 三档写同一个探针**都是 OK** | 无关（见第 2 节那条 ⚠⚠） |

**两条机制并存**：`按映像位置`（后端 exe 在工作区内就必然被限）+ `按继承`
（受限父进程拉起的、工作区外的 python 照样被拒，`tests/tmp-inherit-probe.rs` 量过）。
所以"换个 ComfyUI 自带的 python 就好"是错的 —— **后端自己就写不了**，起手写实例 `temp/` 就死。

### 退路（以前漏了，现在有）

**正解是发布包** —— 它同时满足第 12 节的不变式与第 13 节"内容在自身文件夹内"：

```powershell
pwsh -File scripts\pack-release.ps1 -OutDir D:\dist\ComfyHub
pwsh -File D:\dist\ComfyHub\scripts\comfyhub.ps1 up
```

发布包的后端在 `<包根>\server\`、数据在 `<包根>\.db\` / `storage\` / `.run\`，**内容一点没外溢**。
`Resolve-RustExe`（`scripts\backend.ps1`）**优先认 `COMFYHUB_SERVER_EXE`** ——
那三个默认候选全在项目根下，所以**源码树布局下后端必然被限**，这不是配置错了。
实测：装完发布包跑 `up` → 留证文件消失、不受限。

⚠ **别把 `COMFYHUB_SERVER_EXE` 指到 `%LOCALAPPDATA%` 之类**（第 13 节：内容一律在自身文件夹内）。
它只该指向"**已经装配好的发布包**里的那个 exe"，或者你自己另外放的一整份安装。

### 三条纪律

1. **闸门留着**（`comfy_launch::ensure_writable`）：白起一个 100% CPU 卡死的实例，比如实拒绝糟得多。
   但**文案必须说对**：先讲真因（按映像位置判），再给上面那条修法。
   **别再写"没有退路 / 只能让用户双击 ComfyUI Desktop"** —— 双击确实能work，但不是唯一那条。
   **也别建议往 `%LOCALAPPDATA%` 之类拷后端**（见第 3 条：内容一律在自身文件夹内）。
2. **别把探针的 `Err` 吞成 `bool`**（`confinement::probe_dir` 返回 `io::Result` 就是为此）：
   只报"哪个目录"不报"报什么错"，就会重演 2026-09-28 那次"现象对、处方错"。
   用例里也不许拿"这台机器哪个目录不可写"当判据 —— 用同名目录占位造出确定性的 `os error 5`。
3. **看得见**（`server-rs\src\confinement.rs`）：后端启动自查 → 受限才写 `.run\confinement.json`、
   **正常就删**（免得过期标记一直亮着）；标记带 `pid`，`doctor` 还要比对"是不是当前在跑的后端"。
   这条沿用第 151/154 条的纪律：**doctor 探不到的现象，就会一直靠猜**。

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
判据：装完发布包后跑一次 `up`，比对 `%APPDATA%\com.example`、`%LOCALAPPDATA\comfy-cli`、
`%LOCALAPPDATA\ComfyHub` 三个目录的文件数 —— **跑前后都不变**才算过。





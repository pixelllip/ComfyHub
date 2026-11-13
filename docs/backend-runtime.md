# 后端运行时：进程、路径、落盘与装配

> 这是 `AGENTS.md` 第 5 节「后端规范」的细则。全局红线见 [`../AGENTS.md`](../AGENTS.md)；受限环境的症状与处方见 [`confinement-integrity.md`](confinement-integrity.md)。

## 1. 本地服务必须静默启动（不弹命令行窗口）

- 后端由 App 自己起（`lib\core\backend_launcher.dart` **直接执行 exe**）：Dart 的 `Process.start` 在 Windows 上**自带 `CREATE_NO_WINDOW`** —— 这就是"不弹黑框"的全部原因，别再引入 WMI / wscript 那一套。
- **"谁起的谁关"**：正常关窗 → `BackendLauncher.releaseOnExit()` kill 自己起的那一个；被任务管理器 / 崩溃强杀 → 后端启动时带的 **`--watch-stdin`** 生效（stdin 写端只有 App 持有，App 一死读到 EOF 优雅退出）。回归 `server-rs/tests/watch_stdin_test.rs`。
- 脚本旁路 `scripts\backend.ps1 start`（给 e2e / dev）用 `Start-Process -WindowStyle Hidden`，**不带 `--watch-stdin`**（Start-Process 给的不是管道，带了后端会当场退出），也不脱离进程树。
- **验证"没弹窗口"要枚举窗口**：控制台窗口属于 `conhost.exe`，`Get-Process cmd | Select MainWindowHandle` 永远是 0（假绿灯）。正确做法 `EnumWindows` + `IsWindowVisible` 按 ~50ms 轮询；**别用 `Start-Process` 起后端来测这条**（它本来就是"不保证无窗口"的那条路）。

## 2. 改脚本时的约定

- **App 的启动逻辑只有一份，在 Dart 里**（`lib\core\backend_launcher.dart` + `backend_paths.dart`），所以别在 `.ps1` 里另写一份"怎么起后端"。
- 新增脚本前先问一句"这件事能不能不进 `scripts\`"；真要加就用 `pwsh -File scripts\xxx.ps1 <动作>` 的形式，并在 README「脚本速查」里登记。
- **`scripts\backend.ps1` 是唯一允许碰后端的脚本**（给 e2e / dev），改一边必须改另一边（见 §3 的镜像关系）。
- PowerShell：`& script.ps1` 的 **stdout 会成为表达式的返回值**，在 `$ok = & other.ps1 …` 这种地方必须 `| Out-Null`，否则返回值被输出数组污染、判断永远为真。

## 3. 两种运行布局：源码树 vs 发布包

"找东西"的逻辑只有一份真源：**`lib\core\backend_paths.dart`**；`scripts\backend.ps1` 是它在脚本世界的**镜像**（只服务 e2e / dev），**改一边必须改另一边**。

| 找什么 | 源码树（开发） | Release 产物 / 发布包 |
| --- | --- | --- |
| 运行根目录 | 往上找 `server-rs\Cargo.toml`（或 `db\schema.sqlite.sql`、`backend\comfyhub-server-rs.exe`） | `viewer.exe` 所在目录本身 |
| 后端 exe | `<根>\server-rs\target\{release,debug}\comfyhub-server-rs.exe` | `<viewer.exe 同级>\backend\comfyhub-server-rs.exe` |
| `viewer.exe` | `build\windows\...\runner\{Debug,Release}\` | `<根>\viewer.exe` |
| 数据库 / 产物 / 端口 | `<根>\.db\` / `<根>\storage\` / `<根>\.run\api.json` | 同一份约定 |
| Java | **不需要** | **不随包携带** |

- **后端 exe 的候选顺序只有一个真源**：`serverExeCandidates()` = `$env:COMFYHUB_SERVER_EXE` > `<exe 同级>\backend\` > `<根>\backend\` > `<根>\server\`（旧布局） > `server-rs\target\{release,debug}\`。`COMFYHUB_SERVER_EXE` **永远最高优先**，它是"把后端映像挪出受限区"的唯一开关。
- **根目录判据必须同时认两种布局**（任一命中即可），否则发布包里会出现"文件明明在，它却说找不到"。
- **`infra::db::open_or_create` 是"第一次运行能起来"的关键**：库缺就自己建（DDL 编译期 `include_str!` 嵌在二进制里）。别再退回"缺库就退出"。
- **发布包是便携式的**：可写数据（`.db` / `storage` / `.run`）都在包内，`packaging\manifest.json` 的 `runtimeLayout` 记着这份约定；改存放位置要先改它。
- **Release 产物默认"就地装配"**（`windows\CMakeLists.txt` + `windows\cmake\bundle_backend.cmake`），三个连带后果别踩：① `flutter clean` 会把它整个清空，重跑一次 release 构建即可；② `pack-release.ps1 -Clean` **不能**整个 `Remove-Item $OutDir`（会把 `viewer.exe` 一起删了），只清清单里的 target + 老组件（`server` / `jre` / `mysql`），运行期数据（`.db` / `storage` / `.run`）**不要动**；③ `flutter_release` 组件此时**源和目标是同一个目录**，必须跳过拷贝。
- **`cargo build --release` 在后端正在跑时必失败**（Windows 锁着运行中的 exe，报"拒绝访问 os error 5"）：先 `pwsh -File scripts\backend.ps1 stop`。
- 旧 Kotlin 后端（`legacy/`）**已整体删除**，要对照行为只能从 git 历史取：`git log --diff-filter=D -- legacy` → `git show <那次>^:legacy/server/...`。

### 3.1 构建产物与新鲜度

- 产物：debug / release 在 `build\windows\...\runner\{Debug,Release}\`。**debug 产物里没有 `backend\`**，用的是 `server-rs\target\{release,debug}\` 那份 —— 改了 Rust 只要 `cargo build`，不用重建 App。
- **"我改了但界面没变"先查构建新鲜度**，别怀疑代码：跑的可能还是旧产物（Release 看 `data\app.so`、debug 看 `data\flutter_assets\kernel_blob.bin`）。界面类修复必须 `flutter build windows --debug`（或 `dev-app.ps1`）重启一次再截图确认；后端看 `scripts\backend.ps1 status`。

## 4. 数据目录可以被用户搬走

- 库与 `storage\` 的落点由 `<根>\data-dir.json`（`{"dataDir":"<目录>"}`）决定，没有它就是运行根目录。`--db` / `--storage` 跟着它走；`.run\` / `backups\` / `comfyui\` / `app-support\` 永远按运行根目录算（端口文件不能跟着数据走，脚本与捕获扩展都读它）。
- 判据只有一份：`lib\core\backend_paths.dart` 的 `dataDirOf()`，`scripts\backend.ps1` 的 `Resolve-DataDir` 是镜像，e2e 读库也要过它。
- **产物层别硬拼 `storage`**：`resolvedStorageDirOf()` 按磁盘实际布局认（标准 / 旧版双层 / 扁平），迁移 `lib\core\data_dir_migration.dart` 写回标准布局（**先停后端 → 备份目标旧库 → 复制 → 逐文件校验 → 最后落指针**，旧目录默认保留、搬完问一次）。界面上是「设置 → 数据目录」。
- **数据目录的"实际在哪"要先校正再信（2026-11-10）**：`dataDirOf()` 背后是 `resolveDataDir()`，**先认身份、再谈路径** —— ① 库带身份（`<数据目录>\.db\library.json` 的 `id`），偏好记着上次用的是哪份、在哪（`SettingsStore.lastLibrary`），指针那一级不是它时按记录打开（**复制出去的构建照样打开你的库**）；② 指针里是另一份**有身份**的库 → 尊重用户指定，但界面要提醒（`pointerHasOtherLibrary`）；③ 指针目录不在了 → 回运行根目录，**绝不**建空库；④ 没有记录就用指针（或运行根）那份，同树里别的库只**列出**（`DataDirResolution.otherCandidates` → 设置页「用这一份」），**绝不按大小 / 时间 / 层级猜**。
- 结论与原因一起交给界面（`BackendLauncher.dataDirInfo` → `DataDirNoticeBanner` + 设置页），别处不再判。设置页**只剩两个动作**：「搬到别处…」与「搬回默认位置」，选完立即开搬（进度弹窗说清从哪到哪、多少文件、可取消），**技术按钮一律不要加回来**。文案按 [`ui-conventions.md`](ui-conventions.md) §4，但不出现 `storage` / `产物` / `指针` / `库`，说"对话、图片和角色卡"。横幅只报真异常（换了一份 / 上次那份找不到 / 位置失效 / 缺图片文件夹），"还发现一份数据"只在设置页。
- **产物目录里不许有运行期数据**：`bundle_backend.cmake` 把 `.db` / `storage` / `.run` / `app-support` 挪进 `_leftover-data\`（**只挪不删** —— 那份残留里有过用户唯一的 30 张图），`pack-release` 跳过它们并把 `data-dir.json` 带进产物，否则复制出去的构建会拿构建残留的旧库当"用户的库"（见 [`pitfalls.md`](pitfalls.md)）。

## 5. 两份库可以合并

`comfyhub-server-rs.exe --merge-library <源数据目录> [--merge-library …] [--media-from <目录>] [--db <目标库>] [--storage <目标 storage>] [--dry-run]`（实现 `server-rs\src\infra\library_merge.rs`）。纪律：

1. **只增不减** —— 源库只读打开，目标库先 `VACUUM INTO <库文件>\backups\pre-merge-<时间>.db`，所有写入一个事务，`PRAGMA foreign_key_check` 不干净就整体回滚；
2. **判重靠内容不靠 id**（`tags.normalized` / `media_assets.sha256+original_name` / `prompts.source+source_ref` / `capture_runs.run_key` / 会话与运行的 UUID），整数 id 被占就改号并跟着重写外键（`tags.id = 1` 在两份库里是不同标签）；
3. **文件只补缺不覆盖**（同名不同内容时保留目标那份并如实报出来；缩略图是派生数据，缺了就补）；
4. **配置不搬**（`app_settings` / `ai_providers` / `ai_models` / `_sync_log` / `sync_clients` 一律不动）；
5. 源库自己就有悬空引用时（老数据把 `fc_...` 塞进 `ai_message_parts.tool_call_id`）**保留行、置空引用**并报出来 —— 行里的正文 / 结果 JSON 不能丢。

合完用 `--dry-run` 再跑一遍应当"新增 0"（幂等）。

## 6. 端口与 base URL

- **端口只有一个真源**：后端绑好之后自己写 `<根>\.run\api.json`（写者是 `api_port_file.rs`）；App（`settings_store.dart`）、脚本（`api-port.ps1`）、捕获扩展（`capture_core.py`）**都只读它**，谁也不许自己编端口。`--port 0` 是刻意的：写死端口会撞"被占 / 落在 Windows 保留段"，真实端口要从 listener 读回来。
- ⚠ **"要不要听用户的"只有一条判据**：`SettingsStore.isLocalBackendUrl()` —— **回环地址永远跟着端口文件走**（哪怕它先前被写进过偏好），只有**指向别的机器**的地址才算用户覆盖。**别退回"偏好里有值 = 用户手填过"**：端口是 `--port 0` 自动挑的（重启就换），那条判据会把自动发现的地址存成"用户配置"，从此界面对着死端口等满 60 秒超时。回归 `test\settings_base_url_test.dart`。

## 7. 内容一律存在自身文件夹内

> **ComfyHub 自己产生的内容，不许散到 `%APPDATA%` / `%LOCALAPPDATA%` / `%TEMP%`。**

它同时是落盘布局的总原则，也是 [`confinement-integrity.md`](confinement-integrity.md) 处方 2 的解法：包放工作区外既不受限、布局又与源码树一致。代价是不再跨安装 / 机器共享，但 db 与 storage 本来就在各自文件夹里，别当回归。

| 内容 | 落点 | 一次性导入 |
| --- | --- | --- |
| 数据目录覆盖（库 + `storage` 被搬走时） | `<根>\data-dir.json` | 无 |
| 数据库 | `<数据目录>\.db\comfyhub.db`（默认 `<根>\.db\`） | 无 |
| 库的身份 | `<数据目录>\.db\library.json`（`{"id":"<uuid>"}`，跟着库走；"上次用哪份库"记在偏好里） | 无 |
| 库备份（`VACUUM INTO` 出来的 .db） | `<根>\backups\` | 无 |
| 产物 / 缩略图 / 附件 | `<数据目录>\storage\`（默认 `<根>\storage\`） | 无 |
| 偏好 | `<根>\app-support` | `<根>\storage\app-support`（旧版）与 `%APPDATA%\…\shared_preferences.json` |
| MCP 运行时（便携 python + venv） | `<数据目录>\storage\ai\mcp` | 无 |
| comfy-cli 凭据 / 缓存 | `<数据目录>\storage\ai\mcp\comfy-cli-home` | `%LOCALAPPDATA%\comfy-cli\{secrets.json,config.ini}` |
| 临时文件 / DevFS | `<根>\.run\tmp` | 无 |

1. **别再用"探系统位置 → 写不进才退回"这个模式**：判据不可靠（取决于进程映像位置），方向也错 —— 静默换地方会让"哪份才是真的"取决于机器上装过什么。写不进去就**如实报错**，让用户把包 / 项目放到可写的地方。
2. **导入是只读复制、且只做一次**（目标已存在就不动，否则会用旧文件盖掉用户新存的值）。受限进程**读**项目外是允许的，所以 errno 5 的现场导入也做得成。

改落盘位置时，`server-rs\src\tools\mcp.rs` 与 `scripts\mcp-runtime.ps1` **必须一起改**（`discover_runtime` 与 `Get-McpRuntimeDir` 是同一真源的两份实现）。判据：从包内启动一次 App，比对 `%APPDATA%\com.example`、`%LOCALAPPDATA%\comfy-cli`、`%LOCALAPPDATA%\ComfyHub` 的文件数，跑前后都不变才算过。

## 8. 错误不静默的具体落点

`catch (_)` 的判据只有一条：**这次失败会不会被当成成功**。会的那几条必须如实说出来（2026-11-07 审过一轮）：

- `lib/core/media_cache.dart` 的 `lastScanError` / `lastClearFailures`（缓存扫描与清理）；
- `lib/core/backend_paths.dart` 的 `dataDir.json` 日志（路径探测）；
- `lib/core/ai_workspace_store.dart` 的 `comfyJobsError`（后台轮询）；
- 回滚指针那一路（迁移失败要能看出来）。

只有纯 best-effort 的（清残片、关客户端、删临时文件）才可以静默。**别为了"代码整洁"把前一类也吞成 `catch (_)`。**

## 9. Rust 用例的落盘位置

**Rust 用例要落盘只许 `testpaths::scratch()`**（落点 `<crate>\target\.scratch\`，`tests/` 里是 `comfyhub_server_rs::infra::testpaths::scratch()`），**别用 `std::env::temp_dir()`** —— 工作区里编出来的测试二进制带 Low 标签（见 [`confinement-integrity.md`](confinement-integrity.md)），受限会话里必然 `os error 5`。要造"写不进去"就用**同名目录占位**，别改 ACL。

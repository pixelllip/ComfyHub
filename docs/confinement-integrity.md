# 受限环境：写不进工作区外的真因是完整性标签

> 这是 `AGENTS.md` 第 5 节「后端规范」里受限那一条的细则。落盘布局见 [`backend-runtime.md`](backend-runtime.md)。

`comfy_start` 报 `COMFY_WRITE_BLOCKED`、`comfy_use_media` 报 `COMFY_INPUT_FAILED … os error 5`、MCP 工具报 `UnexpectedToolError`、`cargo test` 一批 `PermissionDenied` —— **同一批症状、同一个根因**。

## 机理

① 代码沙箱给**工作区目录树**打 `Mandatory Label\Low Mandatory Level:(OI)(CI)(NW)`；② 在工作区里构建出来的 exe（`comfyhub-server-rs.exe`、`viewer.exe`、测试二进制…）**继承了 Low 标签**；③ Windows 创建进程取 **`min(父进程令牌, 映像标签)`**；④ **Low 写不了 Medium 对象**（工作区外默认无标签 = Medium）⇒ `拒绝访问 (os error 5)`。

两条推论：**只把 exe 的标签清回 Medium（位置不动），后端立刻哪儿都能写**；但链路上任何一环是 Low，子进程照样掉回 Low —— `viewer.exe` 是 Low 时它起的后端一定是 Low，**连"修标签"都做不动**。已排除 ACL、路径、用户身份、Defender、启动方式 —— 只看**映像标签**。

## 三条处方

1. **构建时归一标签**：`windows\cmake\bundle_backend.cmake` 在 release 装配末尾对 `viewer.exe` 与 `backend\comfyhub-server-rs.exe` 各跑一次 `icacls <exe> /setintegritylevel Medium`（失败只警告、不中断构建）。
2. **把整套 ComfyHub 放到工作区外**（终解）：`pwsh -File scripts\pack-release.ps1 -OutDir "$env:USERPROFILE\ComfyHub"`（目录随便挑，只要在工作区外），然后运行那里的 `viewer.exe`。那边的文件没有 Low 标签可继承 —— 既不受限，内容也仍全在它自己的文件夹里。
3. **运行时就地修**：App 起后端前 best-effort 修一次（`backend_launcher.dart` 的 `_normalizeIntegrity`），`scripts\backend.ps1 start` 也会先修（`Repair-LaunchImages`）。这条**只能救"App 是 Medium、只有后端 exe 还带 Low 标签"那一半**。

⚠ 两条边界：**"换个终端重起"没用**（映像标签没变）；**别建议往 `%LOCALAPPDATA%` / `%TEMP%` 拷后端**。

## 四条纪律

1. **闸门留着**（`comfy::comfy_launch::ensure_writable`）：白起一个 100% CPU 卡死的实例，比如实拒绝糟得多。但**文案必须说对** —— 先讲真因，再给上面三条处方；**不许出现"权限 / ACL / 换个终端 / 双击 ComfyUI Desktop"**（`infra::confinement::remedy_text()`、`comfy_launch`、`image_edit` 三处同一纪律，用例 `修法文案指向发布包而不是让用户改_acl` 盯着）。
2. **别把探针的 `Err` 吞成 `bool`**（`infra::confinement::probe_dir` 返回 `io::Result` 就是为此）：只报"哪个目录"不报"报什么错"，就会重演"现象对、处方错"。用例夹具按 `AGENTS.md` 第 3 节造确定性的 `os error 5`，不拿"这台机器哪个目录不可写"当判据。
3. **看得见**（`server-rs\src\infra\confinement.rs`）：后端启动自查 → 受限才写 `.run\confinement.json`、**正常就删**（免得过期标记一直亮着），标记带 `pid`。`GET /api/system/confinement` 把结论**连判据一起**交给界面（`serverImage` / `imageInsideProjectRoot` / `blockedPaths` / `remedy`），受限时 App 顶部常驻横幅（`lib\widgets\confinement_banner.dart`）。⚠ 字段名只能说"**落在运行根目录里**"，别说"在工作区内"（发布包里根目录就是包目录，那里 `restricted=false`）。**界面上看不见的现象，用户和模型就只能靠猜。**
4. **写 input 的地方必须共用同一个入口**（`comfy::image_edit::write_or_defer`）：投放原图 / 遮罩 / 摆好的人物图三处曾经只有两处有 HTTP 退路，于是"摆好的那份没落地"。直写被拒一律改让 **ComfyUI 自己写**（`/upload/image`），并按它回的名字回填结果字段。逐条清点见 [`permission-audit.md`](permission-audit.md)。**"写哪个 input"也只有一个真源**：起实例的 argv 由 `comfy::comfy_launch::start` 落档（`.run\comfyui-<port>.launch.json`），投放走 `comfy::image_edit::target_input_dir` —— **先问实例**（`/object_info/LoadImage` 的名单 × 候选目录），问不到才按"留档 → 配置 output 兄弟 → Desktop `settings.json` 的 `inputDir` → 盘探"回落。⚠ 别拿**全名**判"调用方写过哪些参数"：`--input` 是 `--input-directory` 的 argparse 缩写、同一个 `dest`、**后写的赢**（用 `tools::mcp::specified_flag`，认 `--flag=value`）。两个环境变量名也别漂：`COMFYHUB_COMFY_OUTPUT` 与 `COMFYHUB_COMFY_OUTPUT_DIR` 都要认（`comfy::capture::comfy_output_env`）。

**连带影响**：受限会话里 `cargo test` 会红一片（测试二进制就在工作区里）；落盘与夹具要求见 [`backend-runtime.md`](backend-runtime.md) §9。

# 权限面彻查（2026-09-29）

这份表回答一个问题：**受限会话（"写不进工作区外"）下，这个项目还有哪些地方会写工作区外？**
每一处要么有退路、要么有闸门、要么本来就只读 —— 不许有"第四种"。

用户报的现场（会话 `36c790df`，17:24）：`comfy_make_mask(mode="subject", …)` 报
`写停放好的人物图失败：拒绝访问。 (os error 5)`，阶段二卡住。顺着它查出来的清单就是本表。

---

## 0. 先把机制钉死（两条都真，别再互相否定）

2026-09-29 在本会话重测（`server-rs\tests\confinement_probe.rs`，诊断用例，手动跑）：

```
测试二进制映像：D:\myProject\FlutterProject\viewer\server-rs\target\debug\deps\confinement_probe-….exe   ← 工作区内
① 本进程直接写 %TEMP%      → 写不进：拒绝访问。 (os error 5)
② 它拉起的 cmd.exe（工作区外的映像）写 %TEMP% → 没写成：Access is denied.
③ 工作区内写               → 写得进
```

同一时刻，**同一个会话**里的 pwsh（映像在 `C:\Program Files\PowerShell\7`）写 `%TEMP%`、
写 `D:\Comfy-Desktop\ComfyUI-Shared\input` **都正常**。于是两条机制同时成立：

| 机制 | 判据 | 后果 |
| --- | --- | --- |
| **按映像位置** | 可执行文件**落在工作区内** → 受限，与谁启动它无关 | 源码树里的 `server-rs\target\release\comfyhub-server-rs.exe`、`build\windows\…\viewer.exe` **永远**受限 |
| **按父子继承** | 受限进程拉起的子进程，**即使映像在工作区外**，照样受限 | 受限后端拉起来的 ComfyUI python / comfy-cli 全都写不了项目外 |

**由此得出两条处方上的硬结论**（此前文档里有写错的地方，已改）：

1. ⛔ **"换个终端重起后端就行"是错的** —— 源码树布局下后端映像就在工作区内，换终端一样受限
   （`confinement.rs` 那行 eprintln 原话就是这么写的，2026-09-29 已改）。
2. ✅ **正解仍然是"把整套东西挪到工作区外"**：发布包里 `viewer.exe` 在包根、后端在 `<包根>\server\`，
   两处都在工作区外，映像规则不成立；而启动它的父进程是用户自己的终端，继承规则也不成立。

> 这也解释了为什么 `comfy_start` 那道闸门是对的：**子进程会继承**，所以受限后端拉起的实例
> 是残废的（写不了自己的 `temp/`），白起一个只会得到 100% CPU 卡死的进程。

---

## 1. Rust 后端：写工作区外的每一处

真源：`server-rs\src\**`。下表按"写哪儿 / 干什么用 / 受限时怎么办"列。

| # | 位置 | 写到哪 | 受限时的行为 | 判定 |
| --- | --- | --- | --- | --- |
| 1 | `image_edit::use_media` / `use_attachment` | ComfyUI `input` | 直写被拒 → 攒给 `/upload/image` | ✅ 有退路 |
| 2 | `image_edit::write_mask_png` | ComfyUI `input` | 同上 | ✅ 有退路 |
| 3 | `image_edit::make_mask_with` **摆好的人物图** | ComfyUI `input` | **原来当场报错**（本次修的就是它） | ✅ 已补退路 |
| 4 | `comfy_launch::ensure_writable` | ComfyUI 工作区 / 产物目录 | **起手闸门**：`COMFY_WRITE_BLOCKED` + 如实文案 | ✅ 有闸门 |
| 5 | `comfy_launch::start`（日志 / pid） | `<根>\.run\comfyui-<port>.log` | 项目内 | ✅ |
| 6 | `mcp::prepare_launch`（comfy-cli 凭据 / 缓存 / TEMP） | `<根>\storage\ai\mcp\comfy-cli-home` | `USERPROFILE` / `LOCALAPPDATA` / `TEMP` **指进项目内** | ✅ 已重定向 |
| 7 | `capture_ingest` / `capture_poller` | **只读** ComfyUI `output`（受限进程**读**项目外是允许的） | — | ✅ 只读 |
| 8 | `capture_ingest::download_output` / `media_upload::upload` | `<storage>\tmp` → `<storage>\media` | 项目内 | ✅ |
| 9 | `capability_probe` / `capability_refresh` | `<storage>\ai\capabilities\custom.json` | 项目内 | ✅ |
| 10 | `tools.rs` 的写入工具（`ToolPolicy::resolveWrite`） | `<根>\comfyui` | 项目内（发布包同样在包内） | ✅ |
| 11 | `db` / `api_port_file` / `attachments` / `thumbs` / `poster` / `memory` / `credentials` / `skills` / `media_write` | 全部 `<根>\…` | 项目内 | ✅ |
| 12 | `comfy_dirs::find_home` / `comfy_models` | **只读**探测 ComfyUI / models | — | ✅ 只读 |

**结论**：后端侧现在只有 #1~#3 会碰工作区外的写，三处都有 HTTP 退路；#4 有闸门；其余全在项目内。
"input 目录写权限"这件事**从来不是** ACL 问题（见 §0）。

### 1.1 配套纪律

* 三处写入**必须走同一个入口** `image_edit::write_or_defer` —— 只补一处、别处照旧，就是这次
  "投放能过、摆放过不去"的成因。新增写入点时别再自己 `fs::write` + `map_err`。
* **回填文件名**：`LoadImage.image` 填的必须是 ComfyUI **回给你的那个名字**
  （`apply_uploaded_names`）。填错的表现是 `Invalid image file`，很像"文件不存在"。
* 错误文案**不许甩锅给 ACL**（`把它的 input 目录写权限给当前用户` 这类说法已被实测否掉），
  也不许说"换个终端重起"。

## 2. Dart / Flutter 侧

| 位置 | 写到哪 | 受限时 | 判定 |
| --- | --- | --- | --- |
| `lib/core/portable_paths.dart` | 偏好 → `<根>\storage\app-support` | `%APPDATA%` 读得到（读允许），写不进 → **一次性导入**旧值后改落项目内 | ✅ |
| `windows/runner/main.cpp` | DevFS `%TEMP%` | 引擎起来之前探一次，写不进就退 `.run\tmp` | ✅ |
| 其余（缩略图 / 附件 / 产物） | 都经后端，落 `<根>\storage` | 项目内 | ✅ |

## 3. 脚本（`scripts\*.ps1`）

`pwsh` 的映像在工作区外，脚本本身不受限；它们做的是**替用户把关**：

* `comfyhub.ps1 doctor` 读 `.run\confinement.json`（带 pid，换后端就重新判）并打印修法；
* `pack-release.ps1 -OutDir` 默认写到工作区外 —— **别把默认改回 `build\`**（那等于把发布包
  放回工作区内，装了也照样受限）；
* `backend.ps1` 的 `Resolve-RustExe` 优先认 `COMFYHUB_SERVER_EXE`：源码树布局下三个默认候选
  全在工作区内，所以**源码树必然受限**，这不是配置错了。

## 4. 测试（这一条是 2026-09-29 才补上的）

`cargo test` 的二进制 `server-rs\target\debug\deps\*.exe` **在工作区内** ⇒ 受限 ⇒
**任何写 `%TEMP%` 的用例都红**。本机实测：`cargo test --lib` 539 过 / **69 红**，
红的全是 `Os { code: 5, kind: PermissionDenied }`，与被测代码无关。

现规矩：

* 要落盘的用例一律用 `crate::testpaths::scratch()`（`tests/` 用 `comfyhub_server_rs::testpaths::scratch()`），
  落在 `<crate>\target\.scratch\`；**不许再写 `std::env::temp_dir()`**。
* 要造"写不进去"的场景，**用"在目标文件名上放一个同名目录"**，不要去改 ACL
  （Windows 上必然回 `ERROR_ACCESS_DENIED`，与受限现场同一个错码，且不依赖本机 ACL）。
* 迁移后 `cargo test` 全绿：lib 611 + 各集成用例（本机 2026-09-29 实测）。

## 5. 复现与自检

```powershell
# 现在这个后端受不受限？（doctor 会读后端自己留的证，带 pid 比对）
pwsh -File scripts\comfyhub.ps1 doctor

# 想自己量一遍机制（诊断用例，默认 --ignore）
cd server-rs; cargo test --test confinement_probe -- --ignored --nocapture
```

---

## 6. 本轮做完的 / 还剩的

**已做（2026-09-29）**

* ✅ 写 input 的三处收敛到 `image_edit::write_or_defer`，直写被拒一律改走 `/upload/image`，
  并按 ComfyUI 回的名字回填结果字段；
* ✅ `Delivery` 支持多份投递（只留一个槽位会静默丢后一份）；
* ✅ 错误文案不再甩锅 ACL、不再建议"换个终端重起"；
* ✅ `GET /api/system/confinement` + App 顶部常驻横幅（受限时才出现，判据与修法一起显示）；
* ✅ 测试暂存目录统一进项目内（`testpaths::scratch`），`cargo test` 在受限会话里全绿；
* ✅ 发布包 A/B 实测（同一个二进制：源码树 `restricted=true`、发布包 `false`）。

**还剩（留给下一次）**

* `comfyhub.ps1 up` 目前是**事后**由 `doctor` 报出受限；可以在起后端之前就判一次映像位置并提示，
  省得用户等界面横幅。
* 横幅只在 `HomeShell` 挂载时问一次；重起后端解除了，要回到设置页点一次「保存并测试连接」
  才会刷新（可考虑挂到 `BackendLauncher` 的重启回调上）。
* 移动端 harness（Android）还没接这个端点；真机提交走 `comfy_queue_submit`，暂不受影响。


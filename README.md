# ComfyHub

**本机的 AI 绘图工作台**：把 ComfyUI 的"能跑"变成"好跑" —— AI 帮你写提示词、选工作流、投放参考图、
提交生成，产物自动进画廊；提示词 / 标签 / 角色卡这些"库"是本地 SQLite 单文件，跨端只走局域网配对。

三句话：

- **一个 App 两种角色**：桌面（Windows）既是 UI 也是后端；手机（Flutter，可选）连桌面的后端干活。
- **数据全在自己文件夹里**：`<根>\.db`（库）、`<根>\storage`（产物 / 附件 / 缩略图）、`<根>\.run`（端口 / 日志 / 临时）。
  不许往 `%APPDATA%` / `%LOCALAPPDATA%` / `%TEMP%` 散（AGENTS §13）。
- **不依赖 JVM / MySQL / Node**：后端是一个静态链接的 Rust 单文件 exe。

---

## 1. 快速开始

```powershell
flutter build windows --release          # 构建（末段会自动 cargo build --release 并把后端装进产物 backend\）
pwsh -File scripts\pack-release.ps1 -OutDir D:\dist\ComfyHub   # 想放到工作区外长期用（推荐，见 §6）
pwsh -File scripts\dev-app.ps1           # 日常开发：debug 版起起来，按 r 热重载
```

- **数据库不用先建**：后端首次启动自己建 `<根>\.db\comfyhub.db`（SQLite，DDL 编译期嵌在 exe 里）。
- **后端不用手动起**：App 启动时先探一次健康检查，连不上就自己执行 `backend\comfyhub-server-rs.exe`
  （不经过任何脚本、无控制台窗口）；已经健康就什么都不做。
- **端口不用配**：后端要一个空闲端口（`--port 0`），把真实端口写进 `<根>\.run\api.json`，
  App / 脚本 / 捕获扩展都只读它（§5）。

## 2. 功能一览

| 页面 | 做什么 |
| --- | --- |
| **AI 工作台**（默认落地页） | 和模型对话干活：写提示词、查 danbooru、投放参考图、提交 ComfyUI、看实时进度与产物入口卡。审批闸门 / 提问闸门 / 轮次与预算上限都在这一页看得见 |
| **画廊** | 生成产物：列表 / 详情 / 大图（滚轮缩放）/ 视频播放；NSFW 标记与模糊；右键菜单 |
| **提示词** | 本机提示词库：搜索、收藏、标签、工作流原文（界面格式 ↔ API 图的转换在服务端） |
| **标签** | 标签库与分类、使用次数 |
| **设置** | Provider / 模型目录 / API Key、「远程访问与配对」、NSFW 开关、ComfyUI 自动捕获、桌面端还有本地服务与后端地址 |

后端能力（都是 AI 的工具，不是界面按钮）：

- **ComfyUI 自动捕获**：轮询 `/history` → 解析节点图 → 产物入库成"提示词 + 媒体"（幂等靠 `prompt_id` + 文件 SHA-256）；
  另外两条入口是**目录导入**（读 PNG 内嵌工作流）与**推送**（自定义节点）。见 §7。
- **工作流**：找能跑的 / 体检（模型与节点齐不齐）/ 摘拼节点 / **配方**（把一份具体工作流归纳成"骨架 + 模型槽位 + 旋钮"，
  换新模型不用重搭）/ 提交（`comfy_submit`，只能提交 API 格式节点图）。
- **图像编辑**：把画廊产物 / 用户附件**投放**进 ComfyUI 的 `input`，生成遮罩，贴人物（抠图交给工作流里的 rembg 节点）。
- **工具与 Skills**：桌面全量工具清单（`server-rs/src/tools/tools.rs` 是唯一真源）+ 三层准入（选完预检 / 发送前禁用 /
  建 Run 前后端再验）；Skills 正文真源是磁盘（`<storage>\ai\skills` 与 `<项目根>\skills\builtin`），**不做缓存**。
- **长期记忆**：`<storage>\ai\memory.md`，一行一条，用户可手改；写超限**报错不截断**。
- **角色卡**：人物的标准容器（名称 / 提示词 / 排除词 / **位置**），**同时最多启用 6 个**、未启用不占 token。见 §4。

## 3. 架构

```
Flutter（Windows / Android）
   │  HTTP + SSE（默认 http://127.0.0.1:<自动端口>）
   ▼
Rust 后端（单文件 exe，SQLite 单文件）──► ComfyUI（127.0.0.1:8188）
   │                                        ▲
   └── 捕获扩展（custom_nodes\comfyhub_capture，可选）──┘
```

- 桌面后端只改 `server-rs/`（Rust）；`legacy/`（退役的 Kotlin 后端）**2026-09-30 已删**，要对照从 git 历史取。
- 数据库是 SQLite 单文件，**没有数据库服务**；端口只有一个真源 `.run\api.json`。

### 目录

| 路径 | 装什么 |
| --- | --- |
| `lib/` | Flutter 前端（`pages/` 页面、`widgets/` 组件、`state/` 状态、`core/` 客户端与启动器、`models/` 形状） |
| `server-rs/src/` | Rust 后端，**按大功能分 10 个域**（模块地图与纪律在 `server-rs/src/lib.rs` 头注释）：`ai` `provider` `tools` `comfy` `workflow` `media` `library` `infra` `mobile` `web` |
| `server-rs/tests/` | 集成测试（`api_test/` 按域分文件 + `common.rs` 共享夹具） |
| `db/schema.sqlite.sql` | **唯一的建表真源**（收敛式 DDL，编译期嵌进后端） |
| `comfyui/` | 自带工作流（`workflows/`）、捕获扩展（`comfyhub_capture/`） |
| `skills/` | 内置 Skills、工作流配方模板 |
| `scripts/` | 构建 / 测试 / 打包（§7），**App 起停后端不经过它们** |
| `docs/` | 设计、契约、排错（§9） |

## 4. 角色卡（生图角色卡框架）

框架来自 [docs.aisese.ai/zh-CN/imagegen/character](https://docs.aisese.ai/zh-CN/imagegen/character)：

- **四字段**：名称（只做标识、不参与生成）/ 提示词 / 排除词 / **位置**；
- **位置 = 5×5 网格**（行 `A`~`E`、列 `1`~`5`），默认 **`C3` 居中**；留空 = 自动（让模型自己摆）；
- **单次生成最多同时启用 6 个**（每张卡一个 ✓ 开关）；未启用留在库里待命、**不占 token**；
- **token 口径**：界面顶部显示 `已启用 N/6 · 提示词 X/512 · 排除词 Y/512`，
  数字由后端 `GET /api/ai/character-cards/summary` 算（**前端不重算**）。

我们在这四件之上还保留了资料库那套（danbooru tag 栈 / LoRA 与权重 / 参考图 / 别名 / 备注 / 核对时间）。

## 5. 端口、环境变量、运行布局

```powershell
Get-Content .run\api.json          # 后端实际监听的端口（唯一真源）
```

| 环境变量 | 作用 |
| --- | --- |
| `COMFYHUB_PORT` | 想要固定端口时设（默认 `0` = 让系统挑） |
| `COMFYHUB_SERVER_EXE` | 指定后端 exe（**把映像挪出受限区**的唯一开关，AGENTS §12） |
| `COMFYHUB_PROJECT_ROOT` | 覆盖运行根目录（工具白名单 / 库 / 产物都按它算） |
| `COMFYHUB_COMFY_URL` / `COMFYHUB_COMFY_OUTPUT` | ComfyUI 地址 / 产物目录的环境默认值 |
| `COMFYHUB_HTTPS_PROXY` | 出网代理（本机与内网强制直连） |

**两种运行布局**（改路径解析前必读 AGENTS §7）：源码树（根 = 有 `server-rs/Cargo.toml` 的那层，
后端用 `server-rs\target\{release,debug}\`）与发布包（根 = `viewer.exe` 所在目录，后端在同级 `backend\`）。

## 6. 为什么建议放到工作区外

工作区里构建出来的 exe 带着 **Low 完整性标签**，Windows 取 `min(父进程, 映像)` ⇒ 进程也是 Low ⇒
**写不了工作区外**（ComfyUI 的 `input` / `output` 都在工作区外），表现是 `拒绝访问 (os error 5)`。
把整套 ComfyHub 打包到工作区外再运行就没有这个问题：

```powershell
pwsh -File scripts\pack-release.ps1 -OutDir D:\dist\ComfyHub
D:\dist\ComfyHub\viewer.exe
```

真因、判据与另两条处方见 AGENTS §12；界面右上角受限时会有一条常驻横幅（`GET /api/system/confinement`）。

## 7. 脚本速查

| 脚本 | 干什么 |
| --- | --- |
| `dev-app.ps1` | 保证后端二进制是最新的 + `flutter run -d windows --debug`（改完按 `r` 热重载） |
| `backend.ps1` | e2e / dev 用的后端旁路：`start` / `stop` / `status` / `build`（先修映像标签） |
| `api-port.ps1` | 读 `.run\api.json` 拿端口（脚本侧唯一的读法） |
| `pack-release.ps1` | 打包 / 校验发布包（`-OutDir` 建议指到工作区外；`-Clean` 不动运行期数据） |
| `comfy-path.ps1` | 找 ComfyUI（判据与后端 `comfy_dirs` 必须一致） |
| `mcp-runtime.ps1` | 可选能力：MCP 运行时（检测优先，装不上下载便携版兜底） |
| `video-poster.ps1` | 给视频产物取封面 / 预览帧（可选能力） |
| `gen-builtin-catalog.ps1` | 重新生成内置模型目录的冻结副本 |
| `build-apk.ps1` | 打 Android 包 |
| `e2e-*.ps1` | 端到端自测（都不出网）：`ai-tools` 111 项、`capture`、`submit`、`image-edit`、`inpaint`、`pair`、`recipe`、`credential` |

## 8. 捕获与接口

**自动捕获三条入口**：轮询 `/history`（默认 4s）、目录导入（读 PNG 内嵌元数据）、推送（自定义节点
`comfyui\comfyhub_capture\`，拷进 `<ComfyUI>\custom_nodes\` 后重启 ComfyUI）。配置在设置页
「ComfyUI 自动捕获」；细节见 [docs/comfyui-capture.md](docs/comfyui-capture.md)。

**REST API** 的机器可读真源是路由表本身：`server-rs/src/web/routes.rs`、`web/write_routes.rs`
与各模块自己的 `router()`（例如 `library/character_cards.rs`）。人读的契约文档：

| 文档 | 管什么 |
| --- | --- |
| [docs/run-event-contract.md](docs/run-event-contract.md) | 一轮 Run 的有序块类型 + 实时事件（SSE） |
| [docs/run-kit-contract.md](docs/run-kit-contract.md) | `GET /api/ai/run-kit` 下发给手机端的规则工件 |
| [docs/sync-api-contract.md](docs/sync-api-contract.md) | 同步 API 与手机排队提交 |
| [docs/tool-scope-contract.md](docs/tool-scope-contract.md) | `DESKTOP` / `MOBILE` 工具作用域 |
| [docs/sqlite-schema-contract.md](docs/sqlite-schema-contract.md) | 库结构、**加列怎么迁移**、红线 R1~R4 |

常用只读探针：`GET /api/health`（引擎与 schema 版本）、`GET /api/ai/tools`（当前能跑的工具）、
`GET /api/system/confinement`（本进程受不受限）、`GET /api/stats`。

## 9. 数据库

- 位置：`<根>\.db\comfyhub.db`（`COMFYHUB_JDBC_URL` 可换）；**SQLite 单文件，没有服务**。
- 建表真源：`db/schema.sqlite.sql`（收敛式 DDL，重跑安全）；版本号是 `PRAGMA user_version`
  （`server-rs/src/infra/db.rs` 的 `SCHEMA_VERSION` 与 DDL 末尾那行**必须相等**，有用例钉）。
- **加一列要三处一起改**：DDL + `infra/db.rs` 的 `NEW_COLUMNS`（`ALTER`，每次开库按
  `PRAGMA table_info` 探测）+ `SCHEMA_VERSION` +1。理由与防线见
  [docs/sqlite-schema-contract.md](docs/sqlite-schema-contract.md) §2。

## 10. 测试

```powershell
$env:PUB_HOSTED_URL='https://pub.dev'; flutter analyze; flutter test   # 前端：408 例 ≈ 25s
cargo test --manifest-path server-rs\Cargo.toml                       # 后端：lib 630 + 集成 150 ≈ 30s
pwsh -File scripts\e2e-submit-test.ps1                                # 提交链路（6 幕，不出网、不花钱）
```

- 纯逻辑改动跑单测就够；**改了 `HarnessRunner` 的上下文拼装要跑三条 e2e**（AGENTS §10.1）。
- 要**活体**验收（真 ComfyUI）用 `spot_check`：`cargo test --test spot_check -- --ignored`。
- 测试落盘一律 `testpaths::scratch()`（`server-rs\target\.scratch\`）—— 受限会话里写 `%TEMP%` 必红。

## 11. 排错

**先在 [docs/pitfalls.md](docs/pitfalls.md) 里搜现象**（60+ 条：现象 → 原因 → 处理）。最常撞的三条：

| 现象 | 先怀疑 |
| --- | --- |
| `拒绝访问 (os error 5)` / `COMFY_WRITE_BLOCKED` | 进程受不受限（§6）；别去改 ACL、也别"换个终端重起" |
| "我改了但界面/服务没变" | **构建新鲜度**：Release 看 `data\app.so`、后端看 `server-rs\target\release\` 的时间 |
| `Invalid image file` | 投放的目录 ≠ 实例在读的 `input`（去向实例本身确认，别猜） |

## 12. 文档地图

| 文档 | 给谁看 |
| --- | --- |
| [AGENTS.md](AGENTS.md) | 在这个仓库里干活的人 / AI：纪律、本机硬约束、踩过的坑怎么做才对 |
| [docs/pitfalls.md](docs/pitfalls.md) | 排错表（现象 → 原因 → 处理） |
| [docs/implementation-master-plan.md](docs/implementation-master-plan.md) | 目标架构现状 + 14 条红线 + 分阶段验收判据 |
| [docs/ai-tools-and-skills.md](docs/ai-tools-and-skills.md) | AI 工具与 Skills 的接线（含与 DSH 的对照） |
| [docs/permission-audit.md](docs/permission-audit.md) | 写工作区外的 12 处，逐条标注退路 / 闸门 |
| [docs/pairing-design.md](docs/pairing-design.md) | 配对闸门与密钥下行 |
| [docs/workflow-recipe-design.md](docs/workflow-recipe-design.md) | 工作流配方 |
| [docs/mobile-harness-design.md](docs/mobile-harness-design.md) | 移动端 harness（Android Kotlin） |

## 13. 已知限制

- **手机端是可选能力**：配对 / 同步的路是通的，但移动端 harness（Android Kotlin）仍在收尾。
- **MCP 运行时是可选下载**（几百 MB，装不上只影响一批 ComfyUI 工具，不拦启动）。
- **ComfyUI 必须在跑**：AI 的 ComfyUI 工具都直连 `comfyUrl`（默认 `127.0.0.1:8188`），连不上会如实报。
- **NSFW 开关只影响两件事**：注入那段越狱/人格系统提示、以及把标记过的产物在列表里打码。

## 14. 开源与致谢

Flutter / Dart、Rust（axum / rusqlite / image / tokio）、ComfyUI（含 `comfy-mcp`、`comfy-cli`）、
以及 `third_party\video_player_win`（上游 3.2.2 的本地补丁副本，原因见 AGENTS §9）。

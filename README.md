# ComfyHub

**本机的 AI 绘图工作台**：把 ComfyUI 的"能跑"变成"好跑" —— AI 帮你写提示词、选工作流、投放参考图、
提交生成，产物自动进画廊；提示词 / 标签 / 角色卡 / 画风卡这些"库"是本地 SQLite 单文件，跨端只走局域网配对。

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
| **AI 工作台**（默认落地页） | 和模型对话干活：写提示词、查 danbooru、投放参考图、提交 ComfyUI、看实时进度与产物入口卡。审批闸门 / 提问闸门 / 轮次与预算上限都在这一页看得见。AppBar 的「LoRA 训练台」进次级页（见下） |
| **LoRA 训练台**（次级页） | 可选能力：本机 anima-lora-next 的**完整训练台**（与 Qt GUI 的页签对齐，MOSS 页除外）—— 取训练集 → **人工筛选** → 打标（可选）→ 预处理 → **参数（复现 Qt GUI 的 Config 表单）** → 正式训练；另有 **Krea-2**、**合并（烘焙 / 融合 / ΔW 提取）**、**EasyControl**（方法 / 描述符变体）、**TensorBoard**、**Anime Tools 策展面板**、**实验（Turbo / Soup）** 与**全局作业队列**。内嵌引擎自己的响应式 WebUI（手机 / 电脑同一份状态），AI 工作台调的是同一批 `anima_studio_*` 工具 |
| **画廊** | 生成产物：列表 / 详情 / 大图（滚轮缩放）/ 视频播放；NSFW 标记与模糊；右键菜单。**手机上默认缓存看过的缩略图 / 原件**，详情菜单可「保存到手机相册」 |
| **提示词** | 本机提示词库：搜索、收藏、标签、工作流原文（界面格式 ↔ API 图的转换在服务端）。AppBar 的「布局 → 提示词」进画布次级页（主入口在 AI 工作台的模板卡，见下） |
| **标签** | 标签库与分类、使用次数 |
| **设置** | Provider / 模型目录 / API Key、「远程访问与配对」、NSFW 开关、ComfyUI 自动捕获、桌面端还有本地服务与后端地址、**产物导出** |

后端能力（都是 AI 的工具，不是界面按钮）：

- **ComfyUI 自动捕获**：轮询 `/history` → 解析节点图 → 产物入库成"提示词 + 媒体"（幂等靠 `prompt_id` + 文件 SHA-256）；
  另外两条入口是**目录导入**（读 PNG 内嵌工作流）与**推送**（自定义节点）。见 §7。
- **工作流**：找能跑的 / 体检（模型与节点齐不齐）/ 摘拼节点 / **配方**（把一份具体工作流归纳成"骨架 + 模型槽位 + 旋钮"，
  换新模型不用重搭）/ 提交（`comfy_submit`，只能提交 API 格式节点图）。
- **蓝图与接线体检**：任意一份工作流都能摊成**节点-连线画布**（真实坐标 / 分组 / 旁路灰显 / 前端 only 节点一个不丢），
  并给出**哪里接错了**（类型不匹配 / 悬空引用 / 空必填 / 环 / 没输出节点 …）；在画布上还能**改参数、关掉某条分支**，
  改完就地复检、可复制或存成文件。入口：详情页「查看工作流 → 看蓝图」。见 [docs/workflow-blueprint-design.md](docs/workflow-blueprint-design.md)。
- **选择现有工作流**：AI 工作台顶栏「选择现有工作流」列出本机 ComfyUI 里已保存的工作流，每行可**看蓝图**；
  选中后把路径写进输入框，AI 会照这一份出图，**不再自己编节点图**。带 `TextGenerate` 的工作流由 AI 服从其
  `system_prompt` 自己写终稿，再用 `comfy_submit(positivePrompt=…)` 摘线注入正面条件编码。
- **图像编辑**：把画廊产物 / 用户附件**投放**进 ComfyUI 的 `input`，生成遮罩，贴人物（抠图交给工作流里的 rembg 节点）。
- **工具与 Skills**：桌面全量工具清单（`server-rs/src/tools/tools.rs` 是唯一真源）+ 三层准入（选完预检 / 发送前禁用 /
  建 Run 前后端再验）；Skills 正文真源是磁盘（`<storage>\ai\skills` 与 `<项目根>\skills\builtin`），**不做缓存**。
- **长期记忆**：`<storage>\ai\memory.md`，一行一条，用户可手改；写超限**报错不截断**。
- **卡片库**：角色卡（画的是谁）+ 画风卡（画成什么样），同等地位。每张卡只有**一版自然语言正面提示词**
  + 负面提示词 + 示例产物；工作台输入区点 `+`（或输入 `@`）就能把卡插进这一条消息。见 §4。
- **2D 布局（工作台内，2026-11-03 换思路）**：用户在 AI 工作台描述画面、要一版提示词时，AI 在写提示词的
  **同一轮**调 `layout_propose` 给出 1~3 个可编辑的构图草稿；回复末尾出现「2D 布局模板」卡，点开进画布
  （`lib/pages/layout_canvas_page.dart`，带 AI 的草图）改框 / 改关系，点「提交回工作台」就把这份规格作为
  一条用户消息发回对话，AI 按新框重写提示词。契约见
  [docs/layout-workspace-design.md](docs/layout-workspace-design.md)；**3D 路线已搁置**
  （[docs/layout-3d-design.md](docs/layout-3d-design.md) 保留为技术评估）。画布仍有两个入口（工作台卡片 /
  预设库图标），产出两条出口：**通用提示词**（散文，任何模型都能读）与 **MOSS 布局 JSON**
  （与 ComfyUI `MOSSImageLayoutPrompt` 节点输出逐字节同构，可直接当提示词喂给 MOSS-Image）。
  坐标内部只有一套单位（`0~1000`，与上游 `Create Bounding Boxes` 一致）；散文是**有损投影**，所以界面会列出
  **翻译损失**（超过 5 个框、框小于 1%、大面积重叠、留白进不了 JSON……）。框之间还能标**关系**（镜像 / 同一主体 /
  挡在前），它会同时进散文与 JSON —— 坐标说不出"镜里那个是不是同一个人"。布局可以**存进本地布局库**
  （`SharedPreferences`，键 `comfyhub.layoutPresets`）随时调回来，存的是规格不是提示词。
  **「让 AI 写」**把布局交给译者（`POST /api/layout/translate`，规则见 [docs/layout-translator.md](docs/layout-translator.md)），
  **输出英文、一个框一段**。翻译是纯函数（`LayoutSpec.translate`）—— 同一份布局翻两次逐字节相同。
  见 `lib/models/layout_spec.dart`、`lib/models/layout_embed.dart`、`lib/core/layout_preset_store.dart`。
- **外观**：默认跟随**系统强调色**（Windows 读 `HKCU` 的 `AccentColorMenu`，Android 12+ 读
  `system_accent1_500`；读不到就回默认紫）。页面与卡片的底色**始终保持白 / 灰**，
  强调色只用在按钮、开关、选中态这些组件上（设置页可关）。见 `lib/core/system_accent.dart`。
- **手机端产物缓存 / 保存**：画廊格子与详情页看过的缩略图、原件默认落到
  `<app-support>/media-cache/`（不是整库复制，避免几十 GB）。**有上限**：缩略图 150MB、
  原件默认 1.5GB（设置页「产物缓存」可调 512MB~4GB），超了按**最久未用**淘汰到 80%；
  超过 200MB 的视频 / 音频**不在打开详情时自动下**（点「保存到手机相册」才下）。
  设置页能看占用、也能一键清理。详情菜单「保存到手机相册」
  把**这一条**交给 Android MediaStore（图片 / 视频 / 音频分别进 Pictures / Movies / Music）。
  见 `lib/core/media_cache.dart`、`lib/widgets/media_cache_card.dart`、
  `android/app/src/main/kotlin/com/example/viewer/MediaSaver.kt`。
- **换网络自动找回桌面**：桌面在局域网里 UDP 4039 广播当前端口，已配对手机收到后
  用**原配对令牌**探一次；只有当前地址不通才切换，**不重扫二维码、不轮换令牌**。
  跨网段 / AP 隔离仍走 Tailscale 或手填地址。见 `server-rs/src/mobile/discovery.rs`、
  `lib/core/lan_discovery.dart`。
- **手机从桌面搬 Provider**：手机「AI 模型与凭据」里点「从电脑同步 Provider」，把桌面那份
  清单 + 模型目录（含能力声明）搬进本机库 —— 桌面有 / 手机没有的**新建**，同 id 的**用桌面覆盖**，
  **只在手机上的保留不删**；API Key 不在其中（要单独从「远程访问与配对」取回）。
  见 [docs/pairing-design.md](docs/pairing-design.md) §14.6。

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

## 4. 卡片库：角色卡 + 画风卡

**角色卡**管"画的是谁"，**画风卡**管"画成什么样"（画师 / 媒介 / 上色 / 光 / 质感）——
两者**同等地位**、字段同形、共用同一个页面与同一套 AI 工具。

每张卡有：

- **名称**：只做标识（同名 = 同一张卡：AI 重复保存是**合并**，不会攒出第二张）；
- **正面提示词**（`prompt`）：**只有一版，整段自然语言**。角色卡要求 **600+ token、从头到脚每一个细节**；
  画风卡要求 **至少 100~200 token**——这是**下限不是上限**：单一媒介的一段话就够，复杂画风
  （上色风格 / 打光习惯 / 面部·头发·衣物·其他物体的材质分开处理 / 画面维度与后期）**按需写长**，
  写到覆盖为止。要 Danbooru tag 版时让 AI **当场另翻一版**，不存进卡里
  （2026-10-24 用户口径：tag 栈不如一句话耐改，换底模 / 换 tag 体系都不用重写）；
- **负面提示词**；
- **示例产物**：可以挂最多 24 条画廊产物（`#mediaId`），界面画成缩略图，AI 用
  `comfy_use_media` 把示例投进 ComfyUI 当参考。

正文空着 = 卡没写完：工具会提醒，AI 会按角色 / 画风的长度要求补。写作规则与自检清单在
skill `card-writer`（角色卡 600+ token 全身上下；画风卡 100~200 token 起，复杂画风按覆盖清单写长）。

**工作台怎么用**（`r2i-prompt` skill）：输入区底行的 `+` 菜单里选「角色卡 / 画风卡…」
（或直接输入 `@`）→ 点卡选中，面板**不收起**，可以接着点第二、第三张角色卡（多角色同框）；
再点同一张 = 取消，画风卡仍然只有一张（选新的替换）。输入框上方每张卡一个 chip，发送时
每张角色卡各拼一段 `[CHARACTER CARD]`、画风卡一段 `[STYLE SPEC]`（**逐字正文**，按选中顺序）
放在用户要求前面，模型按 skill `r2i-prompt` 扩写成一整段提示词。AI 给的成品提示词一律放进
Markdown 代码块（系统提示第 24 条，围栏语言 `text`），聊天气泡里可以一键复制。

为什么是结构化的一张表而不是长期记忆：记忆是"一行一条文本"、有 100 条上限、超限**直接报错**，
而卡片要按名字幂等地反复覆盖。

- **入口**：「预设库」页**顶部导航**直接切到「角色卡」/「画风卡」（与「常规预设」同级，
  不再是右上角的「卡片库」子页）；点卡片本体 = 编辑，示例缩略图仍单独可点。
  刻意不做成第六个一级导航项 —— 窄屏上六个图标会挤成两行。
- **AI 四条工具**：`search_character_cards` / `save_character_card`、
  `search_style_cards` / `save_style_card`。两个正面字段都是**整段替换**：想在已有内容上补两句，
  先把卡读出来、再给完整的那一份（追加会把两段风格不同的提示词粘成一条）。
- **端点**：
  - `GET/POST /api/ai/{character,style}-cards`、`GET/PUT/DELETE /api/ai/{character,style}-cards/{id}`
    （`PUT` 是全量替换，界面弹窗用）；
  - `GET/POST/PUT /api/ai/{character,style}-cards/{id}/examples`、
    `DELETE /api/ai/{character,style}-cards/{id}/examples/{mediaId}`。
- **老库升级**：schemaVersion 11 把卡表里残留的 `tag_prompt` **转成自然语言**灌进 `prompt`
  （只填空卡，已有正文一个字不动；只在升到 11 的那一次跑）。新库不再建那一列；
  老库那一列留在原地、再没人读。老 `tags`（JSON 数组）的更早一次迁移照旧。
- **不进同步范围**：卡片是桌面侧知识，手机提交走 `comfy_queue_submit`；与 `workflow_recipes` 同一档。

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
| `train-engine.ps1` | 可选能力：本地 LoRA 训练引擎（探测器 / 起停守护进程与自有 WebUI / 打开界面）。见 [docs/training-engine.md](docs/training-engine.md) |
| `video-poster.ps1` | 给视频产物取封面 / 预览帧（可选能力） |
| `gen-builtin-catalog.ps1` | 重新生成内置模型目录的冻结副本 |
| `build-apk.ps1` | 打 Android 包（只出 arm64-v8a） |
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

### 8.1 产物导出（2026-10-27）

产物原件本来就在磁盘上（`<根>\storage\media\<stored_name>`），库里存的是**定位 + 元信息**；
此前缺的是"带出去"的入口。现在四个入口：画廊顶栏（全部 / 仅收藏）、画廊多选（选中项，
集合格子会展开成整组）、产物详情页「更多 → 导出这一条…」、设置页「导出产物」。

- 后端 `POST /api/media/export`（**只允许回环**，会往任意绝对路径写文件；手机端不显示入口）；
  实现只有一份：`server-rs/src/media/export.rs`。
- 落盘结构：`图片/` `视频/` `音频/` `其它/` 按类型分；同一集合单独一个文件夹（`collection.json`
  记成员顺序）；每份产物旁边一份 `<文件名>.json` 侧车（提示词 / 集合 / 工作流引用）。
- 工作流按内容 SHA-256 去重放 `工作流/`（界面格式 `*.ui.json`；有捕获记录时再带 `*.api.json`）。
- 根目录 `导出清单.json` 是总账：相对路径、数量、失败原因。文件缺失 / 复制失败**只报那一条**，
  其余照常导出；同名文件不覆盖，自动加序号。**只读库、不搬不移**。

## 9. 数据库

- 位置：`<根>\.db\comfyhub.db`（`COMFYHUB_JDBC_URL` 可换）；**SQLite 单文件，没有服务**。
- **产物原件不在库里**：落在 `<根>\storage\media\<stored_name>`，`media_assets` 只存
  `stored_name` / 大小 / 哈希 / 提示词与工作流等元信息。导出（§8.1）是**复制**，不搬不移。
- 建表真源：`db/schema.sqlite.sql`（收敛式 DDL，重跑安全）；版本号是 `PRAGMA user_version`
  （`server-rs/src/infra/db.rs` 的 `SCHEMA_VERSION` 与 DDL 末尾那行**必须相等**，有用例钉）。
- **加一列要三处一起改**：DDL + `infra/db.rs` 的 `NEW_COLUMNS`（`ALTER`，每次开库按
  `PRAGMA table_info` 探测）+ `SCHEMA_VERSION` +1。理由与防线见
  [docs/sqlite-schema-contract.md](docs/sqlite-schema-contract.md) §2。

## 10. 测试

```powershell
$env:PUB_HOSTED_URL='https://pub.dev'; flutter analyze; flutter test   # 前端：592 例 ≈ 65s
cargo test --manifest-path server-rs\Cargo.toml                       # 后端：lib 789 + 集成 158 ≈ 40s
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
| [docs/workflow-blueprint-design.md](docs/workflow-blueprint-design.md) | 工作流蓝图与接线体检（含两份 krea2 结构样例的实测结论；样例不是出图起点） |
| [docs/training-engine.md](docs/training-engine.md) | 可选能力「本地 LoRA 训练引擎」：全链路（项目 / 数据集 / 打标 / 预处理 / 训练）、Rust `train` 域、内嵌 WebUI |
| [docs/mobile-harness-design.md](docs/mobile-harness-design.md) | 移动端 harness（Android Kotlin） |
| [docs/archive/](docs/archive/README.md) | **已归档**：做完了的任务书、被取代的进度快照、当时的审计与调研 —— 不是当前依据 |

## 13. 已知限制

- **手机端是可选能力**：配对 / 同步的路是通的，但移动端 harness（Android Kotlin）仍在收尾。
- **MCP 运行时是可选下载**（几百 MB，装不上只影响一批 ComfyUI 工具，不拦启动）。
- **ComfyUI 必须在跑**：AI 的 ComfyUI 工具都直连 `comfyUrl`（默认 `127.0.0.1:8188`），连不上会如实报。
- **NSFW 开关只影响两件事**：注入那段越狱/人格系统提示、以及把标记过的产物在列表里打码。

## 14. 开源与致谢

Flutter / Dart、Rust（axum / rusqlite / image / tokio）、ComfyUI（含 `comfy-mcp`、`comfy-cli`）、
以及 `third_party\video_player_win`（上游 3.2.2 的本地补丁副本，原因见 AGENTS §9）。

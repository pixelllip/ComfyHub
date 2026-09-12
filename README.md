# ComfyHub

> ComfyUI 提示词 & 生成产物管理器 —— **Flutter 前端 + Kotlin(Ktor) 后端 + 本地 MySQL**

把 ComfyUI 里跑通的提示词存起来、打上标签；把生成出来的图片 / 视频 / 音频导入进来并**关联到对应的提示词**。
之后点开任何一张图，就能立刻看到当初用的完整提示词；想找"那种赛博朋克夜景风格是怎么调出来的"，点一下标签就全出来了。

---

## 1. 功能一览

| 模块 | 能力 |
| --- | --- |
| **自动启动** | App 一启动就自己把 **MySQL + 后端**拉起来（先探健康，不健康才启动；启动过程实时回显在启动页上），不用再手动开脚本；**全程不弹命令行窗口**（见 [12 节](#12-本机环境踩坑记录)最后几条） |
| **提示词库** | 新建 / 编辑 / 复制 / 删除；区分「生图 / 生视频 / 生音频 / 混合」；正向 + 负向提示词；模型、采样器、调度器、步数、CFG、Seed、宽高、批量、LoRA 列表、备注、收藏；**多选批量管理**（收藏 / 取消收藏 / 加标签 / 删除） |
| **ComfyUI 自动捕获** | ComfyUI 里跑完一次生成，**提示词 + 全部参数 + 完整工作流 + 生成的图片/视频/音频**自动进库并互相关联；不需要改动工作流，也不需要装任何东西（装一个可选的推送节点可以做到零延迟） |
| **历史产物导入** | 指向 ComfyUI 的 output 目录，把**以前生成好的**图连同图片里内嵌的 `prompt` / `workflow` 一起收进来，自动建提示词并关联 |
| **搜索** | 关键词匹配（标题 / 正向 / 负向 / 备注 / 模型名，兼容中文）；按标签筛选（任一 / 全部）；按类型、收藏过滤；多种排序；分页 |
| **产物画廊** | 批量上传（图片 / 视频 / 音频，自动识别类型）；缩略图网格；类型 / 标签 / 收藏 / 未关联过滤；多选批量「关联提示词 / 收藏 / 删除」；**右键单个产物**弹出「关联提示词 / 收藏 / 删除」；**App 默认落在这一页**（提示词库排第二位） |
| **详情闭环** | 打开任意产物 → 直接显示**关联的提示词全文**（提示词正文一张卡）、**生成参数**（模型 / 采样器 / 调度器 / 步数 / CFG / Seed / 尺寸 / 批量，以及**逐个列出的 LoRA**，点一下复制 `<lora:名字:权重>`）单独一张卡、标签，可一键跳转到提示词详情；**右键图片 / 视频 / 音频**即可复制文件地址、文件名、正向 / 负向提示词；支持更换 / 解除关联；自动捕获的还能**查看完整工作流 JSON**（界面格式可拖回 ComfyUI 复现；agent / 脚本提交的运行只有 API 格式节点图，存成 `.json` 拖进 ComfyUI 也能加载） |
| **标签体系** | 全局词表 + 分类 + 颜色 + 使用次数；标签详情页同时列出该标签下的提示词与产物 |
| **媒体播放** | 图片用**大图查看器**：滚轮以鼠标位置为中心缩放、按住拖动平移、右下角缩略图指示当前看到的位置（点缩略图可直接跳过去）、左下角显示倍数并可一键适应窗口；视频内嵌播放（Windows Media Foundation，支持拖动进度）；音频用**紧凑播放器**（标题 + 播放行两行，限宽居中，不再占掉大半屏）；均可用系统默认播放器打开 |
| **多列布局** | 提示词 / 标签 / 设置三页的列数跟着窗口走：**列数 = 可用宽度 / 550**（最多 4 列），宽窗口不再把单列卡片拉成一米长；窄窗口自动退回单列 |
| **界面语言** | 界面全中文，文本框选择菜单（复制 / 全选 / 剪切 / 粘贴）等系统文案也走 `zh_CN`（`flutter_localizations`），不会漏出英文 |
| **去重** | 上传 / 捕获 / 导入一律按 SHA-256 去重；同一次生成按 ComfyUI 的 `prompt_id` 去重，重复轮询不会产生重复数据 |

---

## 2. 架构

```
┌──────────────────────────┐        HTTP / JSON + multipart
│  Flutter (Windows 桌面)   │  ────────────────────────────────┐
│  provider 状态管理        │                                  │
│  Image.network / 播放器    │  ◄──── /api/media/{id}/file ─────┤
│  启动时自动拉起后端        │        （支持 Range，可拖进度条）   │
└──────────────────────────┘                                  │
                                                              ▼
                                    ┌──────────────────────────────────┐
                                    │  Kotlin + Ktor (Netty, :8080)     │
                                    │  ├─ 原生 JDBC + HikariCP          │
                                    │  ├─ 文件存储 storage/media         │
                                    │  ├─ 缩略图 storage/thumbs          │
                                    │  ├─ ImageIO 生成缩略图             │
                                    │  └─ 自动捕获：轮询 /history ★      │
                                    └───────┬──────────────────┬────────┘
                                            │ JDBC             │ HTTP
                                            ▼                  ▼
                          ┌──────────────────────────┐  ┌──────────────────────────┐
                          │  MySQL 8.4 (:3307)        │  │  ComfyUI (:8188)          │
                          │  comfy_hub 库             │  │  /history /view /queue    │
                          │  目录可自定义 ★            │  │  产物默认落本机 output 目录 │
                          └──────────────────────────┘  └──────────────────────────┘
```

**自动捕获是怎么接上的**（三种方式，默认只开第一种，都不需要改你的工作流）：

| 方式 | 触发时机 | 需要装东西吗 | 拿到什么 |
| --- | --- | --- | --- |
| **后端轮询 `/history`** | 默认每 4 秒一次 | 不需要 | 参数（API 节点图）+ 界面工作流（`extra_pnginfo`）+ 全部产物 |
| **自定义节点推送** | 跑完立刻 POST | 需要把 `comfyui\comfyhub_capture` 装进 ComfyUI | 同上，零延迟，ComfyUI 卡死也不丢 |
| **目录导入** | 手动点「导入已有产物」 | 不需要 | 从 PNG 内嵌的 `prompt` / `workflow` 还原参数与工作流 |

轮询那条路能同时看到「正在跑的队列」和「刚跑完的运行」，并且在 ComfyHub 启动时会把最近
`maxPerPoll` 条没捕获过的运行补上 —— 所以「先跑图、后开 App」也不会漏。

**为什么后端用原生 JDBC 而不是 ORM**：查询里有 `GROUP_CONCAT`、动态标签 `IN (...)`、`EXISTS` 子查询等 MySQL 方言，
直接写 SQL 更可控，也避开了 ORM 的版本兼容问题。整个后端只依赖 Ktor + HikariCP + MySQL 驱动，没有额外框架。

---

## 3. 目录结构

```
viewer/
├── lib/                          # Flutter 前端
│   ├── main.dart                 # 入口
│   ├── app.dart                  # ★ 启动闸门（先拉起本地服务）+ 主题 + 导航框架
│   ├── core/
│   │   ├── api_client.dart       # 后端 REST 客户端（含自动捕获接口）
│   │   ├── backend_launcher.dart # ★ 启动时自动拉起 MySQL + 后端，并回显脚本输出
│   │   ├── settings_store.dart   # 后端地址 / 项目目录 / MySQL 数据目录等本地设置
│   │   ├── theme.dart            # ★ 主题：中文字体族 / 字号 / 行高 / 字重
│   │   └── formatting.dart       # 时间 / 体积 / 颜色格式化
│   ├── models/models.dart        # 与后端 DTO 对应的数据模型（含捕获配置 / 状态）
│   ├── state/library_store.dart  # 全局状态（搜索条件 + 缓存）
│   ├── pages/
│   │   ├── prompts_page.dart         # 提示词库（搜索 / 标签筛选 / 列表）
│   │   ├── prompt_detail_page.dart   # 提示词详情 + 关联产物 + 查看工作流
│   │   ├── prompt_edit_page.dart     # 新建 / 编辑提示词
│   │   ├── gallery_page.dart         # 产物画廊（多选批量操作）
│   │   ├── media_detail_page.dart    # ★ 产物详情：直接看到关联提示词
│   │   ├── tag_results_page.dart     # 单个标签的提示词 + 产物
│   │   ├── tags_page.dart            # 标签管理
│   │   └── settings_page.dart        # ★ 设置：本地服务 / 自动捕获 / 统计 / 接口清单
│   └── widgets/                  # 通用组件（标签胶囊、缩略图、播放器、上传面板、工作流查看器、大图查看器…）
│
├── server/                       # Kotlin 后端（Gradle 项目）
│   ├── build.gradle.kts
│   └── src/main/kotlin/com/comfyhub/
│       ├── Application.kt        # main + Ktor 插件装配 + 路由挂载 + 启动自动捕获
│       ├── Config.kt             # 环境变量配置
│       ├── Db.kt                 # HikariCP + JDBC 小工具
│       ├── Migrate.kt            # ★ 启动时幂等补齐表结构（不依赖用户手动跑 SQL）
│       ├── Models.kt             # DTO（kotlinx.serialization）
│       ├── TagRepo.kt            # 标签仓储
│       ├── PromptRepo.kt         # 提示词仓储（搜索 / 标签过滤 / 工作流快照）
│       ├── MediaRepo.kt          # 产物仓储
│       ├── MediaFiles.kt         # 类型识别 / 尺寸探测 / SHA-256 / 缩略图
│       ├── Storage.kt            # 磁盘存储
│       ├── Http.kt               # 请求参数 / 响应头工具
│       ├── ComfyCapture.kt       # ★ 自动捕获：轮询 /history、解析、入库、目录导入
│       ├── GraphParse.kt         # ★ 从 API 节点图里提取提示词与参数
│       ├── PngMeta.kt            # ★ 读 PNG 内嵌的 prompt / workflow 文本块
│       ├── CaptureRepo.kt        # ★ 产物入库（SHA-256 去重）+ 运行记录
│       ├── SettingsRepo.kt       # ★ app_settings k/v（自动捕获配置存放处）
│       ├── CaptureRoutes.kt      # ★ 自动捕获 / 工作流查询接口
│       └── *Routes.kt            # 三组 REST 路由
│
├── db/
│   ├── schema.sql                # 建库建表 + 视图（全新安装）
│   ├── migrate.sql               # 老库升级到最新结构（幂等，后端启动时也会自动做）
│   └── seed.sql                  # 演示数据（4 条提示词 + 12 个标签）
│
├── scripts/
│   ├── comfyhub.ps1              # ★ 统一入口：把 MySQL + 后端当成一个整体来 up/down/status
│   ├── silent-process.ps1        # ★ 后台进程「无窗口 + 脱离进程树」启动（MySQL / 后端共用）
│   ├── mysql.ps1                 # 只操作 MySQL（init/start/stop/status/cli/seed/reset/move）
│   ├── server.ps1                # 只操作后端（自动挑选 JDK 21，自动确保数据库在跑）
│   ├── pack-release.ps1          # ★ 打包发布版：App + 后端 + 便携版 MySQL + JRE → dist\ComfyHub\
│   ├── autorun-app.ps1           # 一键：服务 → 分析 → 构建（Release）→ 启动 App
│   ├── dev-app.ps1               # ★ 前端开发用：服务 → flutter run --debug（热重载）
│   ├── e2e-capture-test.ps1      # ★ 自动捕获端到端自测（用假 ComfyUI，不跑真实生成）
│   ├── check-silent-start.ps1    # ★ 静默启动自测：启动服务时盯屏，确认没有弹窗
│   ├── install-comfy-node.ps1    # ★（可选）把捕获节点装进 ComfyUI
│   ├── e2e/                      # 自测用的假 ComfyUI 与测试 PNG 生成器
│   └── make-sample-media.ps1     # 生成演示用图片 / 音频
│
├── packaging/manifest.json       # ★ 软件打包清单：发布包里装哪些件、各放在哪（pack-release.ps1 读它）
├── dist/                         # 发布包装配输出（运行时生成，已 gitignore）
├── comfyui/comfyhub_capture/     # ★（可选）ComfyUI 自定义节点：跑完立刻推送捕获
├── docs/comfyui-capture.md       # ★ 三种捕获方式的详细说明与排错
├── docs/android-agp9-builtin-kotlin-migration.md  # ★ AGP 9.1 / 内建 Kotlin 迁移记录与踩坑
├── storage/                      # 产物文件（运行时生成，已 gitignore）
├── .mysql/                       # 默认的 MySQL 实例目录（可用 -DataDir 换位置）
└── .run/                         # 后端日志 / PID / e2e 临时文件（已 gitignore）
```

---

## 4. 环境要求

| 依赖 | 说明 |
| --- | --- |
| Flutter | 3.47+（本机 3.47.3） |
| JDK | **21 ~ 23**（后端用的 Gradle 8.12 不支持 JDK 24/25，脚本会自动挑） |
| Gradle（后端 `server/`） | 8.12 |
| Android 构建（`android/`） | AGP **9.4.0** + Gradle **9.6.0** + **AGP 内建 Kotlin**（`android.builtInKotlin=true`，KGP 只用来钉版本、不 apply）；迁移与踩坑见 [docs/android-agp9-builtin-kotlin-migration.md](docs/android-agp9-builtin-kotlin-migration.md) |
| MySQL | 8.0+（本仓库用免安装的 8.4.3 zip 版，放在 `D:\tools\mysql\mysql-8.4.3-winx64`） |
| Visual Studio | 2022+ 带「使用 C++ 的桌面开发」（编译 Flutter Windows 桌面端） |

> 环境变量 `COMFYHUB_MYSQL_HOME` 可覆盖 MySQL 解压目录；`COMFYHUB_JDK_HOME` 可指定 JDK。

---

## 5. 快速开始

```powershell
# 1) 一次性初始化本地 MySQL（建库 + 建表 + 演示数据 + 创建应用账号）
pwsh -File scripts\mysql.ps1 init

# 2) 生成演示媒体（可选）
pwsh -File scripts\make-sample-media.ps1

# 3) 构建并运行 Flutter 桌面端
pwsh -File scripts\autorun-app.ps1
```

**从第 3 步开始，MySQL 和后端就不需要你手动起了**：
App 启动时会先探一次 `http://127.0.0.1:8080/api/health`，
连不上或数据库不通就自己去调 `scripts\comfyhub.ps1 up`（按「MySQL 先就绪 → 后端再起」的顺序），
并把脚本输出实时显示在启动页上（首次如果后端没构建过，会顺便跑一次 gradle 构建，会慢一些）。
后端已经健康时它什么都不做，所以重复启动 App 不会有副作用。
**整个过程是静默的**：mysqld 和后端都在隐藏的（无窗口）控制台里跑，不会弹出 cmd 黑框。

想关掉这个行为：设置页 →「本地服务」→ 关掉「启动 App 时自动拉起服务」。
也可以在同一个卡片里手动「启动 / 修复」「重启」「停止」「查看日志」。

命令行仍然完全可用（脚本是同一个，两边行为一致）：

```powershell
pwsh -File scripts\comfyhub.ps1 up -WithApp   # 起 MySQL + 后端 + 桌面 App
pwsh -File scripts\comfyhub.ps1 status        # 三者状态一览
pwsh -File scripts\comfyhub.ps1 down          # 全停（App → 后端 → MySQL）
```

### 脚本速查

| 命令 | 作用 |
| --- | --- |
| **`scripts\comfyhub.ps1 up / down / status / restart / logs / doctor`** | **统一入口：把 MySQL + 后端当成一个整体来起停和体检** |
| `scripts\comfyhub.ps1 up -WithApp` | 顺带把桌面 App 也拉起来 |
| `scripts\comfyhub.ps1 up -SkipBuild` | 跳过 gradle 构建，直接用上次的产物启动（App 自动启动时走的就是这个） |
| `scripts\comfyhub.ps1 up -OwnerPid <pid>` | **App 自动启动时用的形式**：多挂一个「关 App 就停服务」的守护进程（见下面说明） |
| `scripts\watch-owner.ps1` | 那个守护进程本体：盯着 `-OwnerPid`，目标进程一退出就按 `-StopApi` / `-StopMysql` 停掉对应服务（一般由 `up -OwnerPid` 自动拉起，不用手敲；日志在 `.run\watch-owner.log`） |
| `$env:COMFYHUB_TRACE=1` + `scripts\comfyhub.ps1 up` | 把冷启动**每一段的耗时**打到 stderr（`[trace   1660ms] mysql: 就绪` 这种），用来定位"到底慢在哪" |
| `scripts\comfyhub.ps1 doctor` | 体检：路径 / 依赖 / 端口占用逐项检查 |
| `scripts\mysql.ps1 init` | 首次初始化数据目录 + 建库建表 + 演示数据 + 创建 `comfyhub` 账号 |
| `scripts\mysql.ps1 start / stop / status / restart` | 只操作数据库（stop 会提示后端还在跑） |
| `scripts\mysql.ps1 move -DataDir <新目录>` | **把 MySQL 实例目录整体搬到别的位置**（自动改配置 + 记住新位置 + 重启） |
| `scripts\mysql.ps1 cli` | 打开 mysql 命令行 |
| `scripts\mysql.ps1 schema / migrate / seed` | 只重建表 / 只做增量迁移 / 只重灌演示数据 |
| `scripts\mysql.ps1 reset` | 删库重建（危险） |
| `scripts\mysql.ps1 logs` | 看 MySQL 错误日志 |
| `scripts\server.ps1 start / stop / status / logs` | 只操作后端（start 会自动确保数据库在跑） |
| `scripts\server.ps1 run` | 后端前台运行（Ctrl+C 停） |
| `scripts\server.ps1 fatjar` | 打成一个独立 jar |
| `scripts\autorun-app.ps1` | 一键全流程：服务 → 分析 → 构建（Release）→ 启动 App |
| `scripts\pack-release.ps1` | **打包发布版**：按 `packaging\manifest.json` 把 App + Kotlin 后端 + 便携版 MySQL + JRE 装配成 `dist\ComfyHub\`（`-Zip` 顺带压包） |
| `scripts\dev-app.ps1` | **只改前端时用这个**：服务 → `flutter run --debug`，跑起来后按 `r` 热重载（见 [9.1](#91-只改前端时用-debug-版热重载省构建时间)） |
| `scripts\check-silent-start.ps1` | **静默启动自测**：启动服务的同时盯屏，报告有没有弹出 cmd / 控制台窗口（加 `-Restart` 从零走一遍） |
| `scripts\e2e-capture-test.ps1` | **自动捕获端到端自测**（假 ComfyUI，不需要真跑一次生成） |
| `scripts\install-comfy-node.ps1` | （可选）把捕获节点装进 ComfyUI，实现「跑完立刻捕获」 |
| `scripts\anima-gen.ps1` | **Anima 生图执行器**：向本机 ComfyUI 提交一次文生图并等落盘（`-PromptFile/-NegativeFile/-Width/-Height/-Seed/-Prefix`） |

> **关 App 的时候，后端 / MySQL 会不会跟着停？** 看服务是谁起的：
>
> * **App 自己拉起来的**（启动页那次 `up`）→ 会。App 启动脚本时会带上 `-OwnerPid <自己的 PID>`，
>   服务起来后 `up` 会挂一个脱离进程树的 `watch-owner.ps1` 守护进程。App 一退出
>   （正常关闭 / 崩溃 / 任务管理器强杀都算）它就把**这次真正启动过**的服务停掉，
>   只停自己起的 —— 库本来就是你在终端里开的，就不会被带走。
> * **你在终端里 `comfyhub.ps1 up` 起的** → 不会。服务常驻，App 开着关着都不影响；
>   之后手工 `up` / `down` 还会顺手把这个守护撤掉，免得它反过来把新起的服务停掉。
>
> 开关在「设置 → 本地服务 → 关闭 App 时一并停止本地服务」。**关掉的好处**是服务常驻，
> 下次开 App 直接热启动（不用等 7~8 秒冷启动）；**开着的好处**是不留后台进程、不占端口。

### 为什么需要一个统一的 `comfyhub.ps1`

MySQL 和后端是**硬依赖**关系：库一挂，后端所有接口都会 500。之前 `mysql.ps1` 和 `server.ps1`
各管一半，会出现三种坏状态：

| 坏状态 | 之前的表现 | 现在 |
| --- | --- | --- |
| 后端在跑、MySQL 挂了 | `server.ps1 start` 看到后端进程存在就直接返回，什么也不修 | `up` 检测到 `db != ok` 会**自动重启后端** |
| `mysql.ps1 stop` 之后 | 后端还活着，静静地对着一个死库 | `stop` 会**明确警告**并指向 `comfyhub.ps1 down` |
| 不知道三者分别是什么状态 | 没有任何一条命令能一次看清 | `status` 一行一个，后端那行报的是**端到端健康**（含数据库连通性） |

后端自身也做了容错：启动时不再 fail-fast，而是**带重试地等数据库**（20 次 × 1.5s），
所以「MySQL 比后端晚几秒就绪」不会让后端直接退出；MySQL 中途重启后 Hikari 连接池会**自动恢复**，
后端进程不需要重启（已实测：PID 不变，health 从 `db=无响应` 回到 `db=ok`）。

---

## 6. ComfyUI 自动捕获

**一句话**：你在 ComfyUI 里正常出图，ComfyHub 自动把这次生成的「提示词 + 全部参数 + 完整工作流 + 生成的图/视频/音频」
收进库里，并把它们互相关联 —— 不需要改工作流，也不需要手动导出。

### 6.1 捕获到的东西

| 内容 | 从哪来 | 存到哪 |
| --- | --- | --- |
| 正向 / 负向提示词 | 顺着采样器的 `positive` / `negative` 引用回溯到 `CLIPTextEncode` | `prompts.positive_prompt` / `negative_prompt` |
| checkpoint / 采样器 / 调度器 / steps / CFG / seed / 宽高 / batch | 采样器与 latent 节点 | `prompts` 对应列 |
| LoRA 链（名字 + 权重） | 从采样器的 `model` 输入一路回溯 `LoraLoader*` | `prompts.loras`（JSON） |
| 其余零散参数 | 所有节点里的标量输入，键名形如 `ControlNetApply.strength` | `prompts.extra_params`（JSON） |
| **完整工作流** | ComfyUI 的界面格式 workflow（`extra_data.extra_pnginfo.workflow`）；没有就退回存 **API 格式节点图**（agent / 脚本直接调 `/prompt` 的运行只有这一份，ComfyUI 同样能加载） | `prompts.workflow_json` / `media_assets.workflow_json` |
| 产物文件 | `/history` 的 `outputs`（`images` / `gifs` / `audio` / `video` …） | `storage/media/`，按 SHA-256 去重后入库并与提示词关联 |
| 运行记录 | 每次运行一条 | `capture_runs`（`run_key` = ComfyUI 的 `prompt_id`，唯一键，保证幂等） |

解析器不是为某一个工作流写死的：它按「**顺着引用把采样链走一遍**」的方式解析，实测覆盖两大类结构：

| 结构 | 参数在哪 | 例子 |
| --- | --- | --- |
| 经典 `KSampler` | 全在采样器自己身上（`steps` / `cfg` / `seed` / `sampler_name` / `scheduler`） | 绝大多数 SDXL / Flux 工作流 |
| 自定义采样链 | 分散在 `sigmas` → `BasicScheduler`（steps/scheduler）、`sampler` → `KSamplerSelect`（采样器名）、`noise` → `RandomNoise`（seed）、`guider` → `BasicGuider`（模型链 + 条件节点）；提示词在条件节点的 `prompt` 输入里 | MiniMax H3 参考图生视频 |

两条路都走不通时再退回「全图扫描」的启发式（最长文本 = 正向、键名带 `negative` 的 = 负向）。
识别不到的东西会以 `节点类.参数名` 的形式原样进 `extra_params`，不会丢；
工作流本身没定义的参数（例如某些自定义采样链里根本没有 CFG、分辨率由 `ResolutionSelector` 的
「比例 + 百万像素」算出）就如实留空，并把 `ResolutionSelector.aspect_ratio` 这类线索放进 `extra_params`。

### 6.2 三种方式，按需选

| 方式 | 怎么开 | 延迟 | 适合 |
| --- | --- | --- | --- |
| **后端轮询**（默认） | 什么都不用做，后端启动就在跑 | 默认 4 秒 | 绝大多数情况；ComfyUI 关掉再开也不影响 |
| **自定义节点推送** | `pwsh -File scripts\install-comfy-node.ps1` 后重启 ComfyUI | 立刻 | 想一点延迟都没有；或 ComfyUI 卡住时也要留下记录 |
| **目录导入** | 设置页 →「导入已有产物…」选 output 目录 | 手动 | 补录 ComfyHub 装好之前生成的老图 |

三种方式共用同一套幂等键（`prompt_id` + 文件 SHA-256），所以**可以同时开着**，不会重复入库。
细节与排错见 [`docs/comfyui-capture.md`](docs/comfyui-capture.md)。

### 6.3 配置（设置页 → ComfyUI 自动捕获）

| 项 | 说明 |
| --- | --- |
| 开启自动捕获 | 关掉后只保留手动「立即同步」和目录导入 |
| ComfyUI 地址 | 默认 `http://127.0.0.1:8188`；ComfyUI 装在别的机器/端口就改这里 |
| ComfyUI 输出目录 | **强烈建议填**：填了后端直接读本地文件（快，不复制大文件）；留空则通过 `GET /view` 下载 |
| 轮询间隔 | 1~30 秒，默认 4 秒 |
| 自动标签 | 捕获进来的提示词自动打上这个标签（默认 `ComfyUI`），方便在画廊里筛 |

配置存在数据库的 `app_settings` 表里，所以 App 和后端读的是同一份 —— 不会出现两边配置打架。
也可以用环境变量给默认值：`COMFYHUB_COMFY_URL`、`COMFYHUB_COMFY_OUTPUT`。

### 6.4 自测（不用真的跑一次生成）

```powershell
pwsh -File scripts\e2e-capture-test.ps1
```

它会起一个假的 ComfyUI（`scripts\e2e\fake_comfy.py`），造一张带 `prompt` / `workflow` 元数据的 PNG，
然后把四条路径全走一遍并逐项断言：**轮询捕获 → 幂等 → 目录导入 → 推送捕获**，
最后把测试产生的提示词 / 产物 / 运行记录清掉（加 `-KeepData` 可以保留来看效果）。

### 6.5 捕获相关接口

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/api/capture/config` | 读自动捕获配置 |
| `PUT` | `/api/capture/config` | 整体覆盖配置 |
| `GET` | `/api/capture/status` | 状态：ComfyUI 是否可达、队列长度、最近捕获的若干次运行 |
| `POST` | `/api/capture/poll` | 立刻轮询一次（App 的「立即同步」） |
| `POST` | `/api/capture/import` | 导入某个目录里已有的产物（读 PNG 内嵌元数据） |
| `POST` | `/api/ingest/comfyui` | 捕获入口，供自定义节点 / 外部脚本推送 |
| `GET` | `/api/prompts/{id}/workflow` | 该提示词的完整工作流 JSON（没有则 204） |
| `GET` | `/api/media/{id}/workflow` | 该产物的完整工作流 JSON（没有则 204） |

<details>
<summary>POST /api/ingest/comfyui 请求体示例</summary>

```json
{
  "runKey": "3f2b1c9e-8a44-4d1e-9c77-1a2b3c4d5e6f",
  "source": "ComfyUI",
  "status": "success",
  "comfyUrl": "http://127.0.0.1:8188",
  "outputDir": "D:\\Comfy-Desktop\\ComfyUI-Shared\\output",
  "prompt":     { "3": { "class_type": "KSampler", "inputs": { "seed": 42, "steps": 24 } } },
  "workflow":   { "nodes": [], "links": [] },
  "outputs": [
    { "filename": "x_00001_.png", "subfolder": "", "type": "output",
      "kind": "IMAGE", "nodeId": "9", "nodeType": "SaveImage" }
  ],
  "extra": { "clientId": "abc", "tags": ["ComfyUI"], "title": "雨夜霓虹" }
}
```

返回：

```json
{ "runKey": "3f2b...", "promptId": 42, "created": true, "mediaIds": [7, 8],
  "imported": 2, "duplicates": 0, "failed": 0, "alreadyCaptured": false }
```
</details>

### 6.6 几个设计上的取舍

- **为什么不改你的工作流**：捕获走的是 ComfyUI 自己的 `/history`，它本来就保存了 API 参数、`extra_pnginfo` 工作流和输出文件名。
  轮询这条路对 ComfyUI 是**只读**的，不装节点、不改图、不影响出图。
- **为什么工作流缺失时退回存 API 节点图**：agent / 脚本（例如 `scripts\anima-gen.ps1`）是直接 POST `/prompt` 的，
  它们从来没给过 ComfyUI 界面格式工作流，`/history` 里也就没有；但那份 API 节点图就是"当时到底跑了什么"的完整记录，
  ComfyUI 前端本来就能加载 API 格式 JSON（`loadApiJson`）。所以宁可存下来并在查看器里注明格式，也不要显示"没有工作流"。
- **为什么同时存两份 JSON**：API 图（`extra_params` 里那份）是"当时到底用什么参数跑的"，
  界面格式 workflow 是"能拖回 ComfyUI 直接复现的"。前者用于检索/对比，后者用于复现，两者不能互相替代。
- **为什么用 `prompt_id` 做幂等键**：ComfyUI 每次执行都会生成唯一的 `prompt_id`，
  重复轮询、刷新历史、后端重启后补捞，都只会命中同一条记录（`capture_runs.run_key` 唯一索引兜底）。
- **单次轮询最多处理几条**：`maxPerPoll`（默认 20）从最新往回取，避免库很空而历史很长时一次性灌进来几百条。
- **ComfyUI 在另一台机器**：填对方的地址即可，但 `outputDir` 用不了（那是本机路径），
  会走 `GET /view` 下载；建议把 `COMFYHUB_MAX_UPLOAD_MB` 和轮询间隔按网络情况调一下。

---

## 7. REST API

后端默认 `http://127.0.0.1:8080`，全部返回 JSON（UTF-8）。

### 提示词

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/api/prompts` | 列表 + 搜索。参数：`q` `tags`（逗号分隔）`tagMode=any\|all` `kind` `favorite` `hasMedia` `sort=newest\|oldest\|updated\|title\|favorite` `page` `size` |
| `POST` | `/api/prompts` | 新建（body 见下） |
| `GET` | `/api/prompts/{id}` | 详情（含标签、产物数量） |
| `PUT` | `/api/prompts/{id}` | 全量更新 |
| `DELETE` | `/api/prompts/{id}` | 删除（关联产物保留，仅解除关联） |
| `POST` | `/api/prompts/{id}/duplicate` | 复制一份 |
| `POST` | `/api/prompts/{id}/favorite` | body: `{"favorite": true}` |
| `POST` | `/api/prompts/{id}/tags` | body: `{"tags": ["赛博朋克"]}`，不存在的标签会自动创建 |
| `DELETE` | `/api/prompts/{id}/tags/{tagId}` | 移除单个标签 |
| `GET` | `/api/prompts/{id}/media` | 该提示词下的全部产物 |

<details>
<summary>POST /api/prompts 请求体示例</summary>

```json
{
  "title": "雨夜霓虹街道 · 赛博朋克",
  "kind": "IMAGE",
  "positivePrompt": "cyberpunk city street at night, heavy rain, neon signs...",
  "negativePrompt": "lowres, blurry, watermark",
  "checkpoint": "sd_xl_base_1.0.safetensors",
  "sampler": "dpmpp_2m",
  "scheduler": "karras",
  "steps": 32,
  "cfgScale": 7.5,
  "seed": 884213771,
  "width": 1216,
  "height": 832,
  "batchSize": 4,
  "loras": [{"name": "detail-tweaker", "weight": 0.8}],
  "extraParams": {"denoise": "0.55"},
  "notes": "雨夜氛围关键词组合",
  "favorite": true,
  "tags": ["赛博朋克", "写实", "8K"]
}
```
</details>

### 产物

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/api/media` | 列表 + 搜索。参数：`q` `tags` `tagMode` `kind` `promptId` `favorite` `untagged` `sort=newest\|oldest\|name\|largest\|favorite` `page` `size` |
| `POST` | `/api/media/upload` | **multipart**：`files`（可重复，多文件）、`promptId`、`title`、`kind`、`source`、`notes`、`tags`（逗号分隔）。自动按 SHA-256 去重 |
| `GET` | `/api/media/{id}` | 详情（含关联提示词的标题 / 正向 / 负向 / 模型 / seed / 标签） |
| `PATCH` | `/api/media/{id}` | `promptId` 关联、`clearPrompt:true` 解除、`title` `notes` `favorite` `source` |
| `DELETE` | `/api/media/{id}` | 删除记录 + 磁盘文件 + 缩略图 |
| `GET` | `/api/media/{id}/file` | 原始文件，`Accept-Ranges: bytes`（视频可拖进度，返回 206） |
| `GET` | `/api/media/{id}/thumb` | 缩略图（仅图片，最长边 512 的 JPEG；非图片返回 204） |
| `GET` | `/api/media/{id}/prompt` | 关联的完整提示词；未关联返回 204 |

### 标签 / 其他

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/api/tags` | 词表。参数：`q` `category` `sort=popular\|name\|newest` `limit` |
| `GET` | `/api/tags/categories` | 所有分类 |
| `POST` | `/api/tags` | 新建 |
| `PUT` | `/api/tags/{id}` | 重命名 / 改色 / 改分类 |
| `DELETE` | `/api/tags/{id}` | 删除（级联解除所有关联） |
| `GET` | `/api/stats` | 库统计 |
| `GET` | `/api/health` | 健康检查（含数据库连通性、存储目录） |

> ComfyUI 自动捕获相关的接口（配置 / 状态 / 轮询 / 目录导入 / 推送捕获 / 工作流原文）见 [6.5 捕获相关接口](#65-捕获相关接口)。

---

## 8. 数据库

库名 `comfy_hub`，字符集 `utf8mb4` / `utf8mb4_unicode_ci`，端口 **3307**（避开常见的 3306）。

| 表 | 作用 |
| --- | --- |
| `prompts` | 提示词主体 + 全部生成参数（`loras` / `extra_params` 为 JSON 列）；`source` / `source_ref` 记录来源，`workflow_json` 存工作流快照 |
| `tags` | 全局标签词表（`normalized` 唯一键，`use_count` 冗余计数） |
| `prompt_tags` | 提示词 ↔ 标签，多对多 |
| `media_assets` | 生成产物元数据（`prompt_id` 可空，`ON DELETE SET NULL`） |
| `media_tags` | 产物 ↔ 标签，多对多 |
| `capture_runs` | ★ 自动捕获的运行记录（`run_key` 唯一 = ComfyUI 的 `prompt_id`，`raw` 留一份 history 片段便于排查） |
| `app_settings` | ★ 运行期设置 k/v（自动捕获配置就存在这，App 与后端共用） |
| `v_media_full` | 视图：产物 + 关联提示词 + 聚合标签名 |

### 8.1 数据库放在哪（可以指定）

默认实例目录是项目下的 `.mysql\`（真正的数据文件在 `.mysql\data\`）。想换到别的盘/目录，三种办法都行：

```powershell
# 1) 命令行参数（推荐，一次到位：搬完自动改配置、记住新位置、重启）
pwsh -File scripts\mysql.ps1 move -DataDir 'D:\mysql-data\comfyhub'

# 2) 环境变量（写进系统环境变量后，所有脚本都按它走）
$env:COMFYHUB_MYSQL_DIR = 'D:\mysql-data\comfyhub'

# 3) 直接给某一条命令指定
pwsh -File scripts\comfyhub.ps1 up -DataDir 'D:\mysql-data\comfyhub'
```

App 里也能改：设置页 →「本地服务」→「MySQL 数据目录」→ 「迁移现有数据到该目录…」，
它会弹确认框，然后调用同一个 `mysql.ps1 move`。

解析顺序（从高到低）：

1. 命令行 `-DataDir`
2. 环境变量 `COMFYHUB_MYSQL_DIR`
3. 指针文件 `.mysql-location.json`（`move` 会写它，记住上次搬到的位置）
4. 默认 `项目\.mysql`

> `move` 只复制不删除：**源目录会保留**，确认新目录能正常启动、数据都在之后，自己删掉即可。
> 搬之前 MySQL 会被安全停掉，搬完自动拉起来。

### 8.2 结构升级

`db/schema.sql` 是全新安装用的；已经存在的库靠 **增量迁移**：

- 后端每次启动都会自动做一遍（见 `Migrate.kt`）：查 `information_schema` 再决定加不加列/表，幂等、可重复执行。
  所以 App 自动拉起后端时**不需要**你手动跑任何 SQL。
- 想手动升级或只想看改了什么：`pwsh -File scripts\mysql.ps1 migrate`（内容就是 `db\migrate.sql`）。

### 8.3 连接信息

应用账号：`comfyhub / comfyhub`（只授权 `comfy_hub.*`）。可用环境变量覆盖：

```powershell
$env:COMFYHUB_JDBC_URL='jdbc:mysql://127.0.0.1:3307/comfy_hub?...'
$env:COMFYHUB_DB_USER='comfyhub'
$env:COMFYHUB_DB_PASSWORD='comfyhub'
$env:COMFYHUB_STORAGE='D:\comfyhub-storage'
$env:COMFYHUB_PORT='8080'
$env:COMFYHUB_COMFY_URL='http://127.0.0.1:8188'
$env:COMFYHUB_COMFY_OUTPUT='D:\Comfy-Desktop\ComfyUI-Shared\output'
```

---

## 9. 一键自动构建任务

注册了一个 Windows 任务计划程序任务 **`ComfyHub-AutoRun`**（**仅按需触发**，没有定时器）：
运行 `scripts\autorun-app.ps1`，做「`comfyhub.ps1 up` → `flutter pub get` → `flutter analyze`
→ `flutter build windows` → 启动 `viewer.exe`」，全过程写入 `.run\autorun.log`。

```powershell
# 需要时手动触发一次
Start-ScheduledTask -TaskName 'ComfyHub-AutoRun'
Get-Content .run\autorun.log -Tail 60

# 查看任务
Get-ScheduledTask -TaskName 'ComfyHub-AutoRun' | Format-List

# 不再需要时删除
Unregister-ScheduledTask -TaskName 'ComfyHub-AutoRun' -Confirm:$false
```

> 提示：如果想让电脑每天 12:00 自动把 App 拉起来，可以给它加一个定时触发器：
> ```powershell
> $t = New-ScheduledTaskTrigger -Daily -At 12:00
> Set-ScheduledTask -TaskName 'ComfyHub-AutoRun' -Trigger $t
> ```

### 9.1 只改前端时用 debug 版热重载（省构建时间）

`autorun-app.ps1` 走的是 `flutter build windows`（**Release 完整构建，几分钟**）。
只改 `lib/` 下的 Dart 前端代码时不要每次都走它 —— 用 debug 版跑一次，之后按 `r` 热重载：

```powershell
pwsh -File scripts\dev-app.ps1
# 等价于：确保 MySQL + 后端在跑 → flutter run -d windows --debug
# 跑起来之后在终端里：r 热重载（保留状态） / R 热重启 / q 退出
```

| 改了什么 | 用什么 | 大概多久 |
| --- | --- | --- |
| `lib/` 里的 Dart 代码 | `scripts\dev-app.ps1` → 之后按 `r` | 首次 30~60s，热重载 1~2s |
| `pubspec.yaml` 依赖 / `windows/` 原生代码 | `scripts\dev-app.ps1`（会重新编译）/ `autorun-app.ps1` | 分钟级 |
| 要出正式产物 / 交给别人用 | `scripts\pack-release.ps1`（Release + 后端 + MySQL + JRE 装配成一个目录） | 几分钟（首次拷 MySQL 会久一点） |

- debug 产物在 `build\windows\...\runner\Debug\`，Release 在 `...\Release\`，两者互不影响；
  脚本启动 App 时**优先挑 Release**，所以做过 debug 调试也不会误启动一个 debug 版。
- 后端不用管：App 启动时自己会拉起（见第 5 节），`dev-app.ps1` 也会先探一次。

### 9.2 出发布包：`pack-release.ps1`

`flutter build windows --release` 的产物（`build\windows\x64\runner\Release\`）**只有一个 App**，
里面没有后端、没有数据库、没有 Java 运行时 —— 直接把这个目录拷给别人跑不起来：

- Kotlin 后端在 `server\build\install\` 下，那是 Gradle 的**构建中间产物**，不会跟着 Release 目录走；
- MySQL 的 `mysqld.exe` 以前只在 `D:\tools\mysql\...` 这种**本机绝对路径**下找，换台机器直接崩；
- 后端是 JVM 程序，不能假设目标机器装了 JDK。

所以「交给别人用」要走 `pack-release.ps1`。它按 `packaging\manifest.json`（**软件打包清单**）
把这几样装配到同一个目录：

```powershell
pwsh -File scripts\pack-release.ps1          # → dist\ComfyHub\
pwsh -File scripts\pack-release.ps1 -Zip     # 顺带压成 dist\ComfyHub.zip
```

| 清单里的件 | 发布包里的位置 | 说明 |
| --- | --- | --- |
| Flutter App | `<根>\viewer.exe` + `data\` | 必须铺在**根目录**：App 从 exe 所在目录往上找 `scripts\comfyhub.ps1` 来认根目录 |
| Kotlin 后端 | `<根>\server\` | `gradle installDist` 的 `bin\` + `lib\`（入口 `bin\comfy-hub-server.bat`） |
| Java 运行时 | `<根>\jre\` | 优先 `jlink` 出精简运行时；本机 JDK 没有 `jmods`（Android Studio 的 JBR 就没有）就整份拷 |
| 便携版 MySQL | `<根>\mysql\` | 默认剪掉调试符号等：`mysqld.pdb` 一个就 368MB，裁完约 400MB（`-NoPrune` 可关） |
| 生命周期脚本 | `<根>\scripts\` | **整个目录**都要：`comfyhub.ps1` 依赖 `mysql.ps1` / `server.ps1` / `silent-process.ps1` / `watch-owner.ps1` |
| SQL 脚本 | `<根>\db\` | 首次初始化要导入 `schema.sql` |

#### 运行时依赖：发布包带不走的那几样

打包只把 **Java 运行时**和 **MySQL 本体**装进了包里，其余得目标机器自己装 ——
缺了服务起不来，而且报错往往没头没尾（`mysqld 启动超时` / `pwsh 不是内部或外部命令`）。
`scripts\comfyhub.ps1 doctor` 会把它们逐个报出来；`up` 失败时也会自动提示缺哪个、怎么装
（缺的都在 `packaging\manifest.json` 的 `runtimeRequirements` 里登记）。

| 依赖 | 谁需要它 | 随包携带？ |
| --- | --- | --- |
| **PowerShell 7 (`pwsh`)** | App 用它执行 `scripts\*.ps1`，脚本之间也**互相调 `pwsh`** | ❌ 要装：`winget install --id Microsoft.PowerShell` |
| **VC++ 2015-2022 x64 可再发行组件** | `mysqld.exe` 依赖 `vcruntime140.dll` / `vcruntime140_1.dll` / `msvcp140.dll`，而便携版 MySQL 的 zip **不带**这几个 DLL | ❌ 要装：`winget install --id Microsoft.VCRedist.2015+.x64` |
| Java 21 运行时 | Kotlin 后端（JVM） | ✅ `<根>\jre` |
| MySQL 8.4（免安装版） | 数据库本体 | ✅ `<根>\mysql` |
| Windows 10 / 11 x64 | 桌面端 + 脚本（WMI / CIM） | — |

> **PowerShell 版本是个坑**：Windows 自带的 `powershell.exe` 是 5.1，**不算数** ——
> `comfyhub.ps1` / `mysql.ps1` / `server.ps1` 之间有 15 处 `& pwsh -NoProfile -File ...` 互调，
> 机器上只有 5.1 时这些子调用会失败。App 若只找到 5.1，会在启动页日志里明确告警并给出安装命令。

**发布包是便携式的**（免安装、免管理员）：解压出来整个目录就是运行时根目录，可整体搬到任意盘/目录。
可写数据都落在根目录内，卸载 = 删目录：

| 用途 | 默认位置 | 怎么改 |
| --- | --- | --- |
| 数据库实例 | `<根>\.mysql\`（`data\` + `my.ini` + 错误日志） | `pwsh -File scripts\mysql.ps1 move -DataDir <新位置>`；或临时用 `-DataDir` / `COMFYHUB_MYSQL_DIR` |
| 生成产物 | `<根>\storage\`（`media\` + `tmp\`） | 后端启动时由 `server.ps1` 显式设成 `COMFYHUB_STORAGE`；要换位置就预先设这个环境变量 |
| 日志 / PID | `<根>\.run\` | — |

拿到包的人双击 `viewer.exe` 即可：App 会自己把 MySQL + 后端拉起来，首次会自动初始化数据库（稍等一会儿）。
命令行等价写法 `pwsh -File scripts\comfyhub.ps1 up -WithApp`；体检 `... doctor`；全停 `... down`。

> 装配完脚本会**自检** `viewer.exe`、`server\bin\comfy-hub-server.bat`、`mysql\bin\mysqld.exe`、
> `jre\bin\java.exe`、`db\schema.sql` 是否都在，缺任何一个都以退出码 1 结束 ——
> 不会悄悄给你一个跑不起来的包。同样的说明也写在包里的 `BUILD-INFO.txt`。

为了让同一个脚本在**源码树**和**发布包**两种布局下都能用，路径解析都做成"两边都认"：

| 找什么 | 源码树 | 发布包 |
| --- | --- | --- |
| 后端启动脚本 | `server\build\install\comfy-hub-server\bin\` | `<根>\server\bin\`（`server.ps1` 会因此跳过 Gradle 构建） |
| `mysqld.exe` / `mysqladmin.exe` | `D:\tools\mysql\...` | `<根>\mysql\bin\`（`mysql.ps1` / `comfyhub.ps1` / `server.ps1` 三处都优先认它） |
| Java | 本机 JDK 21~23 | `<根>\jre`（`server.ps1` 的 `Resolve-Jdk` 优先用它） |
| `viewer.exe` | `build\windows\...\runner\Release\` | `<根>\viewer.exe` |

---

## 10. 界面与中文排版

Flutter 在 Windows 上的默认字体族是 **Segoe UI**，它**不含中文字形**，中文只能靠引擎的系统兜底
去挑字体。结果是中英混排风格不统一、中文偏细发虚；再加上 Material 3 的字号/字距是按
Roboto（拉丁字体）调的（`bodyMedium` 14px + `letterSpacing: 0.25`），套到方块字上又小又挤。

所以字体统一收在 `lib/core/theme.dart` 里，做了三件事：

1. **显式指定中文字体族**，并按平台给兜底链：
   Windows → `Microsoft YaHei UI`；Linux → `Noto Sans CJK SC`；
   macOS/iOS/Android 交给系统（自带的中文都不错）。
   兜底链：`Microsoft YaHei UI → Microsoft YaHei → PingFang SC → Hiragino Sans GB →
   Noto Sans CJK SC → Source Han Sans SC → WenQuanYi Micro Hei → SimHei → sans-serif`。
2. **字距归零、行高放到 1.55~1.6** —— 拉丁字体的字距放到中文上会显得散，中文也需要更大的行距才不挤。
3. **小字号统一加字重**（正文 400、标签 500、标题 600~700），让笔画在低分屏上也立得住；
   正文从 14 → 14.5，`labelSmall` 从 11 → 12。

顺带把 NavigationRail / NavigationBar / Chip / Tooltip 的标签样式也一起在主题里定死，
避免各处再写零散的 `TextStyle`。缩略图上的文件名额外加了黑色投影，保证在任意底图上都可读。

### 10.1 系统菜单也要中文（`MaterialApp.locale`）

App 自己的文案都是中文，但文本框右键的「复制 / 全选 / 剪切 / 粘贴」、返回按钮的 tooltip
这些**不是 App 写的**，而是 `MaterialLocalizations` 提供的。不接 `flutter_localizations`、
不指定 `locale: zh_CN`，这些菜单就会是英文的 Copy / Select all —— 中文界面里混着英文很像 bug。
所以 `lib/app.dart` 里显式装了三个 delegate，并把 locale 钉成 `zh_CN`。

### 10.2 列表按窗口宽度分列（列数 = 宽度 / 550）

桌面端窗口能拉得很宽，单列列表会把一行拉到一米长，右边还空着一大片。
`lib/widgets/adaptive_layout.dart` 提供两个复用组件，规则都是
**列数 = 可用宽度 / 550**（最多 4 列，窄了自动退回单列）：

| 组件 | 用在哪 | 排法 |
| --- | --- | --- |
| `AdaptiveColumnList` | 提示词库、标签（卡片高度接近） | 按**行**排，同一行的卡片用 `IntrinsicHeight` 对齐到最高的那一张，横着看是齐的 |
| `AdaptiveColumns` | 设置页（卡片高度差几倍） | 按行排会留下一大片空白，所以改成"每块依次放进当前最矮的一列" |

550 是"一行中文提示词读起来不费劲"的经验值：再宽眼睛就要来回扫。新增列表页时直接套这两个组件，
不要自己写 `ListView` 的单列布局。

---

## 11. 测试

```powershell
$env:PUB_HOSTED_URL='https://pub.dev'
flutter analyze     # 无任何 error / warning / info
flutter test        # 52 个用例
```

| 文件 | 覆盖内容 |
| --- | --- |
| `test/widget_test.dart` | 模型 JSON 解析（Prompt / MediaAsset / 分页 / 捕获配置）、`formatSize` / `formatDuration` / `ellipsis` / 颜色解析等纯逻辑 |
| `test/media_prompt_flow_test.dart` | **核心闭环**：用 `MockClient` 假造后端，验证「画廊渲染 → 点开产物 → 详情页显示关联提示词全文与参数 → 点标题跳提示词详情 → 点标签进入标签搜索页」，并断言每一步发出的 HTTP 请求；另有一条详情页布局断言（大图走可缩放查看器、文件信息铺满整栏） |
| `test/home_nav_test.dart` | **首页落地页**：断言打开 App 落在「画廊」页、导航顺序是 `画廊 → 提示词 → 标签 → 设置`、点第二个才进提示词库 |
| `test/workflow_viewer_test.dart` | 工作流查看器：格式化展示 / 204 空状态 / 复制全文 / 错误重试、界面格式与 API 格式的提示语区分，以及两个详情页的接线（按钮只在有工作流时出现） |
| `test/zoomable_image_test.dart` | **大图查看器**：滚轮缩放（含上下限）、放大后拖动平移、缩略图只在放大后出现且高亮框跟着视野走、点缩略图跳转、适应窗口复位、图片加载失败兜底 |
| `test/adaptive_layout_test.dart` | **多列布局**：列数规则（宽度 / 550、上限 4 列、异常宽度退回单列）、宽窗口排两列 / 窄窗口退回单列、设置页那种瀑布流把块放进最矮的一列 |
| `test/context_menu_test.dart` | **右键菜单**：画廊缩略图右键弹出「关联提示词 / 收藏 / 删除」并真的发出 PATCH / DELETE、删除前必须确认；详情页右键图片弹出复制项，复制到剪贴板的是完整文件地址 / 提示词全文 |
| `test/prompt_batch_test.dart` | **提示词批量管理**：宽窗口分两列；多选后批量收藏（只打勾的那几条）、批量加标签（走追加标签接口）、批量删除（先确认再逐条 DELETE） |
| `test/tags_layout_test.dart` | 标签页在宽窗口分两列，超长分类 / 说明只截断一行，不会把固定高度的卡片撑破 |
| `test/settings_layout_test.dart` | 设置页宽窗口分列；开关行（「开启自动捕获」这些）左右都留出内边距，不贴卡片边缘 |
| `test/localization_test.dart` | App 装的是 `zh_CN` 的 Material 本地化：选择菜单是「复制 / 全选 / 剪切 / 粘贴」而不是 Copy / Select all |
| `test/comfyui_capture_test.py` | ComfyUI 捕获节点的纯逻辑（payload 组装、类型判定、重试），见 `docs/comfyui-capture.md` |
| `server/src/test/kotlin/.../GraphParseTest.kt` | **参数解析器的回归测试**：经典 KSampler 图、真实的自定义采样链（MiniMax H3 那种）、空图/坏图、只有 `text_g`/`text_l` 的图。`pwsh -File scripts\server.ps1 test` |
| `server/src/test/kotlin/.../HistoryEntryTest.kt` | **`/history` 记录解析的回归测试**：ComfyUI 0.34.2 的六元组、老版本三元组、带界面工作流 / 不带、坏数据不抛异常 —— 钉住"提示词与工作流整条丢失"那个坑 |

### 自动捕获的端到端验证

不用真的跑一次生成 —— 用假的 ComfyUI 把整条链路走一遍，并逐项断言：

```powershell
pwsh -File scripts\e2e-capture-test.ps1
# 轮询捕获 → 幂等 → 目录导入（读 PNG 内嵌元数据）→ 推送捕获
# 跑完自动清理测试数据；加 -KeepData 可以保留下来在界面上看效果
```

后端接口的端到端验证（真实 MySQL + Ktor）也可以用 curl：

```powershell
# 上传一张图并关联到提示词 1
curl.exe -X POST http://127.0.0.1:8080/api/media/upload `
  -F "files=@samples\neon-street.png" -F "promptId=1"

# 按标签搜索提示词
curl.exe "http://127.0.0.1:8080/api/prompts?tags=%E8%B5%9B%E5%8D%9A%E6%9C%8B%E5%85%8B"

# 视频拖动进度依赖的 Range 请求（应返回 206）
curl.exe -o NUL -w "%{http_code}`n" -H "Range: bytes=0-999" http://127.0.0.1:8080/api/media/1/file

# 自动捕获：看一眼状态、手动同步一次
curl.exe http://127.0.0.1:8080/api/capture/status
curl.exe -X POST http://127.0.0.1:8080/api/capture/poll
```

---

## 12. 本机环境踩坑记录

这些都是搭建时实际遇到并已绕开的，换机器时可以直接参考：

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| `flutter pub get` 报 `424 Failed Dependency ... synchronized` | `PUB_HOSTED_URL=https://pub.flutter-io.cn` 镜像对个别包返回 424 | 改用 `https://pub.dev`（本机可直连） |
| `Could not resolve plugin org.jetbrains.kotlin.plugin.serialization` | `plugins.gradle.org` 不可达，而 Kotlin 插件 marker 不在 Maven Central | `build.gradle.kts` 改用 `buildscript { classpath(...) } + apply(plugin=...)` 从 Maven Central 拉取 |
| Gradle 报 `Unsupported class file major version 69` | 本机默认 JDK 25，Gradle 8.12 只支持到 JDK 23 | `scripts\server.ps1` 自动挑选 JDK 21~23（本机用了 Android Studio 自带 JBR 21） |
| `Unknown or incorrect time zone: 'Asia/Shanghai'` | 便携版 MySQL 没有导入时区表 | JDBC URL 去掉 `connectionTimeZone`，用驱动默认的 `LOCAL` |
| `Unsupported character encoding 'utf8mb4'` | Connector/J 不认 `characterEncoding=utf8mb4` | 改成 `characterEncoding=UTF-8`（驱动会自动映射到 utf8mb4） |
| `ERROR 1130 Host '127.0.0.1' is not allowed to connect` | my.ini 里开了 `skip-name-resolve`，`root@localhost` 匹配不上 TCP 连接 | 去掉 `skip-name-resolve` |
| 命令行结束后 mysqld / java 立刻消失 | `Start-Process` 启动的进程属于当前命令的进程树，命令结束被一起回收 | 改用 `Win32_Process.Create`（WMI）启动，脱离进程树；后端再包一层临时 `.cmd` 处理 `cd` / `set` / 输出重定向 |
| 改用 WMI 之后，App 自动拉起服务时**闪出一个 cmd 黑框** | WMI 的提供程序进程（WmiPrvSE）自己没有控制台，`CreateProcess` 没带 `CREATE_NEW_CONSOLE` 时系统会给控制台程序**新分配一个可见的控制台窗口** | 给 WMI 传一个 `Win32_ProcessStartup{ShowWindow=0}`（SW_HIDE）：窗口在创建时就是隐藏的（不是"先显示再隐藏"，所以不闪）；`cmd → .bat → java.exe` 这类孙进程会继承这个隐藏控制台，也不会各弹一个。见 `scripts\silent-process.ps1` |
| 想一步到位用 `CreateFlags = CREATE_NO_WINDOW` | WMI 的 `Win32_ProcessStartup.CreateFlags` **拒绝**这个值 | `InvokeMethod('Create', …)` 返回 `21`（Invalid parameter）。所以只设 `ShowWindow = 0`；`[wmiclass]`（System.Management）不可用时退到 `wscript` + 临时 `.vbs`（`WScript.Shell.Run cmd, 0, False`），仍然拿 `ShowWindow`，最后才退回裸 WMI（会弹窗但服务能起来） |
| 同一个 WMI 调用：命令行手敲成功、放进脚本却返回 `21` | PowerShell 里**没传值的 `[string]` 参数是空串，不是 `$null`**；WMI 的 `Create` 一收到 `CurrentDirectory=''` 就报 `21`（Invalid parameter） | 空串统一转回 `$null`（`silent-process.ps1` 里的 `$curDir`）—— "手敲行、脚本不行"时先怀疑参数类型的默认值 |
| 用 `Get-Process xxx \| Select MainWindowHandle` 判断"有没有弹窗口"永远得到 0 | Windows 的控制台窗口属于 **conhost.exe**，不属于那个控制台程序本身 | 验证「静默启动」要枚举可见的顶层窗口（`EnumWindows` + `IsWindowVisible`），并且**边启动边轮询**（50ms 一次）才能发现"闪一下"的窗口 —— 现成的一条命令：`pwsh -File scripts\check-silent-start.ps1 -Restart` |
| 静默启动的兜底路径（`wscript` + `.vbs`）起不来 mysqld | 两个坑叠在一起：`WScript.Shell.Run` 直接跑「"带引号的完整路径" + 参数」时会把 mysqld 悄悄丢掉（交给 `cmd` 解析才稳）；命令行里的引号 / 中文路径作为 wscript 参数传递时会被拆坏 | 命令行写进一个 UTF-16 文件，VBS 读出来再交给 `cmd.exe /c "…"` 执行；启动方式见 `Start-SilentProcess` 的返回值（`wmi-hidden` / `wmi-vbs` / `wmi-plain`） |
| 后端存储目录跑到 `C:\Windows\System32\...` | WMI 启动时工作目录丢失 | 启动脚本里显式 `cd /d` + `set COMFYHUB_STORAGE=...`；后端还加了系统目录兜底检查 |
| `media_kit` 无法构建 | 它的 Windows 原生库要从 GitHub Releases 下载，本机 GitHub 不可达 | 视频改用 `video_player_win`（Windows Media Foundation，只依赖 nuget.org，本机可用）；音频用 `audioplayers` |
| `main.cpp` 报 `C4819` / `C2001 常量中有换行符` | 窗口标题是中文，而 MSVC 默认按 936(GBK) 代码页读 UTF-8 源码，`/WX` 又把警告升级成错误 | `windows/CMakeLists.txt` 里加 `add_compile_options("/utf-8")` |
| `FilePicker.platform` / `PlatformFile.size` 找不到 | file_picker 12.x 把 `pickFiles` 改成了**静态方法**并返回 `List<PlatformFile>`，`size` 换成 `lengthSync()`/`length()` | 按新 API 改写 `upload_sheet.dart` |
| 详情页「正向提示词」的框被撑到几百像素高 | `SelectableText` 底层是 `EditableText`，设了 `maxLines: 30` 会**预留** 30 行高度 | `CopyableText` 不设 `maxLines`，按内容自然撑开 |
| 用 PowerShell 点击 App 坐标总是偏 | pwsh 进程 DPI 不感知，`SetCursorPos` 坐标被系统按缩放比换算（本机 150%） | 脚本里先调用 `SetProcessDPIAware()`，坐标统一用物理像素 |
| 大文件上传内存暴涨 | 早期实现把整个 multipart 读进内存 | 改为流式落盘（`ByteReadChannel` → 临时文件 → 原子 `move`） |
| `PrintWindow` 抓 Flutter 窗口抓到错帧 | Flutter 走 ANGLE/D3D 合成，`PrintWindow` 对这类窗口不可靠 | 改用 `SetForegroundWindow` + `CopyFromScreen` 抓真实屏幕像素 |
| `loras` / `extra_params` 静默存成 `NULL` | 早期写成 `encode(value: Any)`：reified 的 `T` 退化成 `Any`，运行时找不到序列化器抛异常，而异常又被 `runCatching` 吞掉 —— 不报错但也没写进去 | 改成显式传序列化器：`encodeToString(ListSerializer(LoraRef.serializer()), …)` / `MapSerializer(…)`。"会吞异常的 runCatching + 泛型退化" 是个必须留意的组合 |
| `ALTER TABLE … ADD COLUMN IF NOT EXISTS` 报语法错误 | MySQL 8.4 **不支持** `IF NOT EXISTS`（那是 MariaDB 的扩展） | 先查 `information_schema.COLUMNS / STATISTICS / TABLES` 再执行 DDL（`Migrate.kt` / `db\migrate.sql`），幂等可重复执行 |
| 换了 `datadir` 之后 mysqld 起不来 | 新目录是空的，里面没有 `mysql` 系统库，mysqld 直接退出 | `mysql.ps1 move` 先停库 → `robocopy` 整个实例目录过去 → 再拉起来；不删源目录，留一条后路 |
| App 里点「启动服务」没反应 / 一闪而过 | 脚本是子进程，工作目录会丢，进程还可能随父进程被回收 | 脚本内部用 WMI（`Win32_Process.Create`）脱离进程树；Flutter 侧用 `Process.start` 流式读 stdout/stderr 实时回显到启动页，超时/失败都有明确文案 |
| 捕获节点挂上了 `add_on_prompt_handler` 却拿不到 `prompt_id` | ComfyUI 0.34.2 里 `prompt_id` 是在回调**之后**才生成/校验的（`server.py:1076` 调回调，`server.py:1088` 才有 id） | 主钩子改成包装 `PromptExecutor.execute_async(prompt, prompt_id, extra_data, execute_outputs)`（`execution.py:730`）：这里同时拿得到 id、参数图和 `extra_data`，而且自带 `finally`，执行失败也能收尾 |
| 包装 ComfyUI 的 `send_sync` 之后事件链路出问题 | 忘了原样转发 `*args/**kwargs` 或改动了返回值 | 包装器只做旁路观察：`*args, **kwargs` 原样透传、返回值原样返回，内部逻辑全部 try/except（`comfyui\comfyhub_capture\__init__.py`） |
| `VHS_VideoCombine` 生成的视频没被捕获 | 它默认把视频写成 `type=temp`（预览临时目录），而自动捕获只收 `output` 类产物 | 工作流里把 `save_output` 打开（或在节点配置里设 `includeTemp: true`），这类视频才会进库 |
| `server.ps1` 在数据库起不来时仍然硬拉起后端 JVM | PowerShell 里 `& script.ps1` 的 **stdout 会成为表达式的返回值**，`$mysqlOk` 被输出数组污染后永远为真，`if (-not $mysqlOk)` 形同虚设 | 调用处加 `| Out-Null`（`Ensure-MySql` 只关心退出/返回值），失败时直接抛「数据库不可用」而不是留一个连不上库的后端 |
| `mysqld` 相关判断时好时坏 | Windows 上 MySQL 8 是**父进程 + 真正的服务子进程**两个 `mysqld.exe`；`CommandLine` 里的路径还可能带引号 | 匹配命令行时两边都去引号再比；状态行打印完整 PID 列表（以前只打进程个数，显示成 `PID 2` 很误导） |
| App 启动页日志刷 `FormatException: Missing extension byte (at offset 12)`，中文变乱码 | pwsh 在 stdout 被**重定向**时跟随控制台代码页输出（中文系统是 936/GBK），而 App 是按 UTF-8 解的 —— 报错 offset 正好落在第一个中文字符上（`  ComfyHub 启…` 的 `启`） | 三个脚本顶部在 `[Console]::IsOutputRedirected` 时把 `[Console]::OutputEncoding` 钉成 UTF-8（不动用户的交互式终端）；App 侧解码再加 `allowMalformed` 兜底，一行坏字节不再打断整个日志流。实测 CP=936 下：旧脚本 `ce b4 d4 cb`（GBK），新脚本 `e8 bf 90 e8 a1 8c`（UTF-8） |
| 冷启动要 10 秒往上，其中一多半是在"干等" | "服务起来没有"的探测直接去连 127.0.0.1 上一个没人监听的端口，本机要等 **SYN 重传 ≈ 2 秒**（`mysqladmin ping` / `Invoke-RestMethod` / 裸 `TcpClient` 实测都是 2.04~2.08s；开着 Clash/mihomo 这类 TUN 代理时 RST 被吃掉更明显），而冷启动时"还没起来"恰恰是常态，一次启动里要撞好几次 | 所有存活探测先过 `Test-TcpPort`（`BeginConnect` + 200ms 超时，端口开着时和正常连接一样快），确认端口开了再去做权威的 ping / 健康检查；轮询间隔 700/800ms 收紧到 200ms。实测 `up -SkipBuild`：**10.8~11.2s → 7.6~7.7s**（后台耗时未变：MySQL 1.7s + JVM 1.9s 是真实启动时间） |
| 自动捕获的提示词是空的（真实的视频工作流） | 参数解析只认「采样器自己身上有 `steps`/`cfg`」的经典图；而 MiniMax H3 这类图是 `SamplerCustomAdvanced` + `BasicGuider` + `BasicScheduler` + `KSamplerSelect` + `RandomNoise`，参数散在兄弟节点上，文本提示词在条件节点的 `prompt` 输入里 | 改成顺着 `sigmas` / `sampler` / `noise` / `guider` 把链走一遍，并统一从「条件节点」的文本输入取提示词；补了 `GraphParseTest` 把这几个真实结构钉住 |
| **用 agent / 脚本生图时，提示词与工作流双双为空** | ComfyUI **0.34.2** 把入队的 prompt 从 3 元组扩成了 6 元组 `(number, prompt_id, prompt, extra_data, outputs_to_execute, sensitive)`（`server.py:1131`），而 `/history` 存的就是这一整条；旧实现按 `prompt[0]` / `prompt[1]` 取节点图和 `extra_data`，升级后取到的是数字和 prompt_id 字符串 → `GraphParse` 拿到 null，提示词、参数、工作流一起丢 | 改成**按内容认**（值全是 `{class_type: ...}` 的对象才是节点图，带 `extra_pnginfo`/`client_id` 的才是 extra_data），新旧两种形状都吃得下；`HistoryEntryTest` 把六元组 / 三元组 / 坏数据都钉住（见 `ComfyCapture.kt` 的 `HistoryEntry`） |
| agent / 脚本生图"没有工作流" | 这类运行是直接 POST `/prompt` 的，从没给过 ComfyUI 界面格式工作流，`/history` 里自然没有 —— 但它提交的 **API 格式节点图**一直躺在 history 里 | 捕获时工作流缺失就退回存 API 格式节点图（ComfyUI 前端 `loadApiJson` 能直接加载这种 JSON），查看器按格式给不同提示语 |
| 产物详情页永远不显示「查看工作流」 | `MediaDto` 里压根没有 `hasWorkflow` 字段（提示词有），前端 `media.hasWorkflow` 恒为 false | `MediaRepo` 的查询补上 `(m.workflow_json IS NOT NULL) AS has_workflow` |
| 详情页大图"拖不动、滚轮不缩放" | 旧实现把 `Image.network` 直接塞进 `InteractiveViewer`，再套在外层 `SingleChildScrollView` 里：图片按原始尺寸撑开、高度不受限，于是一放大就变成"整页滚动"而不是"画面平移" | 抽出 `ZoomableImageView`：视口有界、图片 `contain` 铺满，滚轮以指针为中心缩放、按住拖动平移，右下角加缩略图 + 视野高亮框（`test/zoomable_image_test.dart` 覆盖） |
| 「文件信息」的值挤在卡片左半边 | 单列键值对在一张宽卡片里，右边一大片空白 | 卡片宽 ≥ 460 时排成两列（文件名 / 备注这类长值仍独占整行），把整张卡片铺满 |
| 删掉导入的产物后，再点「导入已有产物」永远提示重复 | `capture_runs.run_key = import:<sha>` 只是个日志，却把重新导入挡在门外（媒体行其实已经删了） | 调用前先按 SHA-256 确认文件不在库里；导入这条路用 `force` 抢占运行记录（**正在处理中的**运行仍不会被抢），轮询那条路保持原样 —— 用户删掉的捕获不会被下一次轮询偷偷加回来 |
| 后端在跑但 MySQL 挂了，`server.ps1 start` 什么都不修 | 它看到后端进程存在就提前 return，跳过了数据库检查 | 统一入口 `comfyhub.ps1 up`：先确保数据库，再判断后端 `db` 是否 ok，不 ok 就重启后端 |
| 脚本报 `Cannot overwrite variable HOME` | `$home` 是 PowerShell 的只读自动变量，不能当普通变量赋值 | 改名成 `$mysqlHome` / `$javaHome` |
| `Get-ChildItem 'D:\tools\mysql\*\bin' -Filter x.exe` 静默返回空 | 路径里带通配符时文件系统提供程序不会正确应用 `-Filter` | 改成先 `-Directory` 列目录再拼 `bin\x.exe` |
| 中文又细又糊 | Flutter 在 Windows 默认用 Segoe UI（无中文字形），且 M3 字号字距按拉丁字体调 | 见第 10 节：显式指定中文字体族 + 字距归零 + 行高 1.6 + 小字号加字重 |
| `flutter build apk` 报 `Error: Your project's Kotlin version (2.2.10) is lower than Flutter's minimum supported version of 2.2.20` | AGP 9 的内建 Kotlin 用 AGP 运行时依赖的 KGP，而 AGP 9.1.0 只带 **2.2.10**；把 `org.jetbrains.kotlin.android` 从 settings 里删掉后 classpath 上就只剩它，低于 Flutter 3.47 的门槛（flutter/flutter#192167，3.47.3 还没带修复） | `settings.gradle.kts` 里保留 `id("org.jetbrains.kotlin.android") version "2.4.0" apply false`：只进 classpath、不 apply，内建 Kotlin 就用 2.4.0，也不会抢 `kotlin` 扩展名 |
| 构建失败时 Flutter 额外弹框说 "opt out of `android.newDsl`"，但 `android.newDsl=false` 早就配了 | `flutter_tools` 的 `useNewAgpDslErrorHandler` 只按 `> Failed to apply plugin 'dev.flutter.flutter-gradle-plugin'` 这一行文本匹配，**apply 阶段任何异常都会命中**，是误报 | 别理这个框，往上看真正的 `* What went wrong:` |
| 插件模块 `compileDebugKotlin` 报 `Could not close incremental caches`，被 Suppressed 的真因是 `this and base files have different roots` | pub 缓存在 **C:** 盘、工程在 **D:** 盘；Kotlin 增量编译的可重定位缓存要把源文件路径相对于工程目录转相对路径，跨盘直接 `IllegalArgumentException`（flutter/flutter#187225 也是它，最后没定位到根因就关了） | `android/gradle.properties` 加 `kotlin.incremental=false`（代价是 Kotlin 不再增量编译）；彻底解决就把 `PUB_CACHE` 挪到 D 盘 |
| Gradle 报 `Could not GET 'https://dl.google.com/...'` / `repo.maven.apache.org` + `TLS protocol versions` / `Remote host terminated the handshake` | `~/.gradle/gradle.properties` 里配的本机代理 `127.0.0.1:7890` 对这两个仓库 TLS 握手失败，**直连反而正常**；但 github.com 直连不通、必须走代理，所以代理不能整个关 | `~/.gradle/gradle.properties` 加 `systemProp.http.nonProxyHosts=dl.google.com\|*.google.com\|repo.maven.apache.org\|*.maven.apache.org\|...`（Java 只有 `http.nonProxyHosts` 一个属性，https 同样生效），改完先 `android\gradlew.bat --stop` |
| 升 Gradle 时构建刚起头就抛 `SSLHandshakeException`，栈里是 `org.gradle.wrapper.Download.downloadInternal` | Gradle 官方分发包地址会 **307 跳到 github.com**，本机 github 直连不通；而 **Gradle wrapper 跑在 Gradle 之前、不读 `~/.gradle/gradle.properties` 里的 `systemProp.*` 代理设置** | 从 `mirrors.cloud.tencent.com/gradle/`（或华为镜像）下 `gradle-x.y.z-all.zip` → 和官方 `.sha256` 核对 → 塞进 `~/.gradle/wrapper/dists/<版本>/<hash>/`；再在 `gradle-wrapper.properties` 里加 `distributionSha256Sum` 防掉包。完整步骤见 [docs/android-agp9-builtin-kotlin-migration.md](docs/android-agp9-builtin-kotlin-migration.md) 的踩坑 P8 |
| 升级 AGP 后构建报"需要更高的 Gradle 版本" | AGP 每个版本都有最低 Gradle 要求（9.1→9.3.1、9.2→9.4.1、9.3→9.5.0、9.4→9.6.0），AGP 与 Gradle 必须配对升级 | 查 AGP release notes 顶部的"最低版本"表，同时改 `android/settings.gradle.kts` 和 `android/gradle/wrapper/gradle-wrapper.properties` |
| 删掉 `android.builtInKotlin=false` 想打开内建 Kotlin，结果又被写回 `false` | Flutter 的 `DisableBuiltInKotlinMigration` 只在属性**完全缺失**时才补 `=false`（`android.newDsl` 同理） | 必须显式写 `android.builtInKotlin=true`，属性名大小写敏感；`android.newDsl` 保持 `false` 是官方现状（Flutter 自己也没迁完新 DSL），不是漏迁移。细节见 [docs/android-agp9-builtin-kotlin-migration.md](docs/android-agp9-builtin-kotlin-migration.md) |

---

## 13. 已知限制

- **视频内嵌播放仅 Windows**：依赖 `video_player_win`（Media Foundation）。其他平台会显示提示并引导用系统播放器打开；视频能否播放取决于系统已装解码器（AV1 / H.265 需另装）。
- **没有视频缩略图**：服务端不调用 ffmpeg，因此视频格子在画廊里显示的是播放图标占位，不是首帧。
- **中文检索用 LIKE 而非 FULLTEXT**：MySQL 默认分词器对中文支持差（`ngram` 需额外配置），所以用多词 AND 的 `LIKE` 匹配。
- **自动捕获有轮询延迟**：默认 4 秒一次（可调到 1 秒），想要零延迟就装自定义节点（见 6.2）。
- **只有 PNG 能还原提示词**：历史导入时，PNG 内嵌的 `prompt` / `workflow` 才能反推出参数；
  视频 / 音频 / 被重新压缩过的图只会作为「未关联产物」入库（在画廊里用「未关联」筛出来就能看到）。
- **`VHS_VideoCombine` 默认写到 temp**：这类视频默认不进库，要么在工作流里打开 `save_output`，
  要么在 `comfyui\comfyhub_capture\config.json` 里设 `includeTemp: true`（推送通道）——**轮询通道只认 `output`**，
  因为 `/history` 里 temp 产物随后会被 ComfyUI 清掉，存了也是死链。
- **能解析多少参数取决于工作流本身**：解析器走的是「图里有什么就取什么」。
  如果某个自定义节点根本没把分辨率/CFG 放进节点图（例如只有 `ResolutionSelector` 的比例 + 百万像素），
  这些字段就会是空的，相关线索会留在 `extra_params` 里 —— 不是丢数据，而是图里确实没有。
- **`capture_runs` 只增不减**：每次运行一条记录，目前没有自动清理，介意的话定期 `DELETE FROM capture_runs WHERE created_at < ...`。
- **agent / 脚本生图只有 API 格式工作流**：这类运行不会经过 ComfyUI 前端，界面格式的工作流谁都没有（PNG 里也没有）。
  库里会存下 API 节点图作为兜底，参数、连接、模型都在，但节点位置是 ComfyUI 自己布局的，不是当初画布上的样子。
- **Web 端**：`file_picker` 在 Web 上拿不到本地路径，上传功能在 Web 不可用；本项目定位是桌面 / 移动端。
- **自动启动本地服务仅 Windows 桌面端**：其他平台会明确提示，不会静默失败。
- **单用户、无鉴权**：后端默认只监听本机，请勿直接暴露到公网。
- **Windows 高 DPI**：窗口按系统缩放渲染（本机 150%），因此逻辑尺寸 = 1440/1.5 = 960×613，界面会走 NavigationRail 布局。

---

## 14. 后续可以加的

- 提示词相似度检索 / 向量搜索
- 批量导出成 JSON / CSV 备份
- 视频首帧抽取（引入 ffmpeg 后可做真正的视频缩略图）
- `capture_runs` 的自动清理 / 归档（现在只增不减，数据量大了需要定期删）
- 从 mp4 元数据里还原 VHS_VideoCombine 的工作流（现在视频只入库、不自动关联提示词）
- 远端 ComfyUI（跨机捕获）的鉴权与限流

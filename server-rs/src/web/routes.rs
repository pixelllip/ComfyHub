//! P4 灰度端点：业务读/写（`health` / `tags` / `prompts` / `media` / `stats` / 捕获配置）、
//! 同步 4 条、以及 `/api/ai/*` 的**只读前两批**（`providers` / 模型目录）。
//!
//! 各批次清单见 `src/main.rs` 的文件头表（那里是**诚实清单**：没搬的一律 501）。
//!
//! # 为什么从这三个开始
//!
//! 总纲 §5.1 的灰度顺序是"先非 AI 端点（tags / prompts）"。理由是它们**只读、无副作用、
//! 无上游调用** —— 灰度期间"Kotlin 与 Rust 同时在跑"，读端点错了最多显示不对，
//! 不会造成数据损坏；而写端点（`POST /api/prompts` 等）要等读端点稳定、
//! 且 `/api/sync/push` 那样的幂等语义也铺好之后再搬。
//!
//! # 一条硬纪律：**形状一个字都不改**
//!
//! Flutter 一行不改是 P4 的验收前提，所以这里的路径、查询参数、JSON 字段名都照抄
//! Kotlin 侧（`PromptRoutes.kt` / `TagRoutes.kt` / `Repo` 的 `search`）：
//!
//! | 端点 | 查询参数 |
//! | --- | --- |
//! | `GET /api/tags` | `q` / `category` / `sort=popular\|name` / `limit` |
//! | `GET /api/tags/categories` | — |
//! | `GET /api/prompts` | `q` / `kind` / `favorite` / `sort=newest\|oldest\|title` / `page` / `size` |
//! | `GET /api/prompts/{id}` | — |

use axum::{
    extract::{ConnectInfo, FromRequestParts, Path, Query, State},
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use crate::infra::db;
use crate::web::models::{prompt_from_row, tag_from_row, Paged, PromptDto, TagDto};

/// 共享状态：一条被 `Mutex` 护住的连接。
///
/// ⚠ 这是**灰度期的权宜之计**，不是目标形态：总纲 §5.1 要求"写走 `tokio-rusqlite` 的专属线程
/// （写天然串行化，彻底不会 `SQLITE_BUSY`），读走一小池只读连接"。
/// 现在只有读端点，先用一条连接把形状与形状用例铺出来；写端点落地时再换。
#[derive(Clone)]
pub struct AppState {
    pub conn: Arc<Mutex<Connection>>,
    pub db_path: String,
    /// 产物原件目录（`<storage>/media`）——文件流端点要它。
    media_dir: Arc<std::path::PathBuf>,
    /// Run 事件总线（P4 第 27 步）：`seq` 分配 + 写穿到 `ai_run_events`。
    ///
    /// **它不是事件的真源**（库才是）：SSE 从库里按 `?after=` 拉，
    /// 总线只保证"号不重、号不退"与"攒批落库"。
    pub bus: Arc<crate::ai::events::RunEventBus>,
    /// 正在跑的 Run 与它们的取消旗标（P4 第 30 步）。
    pub executor: Arc<crate::ai::executor::Executor>,
    /// MCP 桥（P4 第 45 步）：官方 comfy-mcp 那 39 个工具的注册表。
    ///
    /// **懒启动**：只有"问工具清单"或"调 MCP 工具"才会真的去起 Python 子进程 ——
    /// 于是集成测试构造的 AppState 永远不会去拉一个外部进程。
    pub mcp: Arc<crate::tools::mcp::McpRegistry>,
    /// 后台捕获轮询器（P4 第 47 步 = O5/O6）：`lastPollAt` / `lastError` 的真源。
    pub capture_poller: Arc<crate::comfy::capture_poller::CapturePoller>,
    /// 最近提交给 ComfyUI 的运行（P4 第 48 步）：`/api/capture/jobs` 的 `submissions[]`。
    pub submissions: Arc<crate::workflow::workflow_submit::Submissions>,
    /// ComfyUI 的**实时进度**（P4 第 72 步）：常驻 WS + 重连，见 [`crate::comfy::comfy_progress`]。
    ///
    /// ⚠ REST 拿不到进度（`/queue` 没有、`/progress` 404、`/history` 只有跑完的），
    /// 所以它是"跑到哪一步了"的唯一来源；`/api/capture/{status,jobs}` 与
    /// `comfy_get_status` 三处读的都是这一份内存表。
    pub comfy_progress: Arc<crate::comfy::comfy_progress::ComfyProgress>,
    /// ComfyUI `/object_info` 的**进程内缓存**。
    ///
    /// `/object_info` 是兆字节级响应，而蓝图、配方、接线体检、盘查全都要它 ——
    /// 以前每次重新拉一遍 + 重新解析一遍，于是"点开蓝图要等一两秒"
    /// （用户 2026-10-18："和 comfy 不是一个量级"）。见 [`crate::workflow::object_info_cache`]。
    pub object_info: Arc<crate::workflow::object_info_cache::ObjectInfoCache>,
    /// 后端**实际**监听的端口（`--port 0` 时是系统挑的那个）。
    ///
    /// `GET /api/net/urls` 用它拼候选地址：不能拿请求里的 Host（那是**客户端**走过的路，
    /// 手机问"这台机器还有哪些地址"时，Host 恰恰是它已经知道的那一条）。
    /// 测试构造的 AppState 是 0（没监听任何端口）→ 那条端点如实回空列表。
    port: Arc<std::sync::atomic::AtomicU16>,
}

impl AppState {
    pub fn new(
        conn: Connection,
        db_path: impl Into<String>,
        media_dir: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: db_path.into(),
            media_dir: Arc::new(media_dir.into()),
            bus: Arc::new(crate::ai::events::RunEventBus::new()),
            executor: Arc::new(crate::ai::executor::Executor::new()),
            mcp: Arc::new(crate::tools::mcp::McpRegistry::new()),
            capture_poller: Arc::new(crate::comfy::capture_poller::CapturePoller::new()),
            submissions: Arc::new(crate::workflow::workflow_submit::Submissions::new()),
            comfy_progress: Arc::new(crate::comfy::comfy_progress::ComfyProgress::new()),
            object_info: Arc::new(crate::workflow::object_info_cache::ObjectInfoCache::new()),
            port: Arc::new(std::sync::atomic::AtomicU16::new(0)),
        }
    }

    /// 告诉 AppState "我们实际听在哪个端口上"（`main.rs` 绑好 listener 之后调一次）。
    pub fn set_port(&self, port: u16) {
        self.port.store(port, std::sync::atomic::Ordering::Relaxed);
    }

    /// 实际监听的端口（0 = 没监听，测试里就是这样）。
    pub fn port(&self) -> u16 {
        self.port.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn media_dir(&self) -> &std::path::Path {
        self.media_dir.as_path()
    }

    /// 项目根（工具权限与 ComfyUI 探测的基准，AGENTS §7）。
    ///
    /// 与 Kotlin `Config.resolveProjectRoot` **同一套算法**：环境变量 →
    /// `<storage>` 的父目录 → 兜底到 storage 自己。发布包/源码树两种布局都认
    /// （`<根>\.db\comfyhub.db` + `<根>\storage\media`）。
    pub fn project_root(&self) -> std::path::PathBuf {
        if let Ok(raw) = std::env::var("COMFYHUB_PROJECT_ROOT") {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return std::path::PathBuf::from(trimmed);
            }
        }
        let media = self.media_dir();
        let storage = media.parent().unwrap_or(media);
        let is_storage = storage
            .file_name()
            .map(|n| n.to_string_lossy().eq_ignore_ascii_case("storage"))
            .unwrap_or(false);
        if is_storage {
            storage.parent().unwrap_or(storage).to_path_buf()
        } else {
            storage.to_path_buf()
        }
    }

    pub fn with<F, T>(&self, f: F) -> Result<T, ApiError>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<T>,
    {
        let guard = self
            .conn
            .lock()
            .map_err(|_| ApiError::internal("连接锁已中毒"))?;
        f(&guard).map_err(ApiError::from)
    }

    /// 写路径：**一律过 `BEGIN IMMEDIATE`**（红线 R2）。
    ///
    /// 事务纪律只有 `db::tx_immediate` 一处实现 —— 端点里别自己拼 `BEGIN`/`COMMIT`。
    pub fn write<F, T>(&self, f: F) -> Result<T, ApiError>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, crate::library::repo::WriteError>,
    {
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| ApiError::internal("连接锁已中毒"))?;
        db::tx_immediate(&mut guard, f).map_err(ApiError::from)
    }
}

/// 路径里的数字 id（`/api/prompts/{id}`、`/api/media/{id}` …）。
///
/// # 为什么不用 `Path<i64>`
///
/// 变量不是数字时 axum 自己回的是 **400 + 纯文本**
/// （`Invalid URL: Cannot parse "abc" to a i64`）—— Dart 的 `jsonDecode` 直接失败，
/// 界面只显示 "HTTP 400"。Kotlin 那边是 `call.requireId() ?: respondBadRequest("非法的 id")`
/// → `{"error":"非法的 id"}`（实测）。
///
/// 这个包装把"路径参数解析失败"也塞回**同一个信封**（`{error, detail?}`）：
/// **Rust 侧任何一个 4xx 都是 JSON**，客户端永远有 `error` 可以给人看。
/// 用法：`Id(id): Id`（元组模式直接解出 `i64`，处理函数里该怎么写还怎么写）。
pub struct Id(pub i64);

impl<S> FromRequestParts<S> for Id
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let raw = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::bad_request("非法的 id"))?;
        raw.0
            .trim()
            .parse::<i64>()
            .map(Id)
            .map_err(|_| ApiError::bad_request("非法的 id"))
    }
}

/// 路径里的**两个**数字 id（`/api/prompts/{id}/tags/{tagId}`）。
///
/// 与 [`Id`] 同理：`Path<(i64, i64)>` 解析失败时是纯文本 400，这里同样塞回信封。
pub struct IdPair(pub i64, pub i64);

impl<S> FromRequestParts<S> for IdPair
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let raw = Path::<(String, String)>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::bad_request("非法的 id"))?;
        let parse = |s: &str| {
            s.trim()
                .parse::<i64>()
                .map_err(|_| ApiError::bad_request("非法的 id"))
        };
        Ok(IdPair(parse(&raw.0 .0)?, parse(&raw.0 .1)?))
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        // 「我现在是不是被关在工作区里了」—— 界面/doctor/工具文案共用的那一份事实（见 confinement.rs）
        .route("/api/system/confinement", get(system_confinement))
        // 本地 LoRA 训练引擎（可选能力）：探测 / 起停 / 状态。
        // 数据面在 anima_daemon 的 /studio/*（经 MCP 工具暴露给模型），这里只给
        // 界面一个“装没装 / 起没起 / WebUI 在哪个端口”的入口。
        .route("/api/train/status", get(train_status))
        .route("/api/train/start", axum::routing::post(train_start))
        .route("/api/train/stop", axum::routing::post(train_stop))
        .route("/api/tags", get(list_tags))
        .route("/api/tags/categories", get(list_tag_categories))
        .route("/api/prompts", get(list_prompts))
        .route("/api/prompts/{id}", get(get_prompt))
        .route("/api/stats", get(get_stats))
        // 产物（P4 第 3 步）：列表 / 详情 / **原件文件流**（Range + ETag）
        .route("/api/media", get(list_media))
        // 集合清单（2026-10-18 手动集合）：给"移到集合…"的选择器用。
        // ⚠ 它和 `/api/media/{id}` 是**同段数**——静态段优先于参数段（matchit 的规矩），
        // 所以 `collections` 不会被当成 id 吃掉。
        .route("/api/media/collections", get(list_collections))
        // 把产物**原件 + 绑定的工作流 / 提示词信息**一次性导到用户指定的文件夹
        // （2026-10-27）。⚠ 只允许回环调用：它会往任意绝对路径写文件，不能跟着
        // 配对令牌一起开放给局域网设备。
        .route("/api/media/export", axum::routing::post(export_media))
        .route("/api/media/{id}", get(get_media))
        .route("/api/media/{id}/file", get(media_file))
        // 这条产物**在磁盘上的绝对路径**（2026-10-18）：给"复制文件本身"用。
        .route("/api/media/{id}/path", get(media_path))
        // 还没搬的：缩略图要图像解码器、封面要 ffmpeg 抽帧、上传要 multipart —— 一律 501
        .route("/api/media/{id}/thumb", get(media_thumb))
        .route("/api/media/{id}/poster", get(media_poster))
        .route("/api/media/{id}/prompt", get(media_prompt))
        .route("/api/prompts/{id}/media", get(prompt_media))
        // 预设的"关联产物"：按**归一化后的提示词**匹配，所以能把自动捕获那条对上（2026-10-11）
        .route("/api/presets/{id}/media", get(preset_media))
        .route("/api/media/upload", axum::routing::post(media_upload))
        // 捕获配置（本地那一半：读写 app_settings 里的一行 JSON）
        .route(
            "/api/capture/config",
            get(capture_config).put(put_capture_config),
        )
        // AI 端点第一批（P4 第 16 步）：provider 清单 —— **不含任何密钥**（AIH-012）
        .route("/api/ai/providers", get(ai_providers).post(create_provider))
        .route(
            "/api/ai/providers/{id}",
            get(ai_provider)
                .put(update_provider)
                .delete(delete_provider),
        )
        // 模型目录（P4 第 20 步 = Task 2）：能力声明的唯一真源（AIH-011），附件/思考强度都读它
        .route(
            "/api/ai/providers/{id}/models",
            get(ai_provider_models).put(put_provider_models),
        )
        // 会话与消息的**读**侧（P4 第 21 步）：AI 工作台打开第一件事就是这三条
        .route(
            "/api/ai/conversations",
            get(ai_conversations).post(create_conversation),
        )
        .route(
            "/api/ai/conversations/{id}",
            get(ai_conversation)
                .patch(patch_conversation)
                .delete(delete_conversation),
        )
        .route(
            "/api/ai/conversations/{id}/messages",
            get(ai_conversation_messages).post(append_message),
        )
        // Run 的生命周期（P4 第 28 步）：创建 202 → 事件流（SSE）→ 取消（AIH-020/021/022）
        .route(
            "/api/ai/conversations/{id}/runs",
            axum::routing::post(start_run),
        )
        .route("/api/ai/runs/{id}", get(get_run))
        .route("/api/ai/runs/{id}/events", get(run_events))
        .route("/api/ai/runs/{id}/cancel", axum::routing::post(cancel_run))
        // 工具审批（P4 第 31 步，R8）：界面上的「批准 / 拒绝」两个按钮。
        // ⚠ 顺序要紧：执行器**先落 pending 行再发 `tool.requested`**，
        // 所以用户手快也点得到（否则那一次点击落空，只能等超时）。
        .route(
            "/api/ai/tool-calls/{callId}/approve",
            axum::routing::post(approve_tool_call),
        )
        .route(
            "/api/ai/tool-calls/{callId}/deny",
            axum::routing::post(deny_tool_call),
        )
        // run-kit 工件（P4 第 22 步）：手机 harness 的**全部规则类输入**（R12/R13 的第一次重建）
        .route("/api/ai/run-kit", get(ai_run_kit))
        .route("/api/ai/run-kit/status", get(ai_run_kit_status))
        // 跨端待办总览（P4 第 25 步，M1 定的契约）：另一台设备刚连上来时一次补齐所有"在等人"的卡片
        .route("/api/ai/pending", get(ai_pending))
        // 提问（P4 第 32 步，用户建议 ⑥）—— 与审批是**两件事**：
        // 审批回"允不允许"，提问回的是一段**文字**，而且那股文字作为工具结果喂回模型、
        // 同一个 Run 接着往下做（所以这里没有"取消"以外的分支，也不改 Run 的状态）。
        .route("/api/ai/runs/{id}/question", get(run_question))
        .route("/api/ai/questions/{questionId}", get(get_question))
        .route(
            "/api/ai/questions/{questionId}/answer",
            axum::routing::post(answer_question),
        )
        .route(
            "/api/ai/questions/{questionId}/skip",
            axum::routing::post(skip_question),
        )
        // 界面端点第一批（P4 第 33 步）：权限档 + 工具清单 + NSFW 开关。
        // 权限档决定"需审批的工具要不要等人点"，界面切不到它 = 第 31 步的闸门永远拦着。
        .route("/api/ai/tools", get(ai_tools))
        .route(
            "/api/ai/tools/policy",
            get(ai_tool_policy).put(put_tool_policy),
        )
        .route("/api/ai/nsfw", get(ai_nsfw).put(put_nsfw))
        // `GET /api/net/urls`（P4 第 73 步）：这台桌面上有哪些**可达地址** ——
        // 「同步与连接」页的候选列表（回环 / 局域网 / Tailscale）。见 `net_urls.rs`。
        .route("/api/net/urls", get(net_urls))
        // 界面端点第二批（P4 第 34 步）：**凭据的写侧**。三条纪律见 `credentials.rs`：
        // DPAPI 不可用就失败（不落明文）/ 空值 = 不修改（清空只能 DELETE）/ env 来源只读。
        .route(
            "/api/ai/providers/{id}/credentials",
            get(ai_provider_credential)
                .put(put_provider_credential)
                .delete(delete_provider_credential),
        )
        // 联网搜索（Keenable）的可选密钥：出厂免密钥可用，这里只是给想提额的用户一个只写入口
        .route(
            "/api/ai/web-search/credential",
            get(ai_web_search_credential)
                .put(put_web_search_credential)
                .delete(delete_web_search_credential),
        )
        // 界面端点第三批（P4 第 35 步）：**Skills 六条**。磁盘是真源、刻意不缓存 ——
        // "AI 注册完下一次回复就能用、用户删掉立刻消失"是需求（AGENTS §10）。
        // ⚠ `/skills/roots` 与 `/skills/{name}` 由 matchit 的静态优先规则分开，不会互相吃。
        .route("/api/ai/skills", get(ai_skills).post(create_skill))
        .route("/api/ai/skills/roots", get(ai_skill_roots))
        .route(
            "/api/ai/skills/rescan",
            axum::routing::post(ai_skill_rescan),
        )
        .route(
            "/api/ai/skills/{name}",
            get(ai_skill_detail).delete(delete_skill),
        )
        // 界面端点第四批（P4 第 36 步）：**长期记忆五条**。真源是 `<storage>\ai\memory.md`
        // （一行一条、用户能手改），界面上的保存 / 加一条 / 删几条 / 清空都打这里。
        .route(
            "/api/ai/memory",
            get(ai_memory).put(put_memory).delete(clear_memory),
        )
        .route(
            "/api/ai/memory/entries",
            axum::routing::post(add_memory_entry),
        )
        // 单条 / 批量删除用 POST 而不是带 body 的 DELETE：Ktor 对 DELETE 带请求体的处理
        // 在各客户端上并不一致（Kotlin 侧就是这么注释的）—— 形状照抄，别"顺手改成 DELETE"。
        .route(
            "/api/ai/memory/delete",
            axum::routing::post(delete_memory_entries),
        )
        // 界面端点第五批（P4 第 38 步）：**内置模型目录**的状态与同步。
        // 它是"出厂默认值"的唯一来源（冻结副本编译进二进制），用户改过的行**永不覆盖**。
        .route("/api/ai/builtin/status", get(ai_builtin_status))
        .route("/api/ai/builtin/sync", axum::routing::post(ai_builtin_sync))
        // 界面端点第六批（P4 第 39 步）：**能力目录的状态与实测校准开关**。
        // `refresh`（2026-09-27：拉 models.dev → 写 remote.json，见 `capability_refresh.rs`）
        // 与 `probe`（2026-09-27：从最高档往下试极小请求、把网关接受的档位写回 `custom.json`，
        // 见 `capability_probe.rs`）**都是实现**。`probe` 会花钱，所以由
        // `probe-toggle`（**默认关**）拦着：没打开时一个上游请求都不发。
        .route("/api/ai/capabilities/status", get(ai_capabilities_status))
        .route(
            "/api/ai/capabilities/probe-toggle",
            axum::routing::put(put_capabilities_probe_toggle),
        )
        .route(
            "/api/ai/capabilities/refresh",
            axum::routing::post(ai_capabilities_refresh),
        )
        .route(
            "/api/ai/capabilities/probe",
            axum::routing::post(ai_capabilities_probe),
        )
        // 本机 ComfyUI 已保存的工作流文件（只读，用户 bug ⑤）—— **不能少任何一条目录来源**
        .route("/api/capture/workflows", get(capture_workflows))
        // 工作流原文（界面「选择现有工作流 → 看蓝图」按需拉）。**只读**，路径过读白名单。
        .route("/api/capture/workflow", get(capture_workflow_raw))
        // 捕获状态与实时进度（第 42 步）：真的去问一次 ComfyUI 的 /queue
        .route("/api/capture/status", get(capture_status))
        .route("/api/capture/jobs", get(capture_jobs))
        // 立刻同步一次 /history 并入库（第 43 步）：**这条路会写库**（建提示词 + 产物）
        .route("/api/capture/poll", axum::routing::post(capture_poll))
        // 推送式捕获（第 49 步）：ComfyUI 自定义节点 / e2e 脚本把一次已跑完的运行推过来
        .route("/api/ingest/comfyui", axum::routing::post(ingest_comfyui))
        // 把本机 ComfyUI 里已保存的工作流文件批量读进库（界面上的「导入本机工作流」按钮）
        .route(
            "/api/capture/import-workflows",
            axum::routing::post(capture_import_workflows),
        )
        // 把一个目录里已经生成好的产物导入画廊（PNG 内嵌的 prompt 会还原成提示词）
        .route(
            "/api/capture/import",
            axum::routing::post(capture_import_folder),
        )
        // 探测 ComfyUI 位置（只读）+ 用户点「使用这个目录」才写配置
        .route("/api/capture/locate", get(capture_locate))
        .route(
            "/api/capture/locate/apply",
            axum::routing::post(capture_locate_apply),
        )
        // 工作流原文（前端按需拉：详情页的「查看工作流」弹窗）
        .route("/api/prompts/{id}/workflow", get(prompt_workflow))
        .route("/api/media/{id}/workflow", get(media_workflow))
        // API 格式节点图（"当时真正跑的东西"：comfy_submit 与端到端自测都读它，R10）
        .route("/api/prompts/{id}/api-graph", get(prompt_api_graph))
        // 布局 → 提示词：把一份布局规格交给 AI 译者，返回提示词 + 校验结果。
        // 模板兜底在前端（`lib/models/layout_spec.dart`），这条接口不持有那份逻辑。
        .route(
            "/api/layout/translate",
            axum::routing::post(layout_translate),
        )
        // 同步 API（P4 第 10 步）：手机连上来的第一件事（契约 §1）
        .route("/api/sync/manifest", get(sync_manifest))
        .route("/api/sync/changes", get(sync_changes))
        .route("/api/sync/push", axum::routing::post(sync_push))
        .route("/api/sync/bootstrap", get(sync_bootstrap))
        .merge(crate::web::write_routes::write_router())
        // 备份与恢复（2026-11-06）：`/api/system/backups*` + `/api/system/restore`。
        // 恢复只落标记，真正换库在下次启动（见 `infra/backup.rs` 的文件头）。
        .merge(crate::web::backup::router())
        // 存储占用 / 孤儿清理 / 事件归档（2026-11-06）：`/api/system/storage*` + `/api/system/events/archive`。
        .merge(crate::web::storage::router())
        // 角色卡 + 画风卡（2026-10-21 合并到同一个仓储）：界面的卡片页与 AI 的
        // `save_character_card` / `save_style_card` 共用这一份实现。
        .merge(crate::library::cards::router())
        // 工作流配方五条（P4 第 67 步）：列表 / 学一份 / 详情 / 绑定 / 删除。
        // **学习与绑定走的是 AI 工具那两个函数**（`workflow_recipe::learn_and_save` /
        // `bind_and_report`）—— 分头写必然漂移，而漂移的表现是"AI 学出来的配方和界面看到的不一样"。
        .merge(crate::workflow::workflow_recipe::router())
        // 蓝图：给"界面里那段任意 JSON"做体检（改完参数就地再验一次，不落库）。
        // 库里那两条（`/api/{prompts,media}/{id}/blueprint`）在上面的路由表里。
        .merge(crate::workflow::workflow_blueprint::router())
        // AI 附件五条（P4 第 50 步）：原件落在 `<storage>/ai-attachments`，缩略图在 `<storage>/ai-thumbs`。
        .merge(crate::media::attachments::router())
        // Provider 连接测试 / 模型发现（P4 第 53 步）：设置页那两个按钮。
        .merge(crate::provider::provider_probe::router())
        // 配对闸门（P4 第 23 步）：`/api/auth*` + **密钥下行** + 令牌校验中间件。
        // ⚠ 顺序要紧：`.layer()` 只作用于**它之前**加进来的路由，所以这里必须排在
        // 所有 `.route()` / `.merge()` 之后（排在前面就会漏掉一半端点，而且不报错）。
        .merge(crate::mobile::auth::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::mobile::auth::gate,
        ))
        // 未知路径也要给**同样形状**的错误体（默认 fallback 是空 body，界面只会显示 "HTTP 404"）
        .fallback(fallback_route)
        .with_state(state)
}

// ---------------------------------------------------------------------------
//  产物（列表 / 详情 / 文件流）
// ---------------------------------------------------------------------------

async fn list_media(
    State(state): State<AppState>,
    Query(q): Query<crate::media::media::MediaQuery>,
) -> Result<Json<Paged<crate::web::models::MediaDto>>, ApiError> {
    Ok(Json(crate::media::media::search(&state, &q)?))
}

async fn get_media(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<Json<crate::web::models::MediaDto>, ApiError> {
    Ok(Json(crate::media::media::get(&state, id)?))
}

/// `POST /api/media/export` —— 产物导出（2026-10-27）。
///
/// 三条纪律：
/// 1. **只允许回环**：它会往用户给的任意绝对路径写文件，不能和配对令牌一起开放给局域网；
/// 2. 文件复制是**阻塞 IO**，放到 `spawn_blocking` 里，别占着事件循环；
/// 3. 目标目录的校验（存在 / 可写 / 不许在 storage 里）在
///    [`crate::media::export::export`] 里，只有一份实现。
async fn export_media(
    State(state): State<AppState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    Json(req): Json<crate::media::export::ExportRequest>,
) -> Result<Json<crate::media::export::ExportReport>, ApiError> {
    if !crate::mobile::auth::is_loopback(Some(remote.ip())) {
        return Err(ApiError::forbidden(
            "导出产物只能在本机（127.0.0.1）操作 —— 它会往你指定的文件夹写文件",
        ));
    }
    let report = tokio::task::spawn_blocking(move || crate::media::export::export(&state, req))
        .await
        .map_err(|e| ApiError::internal(format!("导出任务失败：{e}")))??;
    Ok(Json(report))
}

async fn media_file(
    State(state): State<AppState>,
    Id(id): Id,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    crate::media::media::media_file(state, id, headers).await
}

/// 顶部那几个数字（`GET /api/stats`）。
async fn get_stats(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::media::media_write::stats(&state)?))
}

async fn media_thumb(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<axum::response::Response, ApiError> {
    crate::media::media::media_thumb(state, id).await
}

/// `GET /api/media/{id}/poster` —— 视频的第一帧封面（P4 第 51 步）。
///
/// 三种结果一条都不能混（Kotlin `MediaRoutes` 的 `/poster` 同）：
/// * 产物不存在 → **404**；
/// * 不是视频（图片本来就有缩略图 / 音频没有画面）→ **204**；
/// * 视频但抽不出帧（脚本失败 / 没有 pwsh）→ **204**（前端退化成"播放器转圈"，**不是破图**）；
/// * 有封面 → 200 + `image/png` + `X-Poster: 1`（命中或现生成都给这个头）。
///
/// 抽帧是**起子进程**（`scripts\video-poster.ps1` 走 Windows 缩略图管线），
/// 所以放到阻塞线程池上，别占着事件循环。
async fn media_poster(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<axum::response::Response, ApiError> {
    let media = crate::media::media::get(&state, id)?;
    if media.kind != "VIDEO" {
        return Ok(axum::response::Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(axum::body::Body::empty())
            .unwrap());
    }
    let poster = crate::media::poster::poster_path(state.media_dir(), id);
    if !poster.is_file() {
        let source = crate::media::media::resolve_media(state.media_dir(), &media.stored_name)
            .ok_or_else(|| ApiError::not_found("文件已丢失"))?;
        let root = state.project_root();
        let dest = poster.clone();
        let ok = tokio::task::spawn_blocking(move || {
            crate::media::poster::write_video_poster(
                &root,
                &source,
                &dest,
                crate::media::poster::POSTER_SIZE,
            )
        })
        .await
        .map_err(|e| ApiError::internal(format!("抽帧任务失败：{e}")))?;
        if !ok {
            // 抽不出来就明确告诉前端"没有封面"（204）：播放器转圈，而不是显示破图
            return Ok(axum::response::Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(axum::body::Body::empty())
                .unwrap());
        }
    }
    let bytes = tokio::fs::read(&poster)
        .await
        .map_err(|e| ApiError::internal(format!("读取封面失败：{e}")))?;
    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "image/png")
        .header(axum::http::header::CONTENT_LENGTH, bytes.len().to_string())
        .header("X-Poster", "1")
        .body(axum::body::Body::from(bytes))
        .unwrap())
}

/// `GET /api/media/{id}/prompt`：204 = 这条产物没关联提示词（Dart 的 `mediaPrompt` 把 204 当 null）。
async fn media_prompt(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<axum::response::Response, ApiError> {
    match crate::media::media::prompt_of_media(&state, id)? {
        None => Ok(axum::response::Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(axum::body::Body::empty())
            .unwrap()),
        Some(prompt) => Ok(Json(prompt).into_response()),
    }
}

/// `GET /api/prompts/{id}/media`：这条提示词下的全部产物。
async fn prompt_media(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<Json<Vec<crate::web::models::MediaDto>>, ApiError> {
    Ok(Json(crate::media::media::list_by_prompt(&state, id)?))
}

/// `GET /api/presets/{id}/media`：**与这条预设同款提示词**的产物（2026-10-11）。
///
/// 与 `/api/prompts/{id}/media`（只认 `prompt_id` 直连）的区别：
/// 这里按**归一化后的提示词**匹配，所以"自动捕获那条"与"人存的预设"能对上 ——
/// 用户要的正是这个（"如果有用这套预设同款提示词的产物，自动关联预设"）。
async fn preset_media(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<Json<Vec<crate::web::models::MediaDto>>, ApiError> {
    // ⚠ **预设不存在 = 200 + `[]`**，不是 404 —— 与 `/api/prompts/{id}/media` 同一条纪律
    // （那条路由也不做存在性检查；详情页拿空列表渲染比弹一个 404 更合适）。
    let ids = state.with(|conn| {
        Ok(match crate::media::collection::media_of_preset(conn, id) {
            Ok(ids) => ids,
            Err(crate::library::repo::WriteError::NotFound) => Vec::new(),
            Err(e) => {
                return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(
                    std::io::Error::other(e.to_string()),
                )))
            }
        })
    })?;
    Ok(Json(crate::media::media::list_by_ids(&state, &ids)?))
}

/// `GET /api/capture/config`
async fn capture_config(
    State(state): State<AppState>,
) -> Result<Json<crate::comfy::capture::CaptureConfig>, ApiError> {
    Ok(Json(crate::comfy::capture::get_config(&state)?))
}

/// `PUT /api/capture/config`（整体覆盖 + 归一化，与 Kotlin 同语义）
async fn put_capture_config(
    State(state): State<AppState>,
    Json(body): Json<crate::comfy::capture::CaptureConfig>,
) -> Result<Json<crate::comfy::capture::CaptureConfig>, ApiError> {
    Ok(Json(crate::comfy::capture::put_config(&state, body)?))
}

/// `GET /api/capture/workflows?q=&limit=` —— 本机 ComfyUI 已保存的工作流文件（**只读**）。
///
/// 用户 bug ⑤：`prompts` 库只收"捕获过的运行"与"手动读进来的文件"，所以首次使用时库里是空的，
/// 而用户机器上早就存着工作流 —— 这个端点把它们摆出来（不写库、不动用户的文件）。
async fn capture_workflows(
    State(state): State<AppState>,
    Query(query): Query<crate::comfy::comfy_dirs::WorkflowQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::comfy::comfy_dirs::workflows_json(
        &state, query,
    )?))
}

/// `GET /api/capture/workflow?path=…` 的查询参数。
#[derive(Debug, serde::Deserialize)]
pub struct WorkflowRawQuery {
    pub path: String,
}

/// `GET /api/capture/workflow?path=…` —— 读一份本机工作流原文（**只读**）。
///
/// 界面「选择现有工作流」列表只给了 `path`；点「看蓝图」时要原文。
/// **准入与 `read_file` 同一条**：过 [`crate::tools::policy::ToolPolicy::resolve_read`]，
/// 不在读白名单里的路径一律拒绝 —— 前端绝不自己拼路径读盘。
async fn capture_workflow_raw(
    State(state): State<AppState>,
    Query(query): Query<WorkflowRawQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    const MAX_BYTES: u64 = 8 * 1024 * 1024;
    let path = query.path.trim();
    if path.is_empty() {
        return Err(ApiError::bad_request("path 不能为空"));
    }
    let policy = crate::tools::policy::ToolPolicy::load(&state)?;
    let resolved = policy
        .resolve_read(path)
        .map_err(|failure| ApiError::ai(failure.code, failure.message))?;
    let size = std::fs::metadata(&resolved)
        .map_err(|e| ApiError::bad_request(format!("读不到 {}：{e}", resolved.display())))?
        .len();
    if size > MAX_BYTES {
        return Err(ApiError::bad_request(format!(
            "工作流文件 {} MB，超过界面预览上限（{} MB）",
            size / 1024 / 1024,
            MAX_BYTES / 1024 / 1024
        )));
    }
    let raw = std::fs::read_to_string(&resolved)
        .map_err(|e| ApiError::bad_request(format!("读不到 {}：{e}", resolved.display())))?;
    Ok(Json(serde_json::json!({
        "path": resolved.display().to_string(),
        "bytes": raw.len(),
        "raw": raw,
    })))
}

/// `GET /api/capture/status` —— 捕获状态（含**真的问一次** ComfyUI 队列）。
///
/// ⚠ 与 `/jobs` 的分工照抄 Kotlin：这一条是"设置页 / 首屏"用的（带统计与最近 20 条记录），
/// `/jobs` 是"右侧栏轮询"用的（只要队列与最近提交）。两条都真的去问一次 `/queue`。
async fn capture_status(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::comfy::capture::status_json(&state).await?))
}

/// `GET /api/capture/jobs` —— 实时进度（AI 工作台右侧栏轮询它）。
async fn capture_jobs(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::comfy::capture::jobs_json(&state).await?))
}

/// `POST /api/capture/poll` —— 立刻同步一次 `/history`（**会写库**：建提示词 + 产物入库）。
///
/// ⚠ 与 `/status`、`/jobs` 的分工：那两条**只读**（只问队列），这一条是"把跑完的东西收进来"。
/// Kotlin 那边它同时是后台轮询与 `comfy_sync_history` 工具的实现。
async fn capture_poll(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let config = crate::comfy::capture::capture_config(&state)?;
    Ok(Json(
        crate::comfy::capture_ingest::poll_once(&state, &config).await,
    ))
}

/// `POST /api/ingest/comfyui`（P4 第 49 步）：外部推一条已跑完的运行过来。
async fn ingest_comfyui(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(
        crate::comfy::capture_import::ingest_json(&state, &body).await?,
    ))
}

/// `POST /api/layout/translate`：布局规格 + 意图 → 提示词（含五条校验的结果）。
async fn layout_translate(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(
        crate::ai::layout_translate::translate(&state, &body).await?,
    ))
}

/// `POST /api/capture/import-workflows`（P4 第 49 步）：批量把本机工作流文件读进库。
async fn capture_import_workflows(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(
        crate::comfy::capture_import::import_workflows(&state, &body).await?,
    ))
}

/// `POST /api/capture/import`（P4 第 52 步）：导入一个目录里已生成好的产物。
async fn capture_import_folder(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(
        crate::comfy::capture_import::import_folder_json(&state, &body).await?,
    ))
}

/// `GET /api/ai/providers` —— provider 清单（不含密钥，AIH-012）。
async fn ai_providers(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::ai::ai::AiProviderDto>>, ApiError> {
    Ok(Json(crate::ai::ai::list_providers(&state)?))
}

/// `GET /api/ai/providers/{id}` —— 单个 provider（不存在 → 400 CONFIG_ERROR + detail）。
async fn ai_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::ai::ai::AiProviderDto>, ApiError> {
    let row = crate::ai::ai::require_provider_row(&state, &id)?;
    let stored =
        crate::provider::credentials::read_store(&crate::ai::ai::credential_store_path(&state));
    let refs: Vec<String> = stored.keys().cloned().collect();
    Ok(Json(row.into_dto(&refs)))
}

/// `POST /api/ai/providers` —— 新建（**201 Created**，与 Kotlin 同）。
///
/// 判定顺序照抄 `AiRoutes`：**ID 形态 → 重名 → 协议 → 信任档 → URL/凭据引用**。
/// ⚠ `endpointTrust` 留空时按 baseURL 的主机**推断**（`EndpointTrust.infer`）——
/// 少了这一步，界面不选信任档就会撞上"端点信任级别 public 不允许回环"。
async fn create_provider(
    State(state): State<AppState>,
    Json(body): Json<crate::ai::ai::AiProviderUpsert>,
) -> Result<(StatusCode, Json<crate::ai::ai::AiProviderDto>), ApiError> {
    let id = crate::ai::ai::require_provider_id(body.id.as_deref())?;
    if crate::ai::ai::get_provider_row(&state, &id)?.is_some() {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            format!("Provider ID 已存在: {id}（ID 创建后不可修改）"),
        ));
    }
    let api = parse_api(body.api.as_deref())?;
    let trust =
        crate::web::endpoint_guard::parse_trust(body.endpoint_trust.as_deref().unwrap_or(""))
            .map(str::to_string)
            .unwrap_or_else(|| {
                crate::web::endpoint_guard::infer_trust(&crate::ai::ai::host_of(
                    body.base_url.as_deref().unwrap_or(""),
                ))
                .to_string()
            });
    let dto = crate::ai::ai::AiProviderDto {
        id,
        display_name: crate::ai::ai::require_display_name(body.display_name.as_deref())?,
        api: api.to_string(),
        base_url: crate::ai::ai::normalize_base_url(body.base_url.as_deref(), &trust)?,
        credential_ref: crate::ai::ai::require_credential_ref(body.credential_ref.as_deref())?,
        endpoint_trust: trust,
        enabled: body.enabled,
        // 下面四个是**回读之后才算得出来**的（库给 revision，凭据状态现查）
        revision: 0,
        credential: crate::ai::ai::describe_credential(None, |_| None, |_| false),
        created_at: None,
        updated_at: None,
    };
    Ok((
        StatusCode::CREATED,
        Json(crate::ai::ai::insert_provider(&state, &dto)?),
    ))
}

/// `PUT /api/ai/providers/{id}` —— 乐观锁更新（AIH-007）。
///
/// 三条：路径 ID 形态 → body 里的 ID **不许改** → `revision` **必填**。
/// 少了最后一条，两个窗口就能互相覆盖设置（而"覆盖了"这件事现场看不出来）。
async fn update_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<crate::ai::ai::AiProviderUpsert>,
) -> Result<Json<crate::ai::ai::AiProviderDto>, ApiError> {
    let id = crate::ai::ai::require_provider_id(Some(&id))?;
    if let Some(body_id) = body.id.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        if body_id != id {
            return Err(ApiError::ai("CONFIG_ERROR", "Provider ID 创建后不可修改"));
        }
    }
    let api = parse_api(body.api.as_deref())?;
    let trust =
        crate::web::endpoint_guard::parse_trust(body.endpoint_trust.as_deref().unwrap_or(""))
            .map(str::to_string)
            .unwrap_or_else(|| {
                crate::web::endpoint_guard::infer_trust(&crate::ai::ai::host_of(
                    body.base_url.as_deref().unwrap_or(""),
                ))
                .to_string()
            });
    let revision = body
        .revision
        .ok_or_else(|| ApiError::ai("CONFIG_ERROR", "缺少 revision，请刷新后重试"))?;
    let dto = crate::ai::ai::AiProviderDto {
        id,
        display_name: crate::ai::ai::require_display_name(body.display_name.as_deref())?,
        api: api.to_string(),
        base_url: crate::ai::ai::normalize_base_url(body.base_url.as_deref(), &trust)?,
        credential_ref: crate::ai::ai::require_credential_ref(body.credential_ref.as_deref())?,
        endpoint_trust: trust,
        enabled: body.enabled,
        revision,
        credential: crate::ai::ai::describe_credential(None, |_| None, |_| false),
        created_at: None,
        updated_at: None,
    };
    Ok(Json(crate::ai::ai::update_provider(
        &state, &dto, revision,
    )?))
}

/// `DELETE /api/ai/providers/{id}` —— 不存在 → 400 CONFIG_ERROR（**不是 404**）。
async fn delete_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !crate::ai::ai::delete_provider(&state, &id)? {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            format!("Provider 不存在: {id}"),
        ));
    }
    Ok(Json(json!({ "deleted": true, "id": id })))
}

/// 三种协议（`AiApi.parse`）：认不出来要**列出合法值**（人话）。
fn parse_api(raw: Option<&str>) -> Result<&'static str, ApiError> {
    let text = raw.unwrap_or("").trim();
    crate::ai::ai::API_WIRE_VALUES
        .iter()
        .copied()
        .find(|wire| *wire == text)
        .ok_or_else(|| {
            ApiError::ai(
                "CONFIG_ERROR",
                format!(
                    "不支持的协议 {text}；首期仅支持 {}",
                    crate::ai::ai::API_WIRE_VALUES.join(" / ")
                ),
            )
        })
}

/// `PUT /api/ai/providers/{id}/models` —— 全量替换模型目录（AIH-011）。
///
/// **先 provider 存在、再逐条校验、最后一个事务里删旧插新** ——
/// 顺序反了会出现"校验失败却已经把旧目录删了"。
async fn put_provider_models(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<crate::ai::ai::AiModelUpsert>,
) -> Result<Json<Vec<crate::ai::ai::AiModelDto>>, ApiError> {
    crate::ai::ai::require_provider_row(&state, &id)?;
    for model in &body.models {
        crate::ai::ai::validate_model(model)?;
    }
    Ok(Json(crate::ai::ai::replace_models(
        &state,
        &id,
        &body.models,
    )?))
}

/// `GET /api/ai/providers/{id}/models` —— 模型目录（P4 第 20 步 = Task 2）。
///
/// provider 不存在 → **400 `{error:"CONFIG_ERROR", detail:"Provider 不存在: xx"}`**
/// （不是 404：客户端按 `error` 分支、按 `detail` 给人看，形状必须与 Kotlin 的
/// `AiException` 一致 —— Task 1 那个"界面显示机器码"的 bug 就是这么来的）。
async fn ai_provider_models(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
) -> Result<Json<Vec<crate::ai::ai::AiModelDto>>, ApiError> {
    Ok(Json(crate::ai::ai::list_models(&state, &provider_id)?))
}
#[derive(Debug, Deserialize, Default)]
struct ConversationQuery {
    /// Kotlin 只认 `1` 与 `true`（不区分大小写），别的值一律当 false。
    #[serde(rename = "includeArchived")]
    include_archived: Option<String>,
}

/// `GET /api/ai/conversations?includeArchived=1|true`。
async fn ai_conversations(
    State(state): State<AppState>,
    Query(q): Query<ConversationQuery>,
) -> Result<Json<Vec<crate::ai::conversations::AiConversationDto>>, ApiError> {
    let include = q
        .include_archived
        .as_deref()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    Ok(Json(crate::ai::conversations::list(&state, include, None)?))
}

/// `GET /api/ai/conversations/{id}`（不存在 → 400 CONFIG_ERROR + detail）。
async fn ai_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::ai::conversations::AiConversationDto>, ApiError> {
    Ok(Json(crate::ai::conversations::get(&state, &id)?))
}

/// `GET /api/ai/conversations/{id}/messages` —— 消息 + **有序块**（工具卡 / 思考段全靠它）。
async fn ai_conversation_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<crate::ai::conversations::AiMessageDto>>, ApiError> {
    Ok(Json(crate::ai::conversations::list_messages(&state, &id)?))
}

/// `GET /api/ai/run-kit` 的查询参数（手机按它选好的模型取环境头；两个都可选）。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RunKitQuery {
    provider_id: Option<String>,
    model_id: Option<String>,
}

/// `GET /api/ai/run-kit` —— **手机 harness 的全部"规则类"输入**（P4 第 22 步，R13）。
///
/// `?providerId=&modelId=` 缺省时用桌面第一个启用的 Provider 与它的第一个启用模型；
/// 指定了但库里没有 / 没启用 → **400 `CONFIG_ERROR`**（不静默回落 —— 那会让手机拿着
/// 一份"提示词里写的模型和它要用的不一样"的 kit 还不自知）。
async fn ai_run_kit(
    State(state): State<AppState>,
    Query(q): Query<RunKitQuery>,
) -> Result<Json<crate::ai::run_kit::RunKitDto>, ApiError> {
    Ok(Json(crate::ai::run_kit::build(
        &state,
        q.provider_id.as_deref(),
        q.model_id.as_deref(),
    )?))
}

/// `GET /api/ai/run-kit/status` —— `{current, revoked}`（手机跑之前先问一句吊销）。
async fn ai_run_kit_status(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::ai::run_kit::status(&state)?))
}

/// `GET /api/ai/pending` —— **跨端待办总览**（P4 第 25 步，M1 定的契约）。
///
/// 另一台设备（手机 / 第二个窗口 / 刚重启的桌面 App）没经历那条 SSE 事件流，
/// 靠这一个接口把所有"在等人"的卡片一次补齐：AI 的提问 + 需要批准的工具调用。
async fn ai_pending(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::ai::pending::pending(&state)?))
}

/// `GET /api/capture/locate` —— 探测 ComfyUI 装在哪（用户"其他建议"第 3 条）。
///
/// **只读**：探测结果不会自动生效，用户点「使用这个目录」才会写进配置（下一个端点）。
async fn capture_locate(
    State(state): State<AppState>,
) -> Result<Json<crate::comfy::comfy_dirs::ComfyLocation>, ApiError> {
    Ok(Json(crate::comfy::comfy_dirs::locate_json(&state)?))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocateApplyRequest {
    output_dir: Option<String>,
}

/// `POST /api/capture/locate/apply` —— 把探测到的输出目录写进配置。
///
/// 两条准入照抄 Kotlin：目录**不能为空**、**必须真的存在**（都要 400 说清楚），
/// 不然会把一个打不开的路径写进配置，之后"捕获一条也收不到"而用户不知道为什么。
async fn capture_locate_apply(
    State(state): State<AppState>,
    Json(body): Json<LocateApplyRequest>,
) -> Result<Json<crate::comfy::capture::CaptureConfig>, ApiError> {
    let dir = body.output_dir.unwrap_or_default().trim().to_string();
    if dir.is_empty() {
        return Err(ApiError::bad_request("outputDir 不能为空"));
    }
    if !std::path::Path::new(&dir).is_dir() {
        return Err(ApiError::bad_request(format!("目录不存在：{dir}")));
    }
    let mut current = crate::comfy::capture::capture_config(&state)?;
    current.output_dir = Some(dir);
    Ok(Json(crate::comfy::capture::put_config(&state, current)?))
}

/// `GET /api/prompts/{id}/workflow` —— 有就原样吐 `application/json`，没有就 **204**。
async fn prompt_workflow(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<axum::response::Response, ApiError> {
    workflow_response(crate::comfy::capture::prompt_workflow(&state, id)?)
}

/// `GET /api/media/{id}/workflow`
async fn media_workflow(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<axum::response::Response, ApiError> {
    workflow_response(crate::comfy::capture::media_workflow(&state, id)?)
}

/// `GET /api/media/{id}/path`：这条产物**在磁盘上的绝对路径**（2026-10-18）。
///
/// # 为什么必须有这一条
///
/// "复制文件本身"要往剪贴板写 `CF_HDROP`，那需要**磁盘绝对路径**；而 `MediaDto` 里
/// 只有 `/api/media/{id}/file` 这种 **HTTP 地址**。让前端自己拼
/// `<storage>\media\<storedName>` 是**被禁止的**——路径解析只有一份真源
/// （`web/models.rs` 的注释 + AGENTS §7）。所以后端算好交给它。
///
/// 两种 404 要分得清（界面据此说人话）：
/// * 产物不存在；
/// * 产物在库里、但**文件不在盘上**（被手动删了 / 外置盘拔了）。
async fn media_path(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<Json<serde_json::Value>, ApiError> {
    let stored: Option<String> = state.with(|conn| {
        Ok(conn
            .query_row(
                "SELECT stored_name FROM media_assets WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    })?;
    let Some(stored) = stored else {
        return Err(ApiError::not_found(format!("产物 {id} 不存在")));
    };
    // `resolve_media` 顺带挡住目录穿越，并且**只认真的存在且是文件**的路径
    let Some(path) = crate::media::media::resolve_media(state.media_dir(), &stored) else {
        return Err(ApiError::not_found(format!("产物 {id} 的文件不在盘上")));
    };
    Ok(Json(
        serde_json::json!({ "path": path.display().to_string() }),
    ))
}

/// `GET /api/prompts/{id}/api-graph` —— **API 格式**节点图（提交给 ComfyUI 的那一份）。
///
/// 与 `/workflow` 的三分（404 / 204 / 200）形状相同，但取数链条完全不同：
/// 这里是 `source_ref → capture_runs.raw → HistoryEntry`（见 `capture::prompt_api_graph`）。
async fn prompt_api_graph(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<axum::response::Response, ApiError> {
    match crate::comfy::capture::prompt_api_graph(&state, id)? {
        // 没有 API 图 → 204：调用方据此如实说"这条跑不了"，而不是当成"提示词不存在"
        None => Ok(axum::response::Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(axum::body::Body::empty())
            .unwrap()),
        Some(graph) => Ok(axum::response::Response::builder()
            .status(StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(graph.to_string()))
            .unwrap()),
    }
}

/// 工作流原文的响应：**原样文本**（不重新序列化：可能有几百 KB，前端按原样塞进弹窗）
fn workflow_response(text: Option<String>) -> Result<axum::response::Response, ApiError> {
    match text {
        None => Ok(axum::response::Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(axum::body::Body::empty())
            .unwrap()),
        Some(json) => Ok(axum::response::Response::builder()
            .status(StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(json))
            .unwrap()),
    }
}

async fn media_upload(
    State(state): State<AppState>,
    multipart: axum::extract::Multipart,
) -> Result<
    (
        axum::http::StatusCode,
        Json<crate::media::media_upload::UploadResult>,
    ),
    ApiError,
> {
    crate::media::media_upload::upload(state, multipart).await
}

/// `GET /api/sync/manifest` —— 手机连上来的第一件事（🔒 `docs/sync-api-contract.md` §1）。
async fn sync_manifest(
    State(state): State<AppState>,
) -> Result<Json<crate::mobile::sync::SyncManifest>, ApiError> {
    Ok(Json(crate::mobile::sync::manifest(&state)?))
}

/// `GET /api/sync/changes?since=<cursor>&limit=500` —— 增量（契约 §2）。
///
/// ⚠ 游标落在保留窗口之外时**必须**回 `410 Gone`（见 `ApiError::gone`）。
async fn sync_changes(
    State(state): State<AppState>,
    Query(q): Query<SyncChangesQuery>,
) -> Result<Json<crate::mobile::sync::SyncChanges>, ApiError> {
    Ok(Json(crate::mobile::sync::changes(
        &state,
        q.since.unwrap_or(0),
        q.limit.unwrap_or(500),
    )?))
}

#[derive(Debug, Deserialize)]
struct SyncChangesQuery {
    since: Option<i64>,
    limit: Option<i64>,
}

/// `POST /api/sync/push` —— 上行（契约 §4）。幂等 + 服务端赢 LWW。
async fn sync_push(
    State(state): State<AppState>,
    Json(body): Json<crate::mobile::sync::PushRequest>,
) -> Result<Json<crate::mobile::sync::PushResult>, ApiError> {
    Ok(Json(crate::mobile::sync::push(&state, body)?))
}

/// `GET /api/sync/bootstrap?table=<表>&page=N` —— 首次全量（契约 §3）。
///
/// ⚠ `table` 是**实现时补的必填参数**（契约 §3 的留白，理由写在 `sync::bootstrap` 的注释里）。
async fn sync_bootstrap(
    State(state): State<AppState>,
    Query(q): Query<BootstrapQuery>,
) -> Result<Json<crate::mobile::sync::BootstrapPage>, ApiError> {
    let table = q
        .table
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request(format!(
                "缺 table 参数；合法值：{}",
                crate::mobile::sync::SYNCED.join(", ")
            ))
        })?;
    Ok(Json(crate::mobile::sync::bootstrap(
        &state,
        table,
        q.page.unwrap_or(1),
        q.page_size.unwrap_or(200),
    )?))
}

#[derive(Debug, Deserialize)]
struct BootstrapQuery {
    table: Option<String>,
    page: Option<i64>,
    #[serde(rename = "pageSize")]
    page_size: Option<i64>,
}

// ---------------------------------------------------------------------------
//  会话与消息的**写**侧（P4 第 24 步）
// ---------------------------------------------------------------------------
//
// 为什么这一批重要：手机的 M4 上行只推它**自己产生的行**（`/api/sync/push`），
// 而"桌面在手机那条会话上接着聊"必须由桌面自己写 —— 写侧不搬，指向 Rust 后端时
// 能看见历史却**动不了它**（界面会拿着 404/501 一脸茫然）。

/// `POST /api/ai/conversations` → **201 Created**（Kotlin 也是 201）。
async fn create_conversation(
    State(state): State<AppState>,
    Json(body): Json<crate::ai::conversations::ConversationCreate>,
) -> Result<
    (
        StatusCode,
        Json<crate::ai::conversations::AiConversationDto>,
    ),
    ApiError,
> {
    Ok((
        StatusCode::CREATED,
        Json(crate::ai::conversations::create(&state, &body)?),
    ))
}

/// `PATCH /api/ai/conversations/{id}` —— 改标题 / 换模型 / 归档（缺的字段一律不改）。
async fn patch_conversation(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<crate::ai::conversations::ConversationPatch>,
) -> Result<Json<crate::ai::conversations::AiConversationDto>, ApiError> {
    Ok(Json(crate::ai::conversations::patch(&state, &id, &body)?))
}

/// `DELETE /api/ai/conversations/{id}` —— 消息与块靠外键级联（R1 是它的前提）。
async fn delete_conversation(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::ai::conversations::delete(&state, &id)?;
    Ok(Json(serde_json::json!({ "deleted": true, "id": id })))
}

/// `POST /api/ai/conversations/{id}/messages` → **201 Created** + 落库后的那条消息。
async fn append_message(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<crate::ai::conversations::MessageAppend>,
) -> Result<(StatusCode, Json<crate::ai::conversations::AiMessageDto>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(crate::ai::conversations::append_message(
            &state, &id, &body,
        )?),
    ))
}

// ---------------------------------------------------------------------------
//  Run 的生命周期（P4 第 28 步）
// ---------------------------------------------------------------------------

/// `POST /api/ai/conversations/{id}/runs` → **202 Accepted** + `{runId, assistantMessageId, userMessageId?}`。
///
/// 202 的语义是"收下了、真正在后台跑"（AIH-020）。界面拿到这一对消息主键之后，
/// **立刻**按自己生成的 id 插好"用户气泡 + 空助手气泡"，再靠 SSE 里的 `messageId`
/// 找到那条气泡往上写字 —— 所以这三个键少一个都会让界面显示不出来。
async fn start_run(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<crate::ai::run_lifecycle::RunStartRequest>,
) -> Result<(StatusCode, Json<crate::ai::run_lifecycle::RunStartResponse>), ApiError> {
    let response = crate::ai::run_lifecycle::start(&state, &id, &body)?;
    Ok((StatusCode::ACCEPTED, Json(response)))
}

/// `GET /api/ai/runs/{id}` —— 刷新页面 / 另一台设备连上来时问"这次 Run 到哪了"。
async fn get_run(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<crate::ai::runs::AiRunDto>, ApiError> {
    Ok(Json(crate::ai::runs::get(&state, &id)?))
}

/// `GET /api/ai/runs/{id}/events?after=N` —— **SSE 事件流**。
///
/// ⚠ `after` 也认 `Last-Event-ID` 头（浏览器 `EventSource` 自动重连时会带它）：
/// 只看查询参数的话，浏览器那条自动重连路径**永远从 0 重放**，
/// 表现是"重连之后整段回复又长了一遍"。
async fn run_events(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(q): Query<RunEventsQuery>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    // Run 不存在 → 400 + 稳定码（**在建立流之前**判断：SSE 一旦开始就没法改状态码了）
    crate::ai::runs::get(&state, &id)?;
    let after = q
        .after
        .or_else(|| {
            headers
                .get("last-event-id")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<i64>().ok())
        })
        .unwrap_or(0);
    Ok(crate::ai::run_lifecycle::sse(state, id, after))
}

#[derive(Debug, Deserialize, Default)]
struct RunEventsQuery {
    after: Option<i64>,
}

/// `POST /api/ai/runs/{id}/cancel` —— 取消（Run 已经收尾时回 `{cancelled:false, status}`）。
async fn cancel_run(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<crate::ai::run_lifecycle::CancelResult>, ApiError> {
    Ok(Json(crate::ai::run_lifecycle::cancel(&state, &id)?))
}

/// `POST /api/ai/tool-calls/{callId}/approve|deny` 的响应（Kotlin `ToolApprovalResult`）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolApprovalResult {
    call_id: String,
    approved: bool,
    /// **确实改到了一行 pending**。手快先点 / 已经超时收尾 → `false`（那不是错误）
    accepted: bool,
}

async fn approve_tool_call(
    State(state): State<AppState>,
    axum::extract::Path(call_id): axum::extract::Path<String>,
) -> Result<Json<ToolApprovalResult>, ApiError> {
    let accepted = crate::ai::runs::resolve_approval(&state, &call_id, true)?;
    Ok(Json(ToolApprovalResult {
        call_id,
        approved: true,
        accepted,
    }))
}

async fn deny_tool_call(
    State(state): State<AppState>,
    axum::extract::Path(call_id): axum::extract::Path<String>,
) -> Result<Json<ToolApprovalResult>, ApiError> {
    let accepted = crate::ai::runs::resolve_approval(&state, &call_id, false)?;
    Ok(Json(ToolApprovalResult {
        call_id,
        approved: false,
        accepted,
    }))
}

// ---------------------------------------------------------------------------
//  提问（P4 第 32 步，用户建议 ⑥）
// ---------------------------------------------------------------------------

/// `GET /api/ai/runs/{id}/question` 的响应（Kotlin `PendingQuestionResponse`）。
///
/// `pending=false` 时 `question` / `questions` 两个键**都不出现**（`explicitNulls = false`）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RunQuestionResponse {
    pending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    question: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    questions: Option<Vec<serde_json::Value>>,
}

/// 这个 Run 现在还在等用户回答的提问（没有就 `pending=false`）。
///
/// 给"界面重开 / 断线重连"用：SSE 里那条 `user.question` 可能是在界面没挂着的时候发的。
/// `questions` 是**整张卡**（一次 `ask_user_question` 最多 5 题）—— 只回第一条的话，
/// 用户看到的是一张缺了几题的卡。
async fn run_question(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<RunQuestionResponse>, ApiError> {
    let group = crate::ai::questions::pending_group_for(&state, &id)?;
    let first = group.first().cloned();
    Ok(Json(RunQuestionResponse {
        pending: first.is_some(),
        question: first,
        questions: if group.is_empty() { None } else { Some(group) },
    }))
}

/// 单条提问的状态（界面按 questionId 校验"我这张卡还作数吗"）。
///
/// 查的是**库**（不是内存闸门）：后端重启后这里依然回答"问过什么、答了什么、什么结局"，
/// 而不是 404 把卡片抹掉 —— M1 落库的意义就在这里。
async fn get_question(
    State(state): State<AppState>,
    axum::extract::Path(question_id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match crate::ai::questions::find(&state, &question_id)? {
        Some(value) => Ok(Json(value)),
        // 与 Kotlin 同一档错误：`AiException(CONFIG_ERROR, "提问不存在或已过期: id")`
        None => Err(ApiError::ai(
            "CONFIG_ERROR",
            format!("提问不存在或已过期: {question_id}"),
        )),
    }
}

/// `POST /api/ai/questions/{questionId}/answer` 的请求体（Kotlin `QuestionAnswerRequest`）。
#[derive(Debug, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct QuestionAnswerRequest {
    answer: Option<String>,
    option_index: Option<i64>,
    /// 多选题勾了哪几个（2026-09-22 的多选改造）
    option_indexes: Option<Vec<i64>>,
}

/// `POST /api/ai/questions/{questionId}/answer` 的响应（Kotlin `QuestionAnswerResult`）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct QuestionAnswerResult {
    question_id: String,
    accepted: bool,
    answer: String,
}

/// 用户回答了：`optionIndex` 给了就是"点了第几个按钮"，否则用 `answer` 的自由文本。
///
/// ⚠ `accepted=false` 时界面**必须如实告诉用户**"这次回答没被采纳"（这条提问已经
/// 超时 / Run 已停）—— 假装成功的话，用户以为他答了，而模型那边什么都没收到。
async fn answer_question(
    State(state): State<AppState>,
    axum::extract::Path(question_id): axum::extract::Path<String>,
    Json(body): Json<QuestionAnswerRequest>,
) -> Result<Json<QuestionAnswerResult>, ApiError> {
    let Some(row) = crate::ai::questions::find_stored(&state, &question_id)? else {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            format!("提问不存在或已过期: {question_id}"),
        ));
    };
    let index = body
        .option_index
        .filter(|i| *i >= 0 && (*i as usize) < row.options.len());
    let text = match (
        index,
        body.answer
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty()),
    ) {
        // 点了按钮：答案就是那个选项的原文（不信任前端传来的文字 —— 它可能是改过的）
        (Some(i), _) => row.options[i as usize].clone(),
        (None, Some(text)) => text.to_string(),
        (None, None) => {
            let hint = if row.options.is_empty() {
                "这一题没有选项，只能给 answer 文本".to_string()
            } else {
                format!(
                    "要么给 optionIndex（0~{}），要么给 answer 文本",
                    row.options.len() - 1
                )
            };
            return Err(ApiError::ai("CONFIG_ERROR", format!("要给出回答：{hint}")));
        }
    };
    let indexes = body
        .option_indexes
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| *i >= 0 && (*i as usize) < row.options.len())
        .collect::<Vec<_>>();
    let accepted = crate::ai::questions::record_answer(
        &state,
        &question_id,
        &crate::ai::questions::Answer {
            answer: text.clone(),
            option_index: index,
            option_indexes: indexes,
        },
    )?;
    if accepted {
        // 事件让**所有**开着这条会话的界面都切成"已回答"（不只是点的那一个窗口）
        let mut payload = serde_json::Map::new();
        payload.insert("questionId".into(), serde_json::json!(question_id));
        payload.insert("answer".into(), serde_json::json!(text));
        if let Some(i) = index {
            payload.insert("optionIndex".into(), serde_json::json!(i));
        }
        state.bus.emit(
            &state,
            &row.run_id,
            crate::ai::events::event_type::USER_QUESTION_ANSWERED,
            &serde_json::Value::Object(payload),
        )?;
    }
    Ok(Json(QuestionAnswerResult {
        question_id,
        accepted,
        answer: text,
    }))
}

/// 用户跳过这条提问：**当作"没有回答"**放行。
///
/// 与"不回答就永远挂着"相比，模型拿到的是"用户没有回答"—— 它据此基于已有信息继续，
/// 而不是卡死在这里。
async fn skip_question(
    State(state): State<AppState>,
    axum::extract::Path(question_id): axum::extract::Path<String>,
) -> Result<Json<QuestionAnswerResult>, ApiError> {
    let Some(row) = crate::ai::questions::find_stored(&state, &question_id)? else {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            format!("提问不存在或已过期: {question_id}"),
        ));
    };
    // 库里那条按"用户跳过"收尾（状态守卫保证：已经 answered 的不被覆盖）
    crate::ai::questions::finalize_if_pending(&state, &question_id, "skipped")?;
    state.bus.emit(
        &state,
        &row.run_id,
        crate::ai::events::event_type::USER_QUESTION_ANSWERED,
        &serde_json::json!({ "questionId": question_id, "answer": "", "skipped": true }),
    )?;
    Ok(Json(QuestionAnswerResult {
        question_id,
        accepted: true,
        answer: String::new(),
    }))
}

// ---------------------------------------------------------------------------
//  界面端点第一批（P4 第 33 步）：工具 / 权限档 / NSFW
// ---------------------------------------------------------------------------

/// `GET /api/ai/tools` —— 这台后端**能跑**的工具（界面用它画权限表）。
async fn ai_tools(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::ai::ai_settings::list_tools(&state)?))
}

/// `GET /api/ai/tools/policy` —— 生效的策略（展开后的绝对路径 + 两条预算 + 权限档）。
async fn ai_tool_policy(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::ai::ai_settings::policy_json(&state)?))
}

/// `PUT /api/ai/tools/policy` —— 只改传了的字段；非法权限档 400（**不静默回落**）。
async fn put_tool_policy(
    State(state): State<AppState>,
    Json(body): Json<crate::ai::ai_settings::PolicyUpdate>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::ai::ai_settings::save_policy(&state, body)?))
}

/// `GET /api/ai/nsfw` —— 两档开关 + 生效的打码结论 + 提示词版本号。
async fn ai_nsfw(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::ai::ai_settings::nsfw_json(&state)?))
}

/// `PUT /api/ai/nsfw` —— 缺省一律按**关**（漏传一个字段不能让那段提示词被悄悄打开）。
async fn put_nsfw(
    State(state): State<AppState>,
    Json(body): Json<crate::ai::ai_settings::NsfwUpdate>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::ai::ai_settings::save_nsfw(&state, body)?))
}

// ---------------------------------------------------------------------------
//  界面端点第二批（P4 第 34 步）：凭据的写侧
// ---------------------------------------------------------------------------

/// 明文密钥的请求体（`CredentialSetRequest`，**只有一个键** `value`）。
///
/// `#[serde(default)]`：空 body / 缺 `value` 都按**空串**处理 —— 那正是
/// `CredentialSetRequest(val value: String = "")` 的默认值，语义是"不修改"（AIH-012）。
#[derive(Debug, Default, Deserialize)]
struct CredentialSetRequest {
    #[serde(default)]
    value: String,
}

/// `GET /api/ai/providers/{id}/credentials` —— 只有状态，**没有值**（AIH-012）。
async fn ai_provider_credential(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
) -> Result<Json<crate::ai::ai::CredentialStatus>, ApiError> {
    let provider = crate::ai::ai::require_provider_row(&state, &provider_id)?;
    Ok(Json(describe_provider_credential(
        &state,
        provider.credential_ref.as_deref(),
    )))
}

/// `PUT /api/ai/providers/{id}/credentials`。
///
/// ⚠ 顺序要紧（照抄 Kotlin）：先 `requireProvider` → 再"有没有 credentialRef" →
/// 再"是不是 env 来源（只读）" → 最后才看 body 空不空。
/// 把 body 判空提到前面的话，"没配 ref 的 provider 传了空值"会被当成成功的 no-op，
/// 而界面明明该看到一句"该 Provider 未设置凭据引用名"。
async fn put_provider_credential(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    Json(body): Json<CredentialSetRequest>,
) -> Result<Json<crate::ai::ai::CredentialStatus>, ApiError> {
    let provider = crate::ai::ai::require_provider_row(&state, &provider_id)?;
    let reference = provider.credential_ref.ok_or_else(|| {
        ApiError::ai(
            "CONFIG_ERROR",
            "该 Provider 未设置凭据引用名（credentialRef）",
        )
    })?;
    if crate::provider::credentials::env_source(&reference).is_some() {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            format!("凭据 {reference} 由环境变量提供，是只读的；请改用受管凭据引用名"),
        ));
    }
    if body.value.trim().is_empty() {
        // 留空 = 不修改（AIH-012）；清除请用 DELETE
        return Ok(Json(crate::provider::credentials::describe_now(
            &state, &reference,
        )));
    }
    Ok(Json(crate::provider::credentials::set(
        &state,
        &reference,
        &body.value,
    )?))
}

/// `DELETE /api/ai/providers/{id}/credentials`（env 来源与 Kotlin 一样**拒绝**）。
async fn delete_provider_credential(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
) -> Result<Json<crate::ai::ai::CredentialStatus>, ApiError> {
    let provider = crate::ai::ai::require_provider_row(&state, &provider_id)?;
    let reference = provider.credential_ref.ok_or_else(|| {
        ApiError::ai(
            "CONFIG_ERROR",
            "该 Provider 未设置凭据引用名（credentialRef）",
        )
    })?;
    if crate::provider::credentials::env_source(&reference).is_some() {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            format!("凭据 {reference} 由环境变量提供，无法在应用内移除"),
        ));
    }
    Ok(Json(crate::provider::credentials::unset(
        &state, &reference,
    )?))
}

/// `GET /api/ai/web-search/credential` —— 联网搜索那把可选 key 的状态。
async fn ai_web_search_credential(
    State(state): State<AppState>,
) -> Result<Json<crate::ai::ai::CredentialStatus>, ApiError> {
    Ok(Json(crate::provider::credentials::describe_now(
        &state,
        crate::provider::credentials::KEENABLE_REF,
    )))
}

/// `PUT /api/ai/web-search/credential`（报错文案与 provider 那条**刻意不同**：
/// 这里会告诉用户具体的环境变量名 `COMFYHUB_CRED_keenable`，因为那是唯一的解除办法）。
async fn put_web_search_credential(
    State(state): State<AppState>,
    Json(body): Json<CredentialSetRequest>,
) -> Result<Json<crate::ai::ai::CredentialStatus>, ApiError> {
    let reference = crate::provider::credentials::KEENABLE_REF;
    if crate::provider::credentials::env_source(reference).is_some() {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            format!(
                "密钥 {reference} 由环境变量（COMFYHUB_CRED_{reference}）提供，是只读的；\
                 请先移除环境变量再在应用内填写"
            ),
        ));
    }
    if body.value.trim().is_empty() {
        return Ok(Json(crate::provider::credentials::describe_now(
            &state, reference,
        )));
    }
    Ok(Json(crate::provider::credentials::set(
        &state,
        reference,
        &body.value,
    )?))
}

/// `DELETE /api/ai/web-search/credential`。
async fn delete_web_search_credential(
    State(state): State<AppState>,
) -> Result<Json<crate::ai::ai::CredentialStatus>, ApiError> {
    let reference = crate::provider::credentials::KEENABLE_REF;
    if crate::provider::credentials::env_source(reference).is_some() {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            format!("密钥 {reference} 由环境变量提供，无法在应用内移除"),
        ));
    }
    Ok(Json(crate::provider::credentials::unset(
        &state, reference,
    )?))
}

/// provider 的凭据状态：`credentialRef` 为空时是 `(false, "none", false)`（Kotlin 的 `describe(null)`）。
fn describe_provider_credential(
    state: &AppState,
    reference: Option<&str>,
) -> crate::ai::ai::CredentialStatus {
    match reference {
        Some(reference) => crate::provider::credentials::describe_now(state, reference),
        None => crate::ai::ai::describe_credential(None, |_| None, |_| false),
    }
}

// ---------------------------------------------------------------------------
//  界面端点第三批（P4 第 35 步）：Skills
// ---------------------------------------------------------------------------

/// skill 详情：元数据 + 正文（正文只在打开详情时读，列表不读）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillDetailDto {
    skill: crate::ai::skills::SkillDto,
    content: String,
}

/// 注册 / 覆盖一个 skill（`SkillUpsertRequest`）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillUpsertRequest {
    name: String,
    description: String,
    /// 可选（Kotlin 是 `String? = null`）
    #[serde(default)]
    when_to_use: Option<String>,
    content: String,
}

/// 工具层失败 → AI 错误（`AiRoutes.toolGuard` 的对应物：**码要原样传出去**）。
fn skill_failed(failure: crate::ai::skills::SkillFailure) -> ApiError {
    failure.into_api_error()
}

/// `GET /api/ai/skills` —— 全量清单（**含非法项**：带 `validationError` 的照样列出来，AIH-038）。
async fn ai_skills(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::ai::skills::SkillDto>>, ApiError> {
    Ok(Json(crate::ai::skills::list(&state)))
}

/// `GET /api/ai/skills/{name}` —— 元数据 + 正文（去 frontmatter）。
///
/// 三档"没有"分清：没有这个名字 → **400 `CONFIG_ERROR` + "没有名为 X 的 skill"**；
/// 有但不合法 → `SKILL_INVALID`；有合法但正文文件找不到 → `SKILL_NOT_FOUND`。
async fn ai_skill_detail(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<SkillDetailDto>, ApiError> {
    match crate::ai::skills::read_detail(&state, &name).map_err(skill_failed)? {
        Some((skill, content)) => Ok(Json(SkillDetailDto { skill, content })),
        None => Err(ApiError::ai(
            "CONFIG_ERROR",
            format!("没有名为 {name} 的 skill"),
        )),
    }
}

/// `POST /api/ai/skills` —— 注册 / 覆盖（**201 Created**，与 Kotlin 同）。
async fn create_skill(
    State(state): State<AppState>,
    Json(body): Json<SkillUpsertRequest>,
) -> Result<(StatusCode, Json<crate::ai::skills::SkillDto>), ApiError> {
    let dto = crate::ai::skills::save(
        &state,
        &body.name,
        &body.description,
        body.when_to_use.as_deref(),
        &body.content,
    )
    .map_err(skill_failed)?;
    Ok((StatusCode::CREATED, Json(dto)))
}

/// `DELETE /api/ai/skills/{name}` —— 只许删用户来源；没有这个名字回 `{deleted:false}`（**不是 404**）。
async fn delete_skill(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let deleted = crate::ai::skills::delete(&state, &name).map_err(skill_failed)?;
    Ok(Json(json!({ "deleted": deleted, "id": name })))
}

/// `GET /api/ai/skills/roots` —— 投放口位置（界面要显示绝对路径，用户得知道往哪拷）。
async fn ai_skill_roots(
    State(state): State<AppState>,
) -> Result<Json<crate::ai::skills::SkillRootsDto>, ApiError> {
    Ok(Json(crate::ai::skills::roots_dto(&state)))
}

/// `POST /api/ai/skills/rescan` —— 给"拷进来但没写 frontmatter"的补上并登记，再回最新清单。
async fn ai_skill_rescan(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = crate::ai::skills::auto_register(&state);
    let skills = crate::ai::skills::list(&state);
    // 键顺序与 Kotlin `SkillRescanDto` 一致：registered / names / errors / skills
    Ok(Json(json!({
        "registered": result.registered,
        "names": result.names,
        "errors": result.errors,
        "skills": skills,
    })))
}

// ---------------------------------------------------------------------------
//  界面端点第四批（P4 第 36 步）：长期记忆
// ---------------------------------------------------------------------------

/// 记忆目录（`<storage>\ai`）—— **只有这一处拼路径**。
fn memory_dir(state: &AppState) -> std::path::PathBuf {
    let storage = state.media_dir().parent().unwrap_or(state.media_dir());
    storage.join("ai")
}

/// 写入类失败的稳定码 → AI 错误（`AiRoutes.toolGuard` 的对应物，**码原样传出去**）。
fn memory_failed(failure: crate::ai::memory::MemoryFailure) -> ApiError {
    ApiError::ai(failure.code, failure.message)
}

/// 整篇替换的请求体（`MemoryUpdateRequest`，缺省空串 = 清空，与 Kotlin 同）。
#[derive(Debug, Default, Deserialize)]
struct MemoryUpdateRequest {
    #[serde(default)]
    content: String,
}

/// 追加一条的请求体（`MemoryEntryRequest`）。
#[derive(Debug, Default, Deserialize)]
struct MemoryEntryRequest {
    #[serde(default)]
    content: String,
}

/// 按下标删若干条的请求体（`MemoryDeleteRequest`）。
#[derive(Debug, Default, Deserialize)]
struct MemoryDeleteRequest {
    #[serde(default)]
    indices: Vec<i64>,
}

/// `GET /api/ai/memory` —— 整篇（一行一条）+ 条数上限 + 文件路径。
async fn ai_memory(
    State(state): State<AppState>,
) -> Result<Json<crate::ai::memory::MemoryDto>, ApiError> {
    Ok(Json(crate::ai::memory::read_dto(&memory_dir(&state))))
}

/// `PUT /api/ai/memory` —— 整篇替换（界面上的「保存」）。**超限报错，不静默截断**。
async fn put_memory(
    State(state): State<AppState>,
    Json(body): Json<MemoryUpdateRequest>,
) -> Result<Json<crate::ai::memory::MemoryDto>, ApiError> {
    Ok(Json(
        crate::ai::memory::write(&memory_dir(&state), &body.content).map_err(memory_failed)?,
    ))
}

/// `POST /api/ai/memory/entries` —— 追加一条（界面的「+ 添加一条」，**不加日期前缀**）。
async fn add_memory_entry(
    State(state): State<AppState>,
    Json(body): Json<MemoryEntryRequest>,
) -> Result<Json<crate::ai::memory::MemoryDto>, ApiError> {
    Ok(Json(
        crate::ai::memory::append_user(&memory_dir(&state), &body.content)
            .map_err(memory_failed)?,
    ))
}

/// `POST /api/ai/memory/delete` —— 按下标删（下标是 `entries` 的顺序，越界忽略）。
async fn delete_memory_entries(
    State(state): State<AppState>,
    Json(body): Json<MemoryDeleteRequest>,
) -> Result<Json<crate::ai::memory::MemoryDto>, ApiError> {
    Ok(Json(
        crate::ai::memory::delete_entries(&memory_dir(&state), &body.indices)
            .map_err(memory_failed)?,
    ))
}

/// `DELETE /api/ai/memory` —— 清空。
async fn clear_memory(
    State(state): State<AppState>,
) -> Result<Json<crate::ai::memory::MemoryDto>, ApiError> {
    Ok(Json(
        crate::ai::memory::clear(&memory_dir(&state)).map_err(memory_failed)?,
    ))
}

// ---------------------------------------------------------------------------
//  界面端点第五批（P4 第 38 步）：内置模型目录
// ---------------------------------------------------------------------------

/// `POST /api/ai/builtin/sync` 的请求体（`BuiltinSyncRequest`，只有 `mode`）。
#[derive(Debug, Default, Deserialize)]
struct BuiltinSyncRequest {
    #[serde(default)]
    mode: Option<String>,
}

/// `GET /api/ai/builtin/status` —— **只读预览**（界面拿它弹确认框："将新增 N 个模型"）。
async fn ai_builtin_status(
    State(state): State<AppState>,
) -> Result<Json<crate::provider::builtin::BuiltinSyncResult>, ApiError> {
    Ok(Json(crate::provider::builtin::preview(&state)))
}

/// `POST /api/ai/builtin/sync` —— 真正登记（**幂等**，绝不对用户改过的行下手）。
///
/// ⚠ 失败**不抛 5xx**：把原因放进 `error` 字段回 200 —— 与 Kotlin 同
/// （登记失败不该让整个设置页报错，用户要能看到"为什么没同步上"）。
async fn ai_builtin_sync(
    State(state): State<AppState>,
    Json(body): Json<BuiltinSyncRequest>,
) -> Result<Json<crate::provider::builtin::BuiltinSyncResult>, ApiError> {
    let mode = crate::provider::builtin::SeedMode::parse(body.mode.as_deref())?;
    Ok(Json(crate::provider::builtin::sync(&state, mode)))
}

// ---------------------------------------------------------------------------
//  界面端点第六批（P4 第 39 步）：能力目录
// ---------------------------------------------------------------------------

/// `GET /api/ai/capabilities/status` —— 目录新鲜度（三层各自的来源 / 条数 / 过期与否）。
///
/// 状态接口本身**不产生** `lastMessage`，所以那个键不出现（形状的一部分）。
async fn ai_capabilities_status(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::ai::ai_settings::capability_status(
        &state, None,
    )?))
}

/// `PUT /api/ai/capabilities/probe-toggle` —— 允许/禁止**实测校准**（校准会发真实请求、花钱）。
async fn put_capabilities_probe_toggle(
    State(state): State<AppState>,
    Json(body): Json<crate::ai::ai_settings::ProbeToggle>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::ai::ai_settings::save_probe_enabled(&state, body.enabled)?;
    let message = if body.enabled {
        "已允许实测校准"
    } else {
        "已关闭实测校准"
    }
    .to_string();
    Ok(Json(crate::ai::ai_settings::capability_status(
        &state,
        Some(message),
    )?))
}

/// `POST /api/ai/capabilities/refresh` —— **立即从 models.dev 拉一次在线数据集**。
///
/// 行为逐条对照 Kotlin `CapabilityRefresh`（见 [`crate::provider::capability_refresh`] 的文件头）：
/// 失败**不抛错**、旧目录原样保留，结果用 `lastMessage` 如实回给界面（界面就靠它显示"为什么没变"）。
/// `COMFYHUB_NO_DOWNLOAD=1` 时如实说"已关闭在线更新"，**不假装刷新成功**。
async fn ai_capabilities_refresh(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = crate::provider::capability_refresh::refresh(&state, true).await;
    Ok(Json(crate::ai::ai_settings::capability_status(
        &state,
        Some(result.message),
    )?))
}

/// `POST /api/ai/capabilities/probe` —— **实测校准**（2026-09-27 落地）：
/// 对每个推理模型从最高档往下试极小请求，把网关**实际接受**的档位写回 `custom.json`。
///
/// 行为逐条对照 Kotlin `ThinkingProbe` + `AiUpstream.probeThinkingLadder`
/// （见 [`crate::provider::capability_probe`] 的文件头六条纪律）。两条最容易踩的：
///
///  1. **开关默认关**：`probe-toggle` 没打开时**一个上游请求都不发**（连库都不读），
///     只如实回一句"实测校准没开（它会对每个模型发真实请求、产生费用）"；
///  2. **状态码是 200**：这是"一次正常执行、结果是没做"，不是服务器出错 ——
///     回 501/400 会让界面的错误分支先跳出来，用户看不到那句人话。
async fn ai_capabilities_probe(
    State(state): State<AppState>,
    Json(request): Json<crate::provider::capability_probe::ProbeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dir = crate::provider::capability_catalog::dir_of(
        state.media_dir().parent().unwrap_or(state.media_dir()),
    );
    let response = crate::provider::capability_probe::probe(
        &state,
        &dir,
        &request,
        &crate::provider::capability_probe::HttpTransport,
    )
    .await?;
    Ok(Json(response))
}

// ---------------------------------------------------------------------------
//  错误
// ---------------------------------------------------------------------------

/// 统一错误响应（形状**逐字对齐 Kotlin 侧的 `ApiError`**：`{error, detail?}`）。
///
/// # 两种形态（Kotlin `Application.kt` 的 StatusPages 就是这么分的）
///
/// | 来源 | `error` | `detail` | 例子 |
/// | --- | --- | --- | --- |
/// | 普通错误（`respondError` / `respondNotFound` / `respondBadRequest`） | **人话** | 不出现 | `{"error":"非法的 id"}` |
/// | AI 领域错误（`AiException`，AIH-024） | **稳定码** | 人话 | `{"error":"CONFIG_ERROR","detail":"会话不存在: 9"}` |
///
/// # ⚠ 为什么键名是 `error` 而不是 `message`（2026-09-24 对齐）
///
/// Dart 侧 `ApiClient._decode`（`lib/core/api_client.dart:50`）读的是 `error` 当人话、
/// `detail` 当附加说明；`AiApiClient._decode` 反过来**先读 `detail` 再读 `error`**。
/// 早先这里写的是 `{error:"NOT_FOUND", message:"提示词 999 不存在"}` ——
/// 那两个客户端都读不到人话，界面显示的是 `NOT_FOUND` 这个机器码。
///
/// # ⚠ `detail == None` 时**整键省略**（不是 `"detail": null`）
///
/// Kotlin 的 `AppJson` 是 `explicitNulls = false`（`server/src/main/kotlin/com/comfyhub/Json.kt:12`），
/// 实测：`GET /api/prompts/abc` → `{"error":"非法的 id"}`，**没有 `detail` 键**。
/// 见 `tests/shape_lint.rs` —— 那里有一条"镜像 DTO 不许吐 null"的防线。
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    /// 响应体 `error` 键的值（普通错误 = 人话；AI 错误 = 稳定码）。
    pub error: String,
    /// 响应体 `detail` 键的值；`None` = 这个键**根本不出现**。
    pub detail: Option<String>,
}

impl ApiError {
    fn new(status: StatusCode, error: impl Into<String>, detail: Option<String>) -> Self {
        Self {
            status,
            error: error.into(),
            detail,
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message, None)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message, None)
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message, None)
    }

    /// **`410 Gone`** —— 同步游标落在保留窗口之外（契约 §2）。
    ///
    /// 手机收到它就丢弃镜像、改走 `/api/sync/bootstrap`。
    /// ⚠ 这个分支**不是可选项**：缺了它，手机少掉的那几百条**永远补不回来**
    /// （增量接口照着旧游标往后扫，永远扫不到那些被裁掉的洞）。
    ///
    /// `error` 照样是**人话**（信封规则对同步端点一视同仁）；手机判 410 认的是**状态码**。
    pub fn gone(message: impl Into<String>) -> Self {
        Self::new(StatusCode::GONE, message, None)
    }

    /// 还没搬到 Rust 的端点：**明确 501**，不要拿空数组假装成功（那会让界面以为"真的没有"）。
    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_IMPLEMENTED, message, None)
    }

    /// **AI 领域错误**（`AiException` 的对应物，AIH-024）：`error` = 稳定码、`detail` = 人话。
    ///
    /// 状态码照 Kotlin 的 StatusPages 固定 **400**（`UNSUPPORTED_CONTENT` / `UNKNOWN_MODEL` /
    /// `CONFIG_ERROR` … 都是 400），客户端拿 `error` 分支、拿 `detail` 给用户看。
    /// 空的人话回落到"只给码"（Kotlin 那边 `cause.message` 为 null 时 `detail` 就是不出现）。
    pub fn ai(code: impl Into<String>, message: impl Into<String>) -> Self {
        let message = message.into();
        Self::new(
            StatusCode::BAD_REQUEST,
            code,
            (!message.trim().is_empty()).then_some(message),
        )
    }

    /// **只给内部用**的人类可读说明（普通错误 = `error`，AI 错误 = `detail`）。
    ///
    /// 用在"把这个错误当成一句话塞进另一个结构"的地方，例如
    /// `sync.push` 的 `rejected[].message`、上传时单个文件失败的原因。
    /// **它不进错误响应体**（响应体只有 `error` / `detail` 两个键）。
    pub fn human(&self) -> &str {
        self.detail.as_deref().unwrap_or(&self.error)
    }

    /// **`403`：没带令牌**（或这台设备没被授权做这件事）。
    ///
    /// ⚠ 与 `401` 的语义**不能混**（`docs/pairing-design.md` §7）：
    /// `403` = "你还没证明自己是谁"，`401` = "你证明了自己，但钥匙不对/已被吊销"。
    /// 混了以后用户看到的那句话会指向错误的排查方向（该去配对，却跑去重新配对）。
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message, None)
    }

    /// **`401`：带了令牌但不对 / 已被吊销**。
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message, None)
    }

    /// **`429`：配对请求太频繁**（按来源地址限速）。
    pub fn new_rate_limited(message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, message, None)
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        Self::internal(format!("数据库错误：{e}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // 手拼对象而**不用 json!({"detail": self.detail})**：那样 `None` 会变成 `"detail": null`，
        // 与 `explicitNulls = false` 的 Kotlin 响应不是同一个形状（多一个键就是多一块差异）。
        let mut obj = serde_json::Map::with_capacity(2);
        obj.insert("error".into(), serde_json::Value::String(self.error));
        if let Some(detail) = self.detail {
            obj.insert("detail".into(), serde_json::Value::String(detail));
        }
        (self.status, Json(serde_json::Value::Object(obj))).into_response()
    }
}

/// 没匹配到任何路由 → `404 {"error":"接口不存在: <路径>"}`。
///
/// Kotlin 的 StatusPages 对未知路径也给这句话（`Application.kt:589`），
/// 但它**把状态码写成了 200**（`call.respond` 在 status 处理器里会重置状态）——
/// 那是 Kotlin 侧的 bug，这里**只对齐 body、不对齐那个 200**：
/// 未知路径必须仍是 404，否则任何"路径写错"都会被界面当成成功。
async fn fallback_route(uri: axum::http::Uri) -> ApiError {
    ApiError::not_found(format!("接口不存在: {}", uri.path()))
}

// ---------------------------------------------------------------------------
//  /api/health
// ---------------------------------------------------------------------------

/// Kotlin 的 `APP_VERSION`（`Application.kt:56`）—— 一个字都别改，设置页会显示它。
const APP_VERSION: &str = "1.0.0";

/// `GET /api/health`。
///
/// # ⚠ 四个键是**承重的**，不是装饰（2026-09-24 对齐时抓到）
///
/// `lib/core/backend_launcher.dart:271/379/426` 把 **`health['database'] == "ok"`** 当作
/// "后端已就绪"的唯一判据：拿不到这个键，它会一直认为库坏了，去走"修复"那条路
/// （表现是启动时反复重试 —— 而这不是"少显示一个字段"那么轻）。
/// 设置页还读 `version`（连接成功那句）与 `storageDir`（显示 + 「打开目录」按钮）。
///
/// 也就是说 P4 切流那天，`{status, db, impl, schemaVersion, engine}` 这份形状会让
/// **App 认为后端没起来**。现在与 Kotlin 的 `HealthDto` 对齐：`status` / `version` /
/// `database` / `storageDir` / `serverTime`；另外三样是 Rust 侧多给的诊断信息
/// （`doctor`、灰度期分辨"是哪一版后端"用），客户端按 key 取值，多给不影响。
/// `GET /api/net/urls` —— 这台桌面的可达地址（「同步与连接」页的候选列表）。
///
/// 形状：`{port, urls:[{kind,url}]}`；`kind ∈ loopback|lan|tailscale|other`（**只用于显示**，
/// 不参与"连不连得上"的判断 —— 那条判断由客户端打一次真实请求给出）。
///
/// `port == 0`（测试构造的 AppState / 还没绑端口）→ `urls: []`：
/// **宁可什么都不说，也不报一个连不上的地址**（那会让用户以为是自己的网络问题）。
/// 实现与判据见 [`crate::mobile::net_urls`]。
async fn net_urls(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::mobile::net_urls::urls_json(state.port())))
}

async fn health(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let storage_dir = storage_dir_of(&state);
    let (engine, schema) =
        state.with(|conn| Ok((db::engine_info(conn)?, db::schema_version(conn)?)))?;

    // `database` 就是 Kotlin 那句 "SELECT 1" 的结果（ok / error: …），不是常量
    let database = match state.with(|conn| conn.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))) {
        Ok(_) => "ok".to_string(),
        Err(e) => format!("error: {}", e.human()),
    };
    let server_time: String = state.with(|conn| {
        conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
            r.get(0)
        })
    })?;

    Ok(Json(json!({
        "status": if database == "ok" { "ok" } else { "degraded" },
        "version": APP_VERSION,
        "database": database,
        "storageDir": storage_dir,
        "serverTime": server_time,
        // ↓ Rust 侧多给的（不影响任何客户端；灰度期用来认"这是哪一版后端"）
        "impl": "rust",
        "db": state.db_path,
        "schemaVersion": schema,
        "engine": engine,
    })))
}

/// `GET /api/system/confinement` —— **"这个后端能不能写工作区外"**，连判据一起给。
///
/// 存在的理由（2026-09-29 用户 bug）：受限这件事原来只有 `doctor` 与工具文案会说，
/// 界面上一处都看不到，于是模型如实转述成"input 目录权限不够"，用户去改 ACL —— 白忙。
/// 受限是**环境事实**，得让人一眼看见，而且要附上"为什么"（后端映像在工作区内）。
async fn system_confinement(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::infra::confinement::describe(&state)))
}

/// 训练引擎状态：装没装、守护进程 / WebUI 在不在线、项目数、问题清单。
async fn train_status(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::train::status(&state.project_root()).await))
}

/// 起训练引擎（守护进程 + WebUI）。幂等：已经在跑就不重复起。
async fn train_start(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    crate::train::start(&state.project_root())
        .await
        .map(Json)
        .map_err(ApiError::bad_request)
}

/// 停训练引擎。**不 kill 正在跑的训练**（走 `/shutdown {kill_jobs:false}`）。
async fn train_stop(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    crate::train::stop(&state.project_root())
        .await
        .map(Json)
        .map_err(ApiError::bad_request)
}

/// `<storage>`（`media_dir` 的父目录）—— Kotlin `cfg.storageDir` 的对应物。
fn storage_dir_of(state: &AppState) -> String {
    let media = state.media_dir();
    media
        .parent()
        .unwrap_or(media)
        .to_string_lossy()
        .to_string()
}

// ---------------------------------------------------------------------------
//  /api/tags
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct TagQuery {
    q: Option<String>,
    category: Option<String>,
    sort: Option<String>,
    limit: Option<i64>,
}

async fn list_tags(
    State(state): State<AppState>,
    Query(q): Query<TagQuery>,
) -> Result<Json<Vec<TagDto>>, ApiError> {
    let limit = q.limit.unwrap_or(500).clamp(1, 2000);
    let keyword = q.q.as_ref().map(|s| format!("%{}%", s.trim()));
    let tags = state.with(|conn| {
        // 排序跟 Kotlin `TagRepo.list` 一致：popular = 使用次数降序，其次按名字
        let order = match q.sort.as_deref() {
            Some("name") => "name COLLATE NOCASE ASC",
            _ => "use_count DESC, name COLLATE NOCASE ASC",
        };
        let sql = format!(
            "SELECT * FROM tags
              WHERE (?1 IS NULL OR name LIKE ?1 OR normalized LIKE ?1)
                AND (?2 IS NULL OR category = ?2)
              ORDER BY {order} LIMIT ?3"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![keyword, q.category, limit], tag_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })?;
    Ok(Json(tags))
}

async fn list_tag_categories(State(state): State<AppState>) -> Result<Json<Vec<String>>, ApiError> {
    let categories = state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT category FROM tags WHERE category IS NOT NULL AND category <> '' ORDER BY category",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })?;
    Ok(Json(categories))
}

// ---------------------------------------------------------------------------
//  /api/prompts
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
/// `/api/prompts` 的查询参数（**不只是端点在用**：`search_prompts` 工具走同一个实现，
/// 见 [`query_prompts`] —— 两份 SQL 的后果是"界面搜得到、AI 说没有"，而那不会有用例变红）。
///
/// ⚠ **`rename_all = "camelCase"` 不能少**：Dart 侧发的是 `includeCaptured`，
/// 而 serde 默认按字段名 `include_captured` 找 —— 少这一行，参数**静默失效**
/// （表现是"传了 true 却还是被过滤掉"，而且不报错）。这条是 2026-10-11 被用例抓到的。
pub(crate) struct PromptQuery {
    pub(crate) q: Option<String>,
    pub(crate) kind: Option<String>,
    /// 布尔项收成字符串自己判（与 `MediaQuery` 同一条纪律）：
    /// axum 的 `Query` 对 `Option<bool>` **只认 `true`/`false`**，发 `1` 会**整个请求 400**。
    pub(crate) favorite: Option<String>,
    pub(crate) sort: Option<String>,
    pub(crate) page: Option<i64>,
    pub(crate) size: Option<i64>,
    /// **把自动捕获的提示词也算进来**（2026-10-11）。
    ///
    /// 默认 `false`：**预设库只放"人自己存下来的"**（用户原话："从 comfy 输出目录获取的
    /// 产物，其关联的提示词不需要送进预设库 —— 毕竟都叫这个名字了"）。
    /// 自动捕获那些仍然是提示词行（画廊详情要按 `promptId` 反查它），只是不进这个列表。
    ///
    /// ⚠ **AI 侧必须传 `true`**：`search_prompts` / `comfy_find_workflow` 找的是
    /// "库里能跑的东西"，自动捕获那些正是最好用的工作流来源 —— 默认过滤会把它们藏起来，
    /// 表现是"AI 说库里没有，而用户明明在画廊里看得到"（而且不会有用例变红）。
    #[serde(default)]
    pub(crate) include_captured: bool,
    /// 按"**有没有关联产物**"过滤（`1` = 只要有关联的，`0` = 只要没关联的）。
    ///
    /// ⚠ 这个参数 Dart 侧**一直在发**（`ApiClient.listPrompts` 的 `hasMedia`），
    /// 而后端以前**根本没有这个字段** —— serde 默认忽略未知键，于是
    /// 「未关联产物」那个筛选**从来没生效过**（不报错、不红用例，只是点了没反应）。
    /// 2026-10-11 补上，判据走 [`crate::media::collection::prompt_ids_with_media`]。
    ///
    /// 收成字符串自己判，与 `favorite` 同一条纪律（axum 的 `Option<bool>` 只认 `true/false`）。
    #[serde(default)]
    pub(crate) has_media: Option<String>,
}

async fn list_prompts(
    State(state): State<AppState>,
    Query(q): Query<PromptQuery>,
) -> Result<Json<Paged<PromptDto>>, ApiError> {
    Ok(Json(query_prompts(&state, &q)?))
}

/// 提示词列表查询（**唯一实现**）。
///
/// # 关键字语义照抄 Kotlin `PromptRepo.buildWhere`（2026-09-26 对齐）
///
/// 三条判据，缺一条就是"界面搜得到、`search_prompts` 说没有"（或反过来）：
/// ① **按空白拆词，每个词都要命中**（AND，最多 8 个）——
///    `"cyberpunk 少女"` 是两个条件，不是一整串子串；工具描述里写的也是这句；
/// ② 匹配 **5 列**：`title` / `positive_prompt` / `negative_prompt` / `notes` / `checkpoint`
///    （少一列的表现是"按备注 / 底模搜不到"）；
/// ③ `kind` 先 `trim + uppercase`，且 **`ALL`（或空）= 不过滤**
///    （库里存的是大写 `IMAGE` / `VIDEO`，小写原样传下去会一条都搜不到）。
///
/// ⚠ `search_prompts`（工具）与 `/api/prompts`（界面）**共用这一份** —— 各写一份 SQL
/// 的漂移不会有任何用例变红，只会让 AI 与界面看到两个不同的库。
pub(crate) fn query_prompts(
    state: &AppState,
    q: &PromptQuery,
) -> Result<Paged<PromptDto>, ApiError> {
    let page = q.page.unwrap_or(1).max(1);
    let size = q.size.unwrap_or(20).clamp(1, 200);
    let offset = (page - 1) * size;

    let terms: Vec<String> =
        q.q.as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| {
                text.split_whitespace()
                    .take(8)
                    .map(|term| format!("%{term}%"))
                    .collect()
            })
            .unwrap_or_default();

    let mut where_sql = String::from("WHERE 1 = 1");
    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    for term in terms {
        where_sql.push_str(
            " AND (title LIKE ? OR positive_prompt LIKE ? OR negative_prompt LIKE ? \
               OR notes LIKE ? OR checkpoint LIKE ?)",
        );
        for _ in 0..5 {
            params.push(rusqlite::types::Value::Text(term.clone()));
        }
    }
    let kind = q
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("ALL"))
        .map(str::to_uppercase);
    if let Some(kind) = kind {
        where_sql.push_str(" AND kind = ?");
        params.push(rusqlite::types::Value::Text(kind));
    }
    // Kotlin 只在 `favorite == true` 时过滤（传 false 等于不过滤）
    if crate::media::media::bool_param(q.favorite.as_deref()) {
        where_sql.push_str(" AND favorite = 1");
    }
    // **预设库只放人自己存的**（2026-10-11，用户拍板）：自动捕获那些
    // （`source` 为 `ComfyUI` / `ComfyUI-Import` / `ComfyUI-File`）不进列表。
    //
    // 判据用"**不是** `Manual`"而不是"是那三个前缀"：这样以后新增任何自动来源
    // （推送捕获、手机排队提交的回流…）都自动被排除，不用回来改这里。
    // `Manual` 之外还有一个 `source IS NULL` 的老数据 —— 那是手动建的早期行，也要留下。
    if !q.include_captured {
        where_sql.push_str(" AND (source IS NULL OR source = 'Manual')");
    }
    // 「未关联产物」筛选：判据是**同款提示词**有没有产物（不是 `prompt_id` 直连）。
    //
    // ⚠ 只有显式传了 `hasMedia` 才算这组 id（不传就别做这次全表匹配）——
    // 这个计算要遍历一遍提示词与产物，不该让普通列表也跟着付这笔钱。
    let media_filter = q
        .has_media
        .as_deref()
        .filter(|raw| !raw.trim().is_empty())
        .map(|raw| crate::media::media::bool_param(Some(raw)));
    let with_media = match media_filter {
        Some(_) => Some(state.with(|conn| {
            crate::media::collection::prompt_ids_with_media(conn).map_err(|e| {
                rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
                    e.to_string(),
                )))
            })
        })?),
        None => None,
    };
    // 条件直接拼进 SQL（而不是取出来再在内存里筛）—— 因为**分页与 total 都靠这条 SQL**，
    // 内存筛会让 `total` 说 10 条、界面只显示 3 条。
    if let (Some(want), Some(ids)) = (media_filter, with_media.as_ref()) {
        let marks = vec!["?"; ids.len()].join(",");
        if ids.is_empty() {
            // 一条都没有关联产物：`hasMedia=1` 返回空、`hasMedia=0` 返回全部
            where_sql.push_str(if want { " AND 0 = 1" } else { " AND 1 = 1" });
        } else if want {
            where_sql.push_str(&format!(" AND id IN ({marks})"));
            params.extend(ids.iter().map(|id| rusqlite::types::Value::Integer(*id)));
        } else {
            where_sql.push_str(&format!(" AND id NOT IN ({marks})"));
            params.extend(ids.iter().map(|id| rusqlite::types::Value::Integer(*id)));
        }
    }

    state.with(|conn| {
        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM prompts {where_sql}"),
            rusqlite::params_from_iter(params.iter()),
            |row| row.get(0),
        )?;
        let order = match q.sort.as_deref() {
            Some("oldest") => "created_at ASC, id ASC",
            Some("title") => "title COLLATE NOCASE ASC, id ASC",
            _ => "created_at DESC, id DESC",
        };
        let sql = format!("SELECT * FROM prompts {where_sql} ORDER BY {order} LIMIT ? OFFSET ?");
        let mut list_params = params.clone();
        list_params.push(rusqlite::types::Value::Integer(size));
        list_params.push(rusqlite::types::Value::Integer(offset));
        let mut stmt = conn.prepare(&sql)?;
        let mut items = stmt
            .query_map(
                rusqlite::params_from_iter(list_params.iter()),
                prompt_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for item in items.iter_mut() {
            item.tags = tags_of(conn, item.id)?;
            // **产物数按"同款提示词"算，不是 `prompt_id` 直连**（2026-10-11 用户 bug）：
            // 从画廊存下来的预设，产物指向的是**自动捕获那条**提示词行，
            // 直连判定会让卡片显示「未关联」—— 而用户明明是从那张图存的。
            //
            // 已经算过 `with_media` 集合时就白拿（`hasMedia` 那次过滤顺便算的）。
            item.media_count = match with_media.as_ref() {
                Some(ids) => {
                    if ids.binary_search(&item.id).is_ok() {
                        // 集合里是"有关联产物"，但**具体几条**还得现算：
                        // 卡片上那个数字要准（"关联 3 个产物"）。
                        media_count_matching(conn, item.id)?
                    } else {
                        0
                    }
                }
                None => media_count_matching(conn, item.id)?,
            };
        }
        Ok(Paged::new(items, total, page, size))
    })
}

/// 单条提示词（含标签与产物数）—— `GET /api/prompts/{id}` 与 `search_prompts` 共用。
pub(crate) fn get_prompt_dto(state: &AppState, id: i64) -> Result<PromptDto, ApiError> {
    let prompt = state.with(|conn| {
        let mut stmt = conn.prepare("SELECT * FROM prompts WHERE id = ?1")?;
        let mut rows = stmt.query_map(rusqlite::params![id], prompt_from_row)?;
        match rows.next() {
            Some(row) => {
                let mut prompt = row?;
                prompt.tags = tags_of(conn, prompt.id)?;
                prompt.media_count = media_count_matching(conn, prompt.id)?;
                Ok(Some(prompt))
            }
            None => Ok(None),
        }
    })?;
    prompt.ok_or_else(|| ApiError::not_found(format!("提示词 {id} 不存在")))
}

async fn get_prompt(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<Json<PromptDto>, ApiError> {
    Ok(Json(get_prompt_dto(&state, id)?))
}

/// 一个提示词的标签（按名字排序，与 Kotlin 侧一致）。
pub(crate) fn tags_of(conn: &Connection, prompt_id: i64) -> rusqlite::Result<Vec<TagDto>> {
    let mut stmt = conn.prepare(
        "SELECT t.* FROM tags t
           JOIN prompt_tags pt ON pt.tag_id = t.id
          WHERE pt.prompt_id = ?1
          ORDER BY t.name COLLATE NOCASE ASC",
    )?;
    let rows = stmt.query_map(rusqlite::params![prompt_id], tag_from_row)?;
    rows.collect()
}

/// 与这条提示词**同款**的产物数量（判据见 [`crate::media::collection::media_of_preset`]）。
///
/// 2026-10-11 起界面上的"关联产物数"**一律走这一条**：从画廊存下来的预设，产物指向的是
/// **自动捕获那条**提示词行，直连 `media_assets.prompt_id == 预设 id` 会说"没关联"，
/// 用户明明是从那张图存的（那条直连计数的老函数已删 —— 留着必然被误用）。
/// `WriteError` → `rusqlite::Error`。
///
/// 为什么需要它：`state.with(...)` 的闭包**只能回 `rusqlite::Result`**
/// （它是个只读连接包装），而集合那一层的函数一律回 [`crate::library::repo::WriteError`]。
/// 只读路径上没有 Validation/NotFound 的花样，所以原样搬过去即可 ——
/// 真正需要 400/404 的那些判断都在 `state.write(...)` 里，那条路直接映射成 [`ApiError`]。
pub(crate) fn as_sql_error(error: crate::library::repo::WriteError) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(error.to_string())))
}

pub(crate) fn media_count_matching(conn: &Connection, prompt_id: i64) -> rusqlite::Result<i64> {
    crate::media::collection::media_count_of_preset(conn, prompt_id).map_err(as_sql_error)
}

/// `GET /api/media/collections`：集合清单（2026-10-18 手动集合）。
///
/// 给"移到集合…"的选择器用：**手动集合排前面**（用户自己建的在上面更好找），
/// 每条带 `kind` 与 `size` —— 界面据此把"装不下这一批"的目标置灰并说明原因。
///
/// 算法分出来的集合也在列表里（用户可以把图拖进一个自动分组，那一组会**就地转成手动**），
/// 只是**没有名字**（`title` 缺席，界面按"算法分组"显示）。
async fn list_collections(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::web::models::MediaCollectionDto>>, ApiError> {
    let rows = state.with(|conn| crate::media::collection::list(conn).map_err(as_sql_error))?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

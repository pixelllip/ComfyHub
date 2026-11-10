//! **M5：手机排队 → 桌面真正提交**（契约 🔒 `docs/sync-api-contract.md` **§4.1**）。
//!
//! # 它在整条链上的位置
//!
//! ```text
//! 手机 comfy_queue_submit（本地，只写 _outbox，**不许说"已提交"**，R11）
//!   → 上行 POST /api/sync/push，kind = "submit_job"（一条**指令**，不是表行）
//!   → 【本模块】受理 + 落库 + 在桌面上等人点批准（R8：先 open 再发请求）
//!   → 批准后走**现有 comfy_submit 全链路**（审核 → 预检 → 提交 → 等 → captureRun 入库，R10）
//!   → 往发起那条会话追加**一条助手消息 + 一个 tool_result 块**（带 mediaIds）
//!   → 手机靠**普通下行同步**看到（不为 M5 另造回传通道）
//! ```
//!
//! # 为什么它自己一张表（`submit_jobs`）而不是塞进 `ai_tool_calls`
//!
//! 契约 §4.1 要求"复用 `GET /api/ai/pending` + 现有批准端点，**不要另造一条审批通道**"，
//! 并且"**不许为了这个去伪造一个桌面 Run 来骗过闸门**"。这两条一起决定了落点：
//!
//! | 想法 | 为什么不行 |
//! | --- | --- |
//! | 批准行直接写 `ai_tool_calls`（最"像"复用） | `ai_tool_calls.run_id` 有外键 `→ ai_runs(id)`，而 `PRAGMA foreign_keys=ON`（R1，`db::assert_ready` 盯着）。手机那条 Run 是**手机自己跑的**，`payload.runId` 在桌面**可能还没有对应行**（契约没保证 `ai_run` 与 `submit_job` 的先后）；插不进去 = 审批卡根本不存在 = 人永远批不了。 |
//! | 那就补一个 `ai_runs` 行凑外键 | 那是**为通过校验而伪造一个桌面 Run** —— 已明确禁止。 |
//! | 那就把 `list_pending_approvals` 的 `r.status='running'` 松掉、允许孤儿行 | 治标：外键那一关就过不去（上一条），而且会把"Run 已经收尾的残留审批"一起放出来（那正是那条 JOIN 在防的东西）。 |
//! | **本模块：`submit_jobs` 自己记审批，`pending.rs` 把它并进 `approvals[]`，`runs::resolve_approval` 认它** | ✅ **通道没变**：还是 `GET /api/ai/pending` 那一个列表、还是 `POST /api/ai/tool-calls/{id}/approve\|deny` 那两个端点、还是同一个 `resolve_approval` 函数（它内部多认一类 callId）。变的只是**这一行的落点**：它本来就不是"某个桌面 Run 的工具调用"，硬塞进那张表反而是在撒谎。 |
//!
//! `callId` 用 `submit:<queueId>`：与模型的 `call_00_…` 不会撞，而且一眼能看出这是排队提交。
//!
//! # 九条纪律（逐条对应任务书）
//!
//! 1. **形状照 §4.1**：`{kind:"submit_job", refId:<queueId>, op:"upsert", payload:{…}}`；
//!    `sync::apply_one` 里**单独一个分支**，**不进 `table_of_kind`**（它不是表）。
//! 2. **幂等**：`queue_id` 是主键，`INSERT … DO NOTHING`；同一条再推 → 回 `applied`、
//!    **绝不重复提交**（连 worker 都不再起一个）。
//! 3. **默认必须过桌面审批**：受理时写 `approval='pending'`、`state='awaiting_approval'`，
//!    worker 只**轮询库**（跨端可见）。"手机点过提交"**不是**批准（总纲 §7 待定项 2）。
//!    **唯一例外**：这台设备在桌面上被显式打开了「自动提交」（`autoApproveSubmits`）——
//!    此时写 `approval='auto'`，worker 直接往下走。默认关，按设备、可随时撤销，
//!    回执里保留 `approval='auto'`，结果卡要能显示「自动提交」。
//! 4. **批准后一条捷径都不抄**（R10）：直接调 [`crate::workflow::workflow_submit::tool_submit`] ——
//!    同一份实现，审核 / 预检 / 提交 / 等 / `capture.captureRun()` / `mediaIds` 全在里面。
//! 5. **拒绝 / 超时 / 重启**都不提交，而且**照样回一条**（见下 "终态一定回执"）。
//! 6. **结果回手机**：往 `conversationId` 追加一条 `role=assistant` 消息
//!    （一句人话 + `tool_result` 块，`jsonPayload.mediaIds`）。**成功与失败都回**。
//!    会话还不存在 → `reply_state='blocked'` + `reply_error`，**保留队列**，投递循环重试。
//! 7. **排队 ≠ 已提交**（R11）：这里产出的回执**只在桌面侧**（那条助手消息），
//!    手机自己那条工具结果仍然什么都不带 —— 桌面不会替手机说"已提交"。
//! 8. **失败不许静默**：每条终态都 `eprintln!`；payload 缺字段 → 逐条 `rejected` +
//!    **永久性**错误码（手机据此丢掉 outbox，不再重试）。
//! 9. **不许阻塞 push**：`accept` 只做"校验 + 落一行 + 起一个 tokio 任务"，
//!    **立刻**回 `applied`；`run_job` 在后台等审批（分钟级）与等生成（分钟级）。
//!    "受理了但还没跑完"**不会丢**：账在库里（`submit_jobs` 一行 = 一份持久工作单），
//!    重启有 [`resume_in_flight`]，worker 猝死有 [`sweep`] 的租约看门狗，
//!    回执送不出去有 `reply_state` + 投递循环（**回执也有账**）。
//!
//! # "终态一定回执、且只回一次"是怎么保证的
//!
//! 它由三件事合成（少一件都会出现"手机上永远停在排队中"或"回了两条"）：
//!
//! * **终态是单赢家的**：`UPDATE … WHERE state='awaiting_approval'` / `'submitting'`，
//!   只有写到一行的那一方才算数（同时有两个 worker 也不会都往下走）。
//! * **回执有独立的账**（`reply_state` / `reply_message_id`）：终态先落库，
//!   回执再投递；投递失败（比如会话还没同步上来）只是 `blocked`，**账还在**。
//! * **回执消息的 id 是确定的**（`submit-reply-<queueId>`）：投递前先查这个 id 在不在，
//!   在就算送过了 —— 重复投递不会在会话里冒出第二条气泡。
//!
//! ⚠ **一处如实记下的不确定**：进程在 `state='submitting'` 期间死掉时，我们**不知道**
//! 那次提交到底送进 ComfyUI 没有（`POST /prompt` 之后、写终态之前被杀就是这个窗口）。
//! 处置是**不重投**（重投会额外消耗一次生成资源），而是回一条说明"结果不确定、
//! 去画廊/队列核对"的消息（`SUBMIT_INTERRUPTED`）；真跑完了的话，后台捕获轮询器
//! （`capture_poller`，O5/O6）会把产物收进画廊 —— 那是既有纪律里就有的兜底。

use std::time::{Duration, Instant};

use rusqlite::{Connection, OptionalExtension, Transaction};
use serde_json::{json, Map, Value};

use crate::ai::conversations::{AiMessagePartDto, MessageAppend};
use crate::mobile::sync::PushItem;
use crate::web::routes::{ApiError, AppState};

// ---------------------------------------------------------------------------
//  状态与常量
// ---------------------------------------------------------------------------

/// 已受理、**在桌面上等人点批准**（唯一会出现在 `/api/ai/pending` 的状态）。
pub const STATE_AWAITING_APPROVAL: &str = "awaiting_approval";
/// 已经拿到"批准"，正在跑 `comfy_submit` 那条链路（提交 / 等 / 入库）。
pub const STATE_SUBMITTING: &str = "submitting";
/// 走完了整条链路（成功；产物可能为空，见 `NO_MEDIA` 那档）。
pub const STATE_DONE: &str = "done";
/// 提交链路上的失败（审核 / 预检 / 上游 / 入库）。
pub const STATE_FAILED: &str = "failed";
/// 被拒绝：人点了拒绝、审批超时、或后端重启把审批等待打断（与工具闸门同语义）。
pub const STATE_DENIED: &str = "denied";

/// 审批等待的上限 —— 与 [`crate::ai::executor::APPROVAL_TIMEOUT_SECONDS`] **同值**：
/// 在桌面上等待一张卡的时间不应因"本次由手机排队"而变长或变短。
pub const APPROVAL_TIMEOUT_SECONDS: u64 = crate::ai::executor::APPROVAL_TIMEOUT_SECONDS;

/// 审批状态的轮询间隔（毫秒）—— 与工具的审批闸门同一个数（读的也是同一张库）。
const APPROVAL_POLL_MS: u64 = crate::ai::executor::APPROVAL_POLL_MS;

/// 心跳间隔（秒）：后台任务每这么长时间把 `heartbeat_at` 推一次。
const HEARTBEAT_SECONDS: u64 = 10;

/// 租约：`heartbeat_at` 比现在早过这个数 = **那个 worker 已经不在了**（进程被杀 / panic）。
///
/// 必须比 [`HEARTBEAT_SECONDS`] 大一个数量级，否则正常的提交（R10 那条链路一次要跑
/// 到 `waitSeconds`，默认 240 秒）会被看门狗误判成"死了"而提前写成失败。
pub const LEASE_SECONDS: i64 = 120;

/// 后台清扫（看门狗 + 回执投递 + 孤儿 worker 复起）的间隔（秒）。
pub const SWEEP_INTERVAL_SECONDS: u64 = 15;

/// 终态稳定码（回执的 `tool_result.jsonPayload.code`，也是手机侧要认的那几个）。
pub mod code {
    /// 人在桌面上点了拒绝。
    pub const APPROVAL_DENIED: &str = "APPROVAL_DENIED";
    /// 等满了审批窗口没人批（**超时按拒绝处理**，与工具闸门同）。
    pub const APPROVAL_TIMEOUT: &str = "APPROVAL_TIMEOUT";
    /// 后端重启，审批等待被打断（同 `expire_stale_pending_tool_calls` 的语义）。
    pub const APPROVAL_INTERRUPTED: &str = "APPROVAL_INTERRUPTED";
    /// 提交过程中进程没了：**结果不确定**，如实说清、不重投。
    pub const SUBMIT_INTERRUPTED: &str = "SUBMIT_INTERRUPTED";
    /// 跑完了但没有可入库的产物。
    pub const NO_MEDIA: &str = "NO_MEDIA";
    /// 等生成等到超时（**不是失败**：产物跑完会自动入库）。
    pub const TIMEOUT: &str = "TIMEOUT";
    /// ComfyUI 那边报错，这次没有产物。
    pub const COMFY_RUN_ERROR: &str = "COMFY_RUN_ERROR";
    /// 读配置 / 起链路时的本机错误（人话在 `outcome_message` 里）。
    pub const TOOL_ERROR: &str = "TOOL_ERROR";
}

/// 受理时按字符截断的几处上限（手机给的字符串不能无限长）。
const MAX_ID_CHARS: usize = 200;
/// 手机写好的**最终提示词**上限（提示词可以很长；一条 t2i 的正向文本 8k 字符足够，
/// 超出就截断 —— 截断这件事会写进回执，不静默）。
const MAX_PROMPT_CHARS: usize = 8_000;
/// `payload_json` 原样存的截断上限（只是排查用的痕迹，别让一行撑爆库）。
const MAX_PAYLOAD_CHARS: usize = 16_000;

// ---------------------------------------------------------------------------
//  受理（push 的 submit_job 分支）
// ---------------------------------------------------------------------------

/// 受理结果：`Fresh` = 这次真的收下了（起了 worker）；`Duplicate` = 早就受理过。
///
/// 两者对手机都是 `applied`（幂等，契约 §4.1）—— 但**只有 `Fresh` 会起后台任务**，
/// 这就是"同一个 `queueId` 推两次 → 绝不重复提交"的落点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accepted {
    Fresh,
    Duplicate,
}

/// 一条受理过的排队提交（`submit_jobs` 那一行的读形态）。
#[derive(Debug, Clone)]
pub struct Job {
    pub queue_id: String,
    pub call_id: String,
    pub conversation_id: String,
    pub run_id: Option<String>,
    pub prompt_id: Option<i64>,
    pub source_ref: Option<String>,
    pub overrides: Map<String, Value>,
    pub nsfw: bool,
    /// 手机只说了"用哪一族画"时的三件事（见 [`AutoPick`]）；给了 `promptId` 就不用它们
    pub auto: AutoPick,
    pub state: String,
    pub approval: String,
    pub ok: Option<bool>,
    pub outcome_code: Option<String>,
    pub outcome_message: Option<String>,
    pub media_ids: Vec<i64>,
    pub elapsed_ms: i64,
    pub reply_state: String,
    pub reply_error: Option<String>,
}

/// "手机只说族"那一路的参数（`docs/workflow-auto-select-plan.md` §1.2）。
///
/// ⚠ 它**代替不了** `promptId` 的精确性：这是一条**经验**路径（同任务同族的最近成功 /
/// 已验证的配方），选中的工作流会写入回执 —— 选择错误可直接看出。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AutoPick {
    /// 任务类型（t2i / i2i / edit / inpaint / upscale / video / audio）
    pub task: Option<String>,
    /// 模型族（常用称呼：anima / krea2 / moss image …）
    pub family: Option<String>,
    /// 手机侧 AI 写好的最终提示词（桌面负责写进工作流）
    pub positive_prompt: Option<String>,
    /// 可选：明确要的构图宽高（**不给就一个字节都不改**，沿用工作流自己的分辨率）
    pub width: Option<i64>,
    pub height: Option<i64>,
}

impl AutoPick {
    /// 这几样里有没有一样**足以决定"提交哪份工作流"**。
    ///
    /// ⚠ **只有提示词不算**：`positivePrompt` 决定不了用哪份图（族 / 任务才是选择依据），
    /// 拿它当判据会让"只说提示词"变成"随便挑最近那份" —— 那正是"不猜"要挡住的东西。
    fn can_pick(&self) -> bool {
        self.task.is_some() || self.family.is_some()
    }

    fn from_payload(obj: &Map<String, Value>) -> Self {
        let text = |key: &str, limit: usize| -> Option<String> {
            obj.get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| truncate_chars(v, limit))
        };
        Self {
            task: text("task", 32).map(|t| t.to_lowercase()),
            family: text("modelFamily", MAX_ID_CHARS),
            positive_prompt: text("positivePrompt", MAX_PROMPT_CHARS),
            width: arg_i64(obj.get("width")),
            height: arg_i64(obj.get("height")),
        }
    }
}

/// **受理一条 `submit_job`**（`POST /api/sync/push` 里那个分支的唯一入口）。
///
/// 三件事，按顺序，而且**都不阻塞**：
/// 1. **校验 payload**：缺字段一律**永久性** `rejected`（见 [`parse_payload`] 的清单）；
/// 2. **落一行**（`DO NOTHING` 保证幂等）；
/// 3. **起后台任务**（只有真落进去的那一次才起）。
///
/// ⚠ 顺序不能倒：行**必须先落库**，"受理了但还没跑完不会丢"才成立 —— 手机收到
/// `applied` 就会把那行 outbox 删掉（`SyncPushClient` 的纪律），此后账只在桌面这一侧。
pub fn accept(state: &AppState, item: &PushItem) -> Result<Accepted, Reject> {
    let reject = |code: &str, message: String| Reject {
        code: code.to_string(),
        message,
    };

    if let Some(op) = item.op.as_deref() {
        if op != "upsert" {
            return Err(reject(
                "UNSUPPORTED_OP",
                format!("submit_job 只支持 upsert，收到 {op}"),
            ));
        }
    }
    let mut parsed = parse_payload(item)?;
    // `sourceRef`（`file:<sha256>` 那种运行键）在**受理时就解析成 `prompt_id`**：
    // 那时才谈得上"逐条 rejected"，晚了手机那行 outbox 早就因为 `applied` 被删了（静默丢失）。
    //
    // ⚠ "只说族"那一路（`task` / `modelFamily` / `positivePrompt`）**受理时不做选择**：
    //   挑工作流要读 capture_runs 的 API 图（分析任务与模型），那是异步 + 稍贵的一步；
    //   而且挑不到也是**这次的真实结论**（要回一条"需要你确认"，不是拒绝推送）。
    if parsed.prompt_id.is_none() {
        if let Some(source_ref) = parsed.source_ref.clone() {
            match resolve_source_ref(state, &source_ref)? {
                Some(id) => parsed.prompt_id = Some(id),
                None => {
                    return Err(reject(
                        "UNKNOWN_SOURCE_REF",
                        format!(
                            "桌面上没有 source_ref = {source_ref} 的工作流（prompts 由桌面产生，手机不许创建）"
                        ),
                    ))
                }
            }
        }
    }

    // 落库：主键就是幂等键。`DO NOTHING`（**不是** `OR REPLACE`：那是 DELETE+INSERT，
    // 会把上一次那条的审批 / 结果 / 回执账**整行抹掉** —— 那正是"推两次提交两次"的入口）。
    let now = now_iso();
    let (queue_id, call_id) = (parsed.queue_id.clone(), parsed.call_id());
    let requested_by = parsed.requested_by.clone();
    let mut auto_approve = false;
    let mut approval = "pending";
    let inserted = state
        .write(|tx| {
            // 这台设备的「免桌面批准」开关（默认关；见 auth::DeviceRecord）。
            // 必须在**同一个写事务**里读：受理那一刻的放权状态就是最终状态。
            auto_approve = requested_by
                .as_deref()
                .map(|id| crate::mobile::auth::device_auto_approve(tx, id))
                .unwrap_or(false);
            approval = if auto_approve { "auto" } else { "pending" };
            let n = tx.execute(
                "INSERT INTO submit_jobs
                   (queue_id, call_id, conversation_id, run_id, payload_json, prompt_id, source_ref,
                    overrides_json, nsfw, state, approval, reply_state, requested_at, requested_by,
                    heartbeat_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'pending', ?12, ?13, ?14, ?14, ?14)
                 ON CONFLICT(queue_id) DO NOTHING",
                rusqlite::params![
                    queue_id,
                    call_id,
                    parsed.conversation_id,
                    parsed.run_id,
                    parsed.payload_text,
                    parsed.prompt_id,
                    parsed.source_ref,
                    parsed.overrides_text,
                    if parsed.nsfw { 1 } else { 0 },
                    STATE_AWAITING_APPROVAL,
                    approval,
                    parsed.requested_at,
                    parsed.requested_by,
                    now,
                ],
            )?;
            Ok(n)
        })
        .map_err(|e| Reject { code: "DB_ERROR".to_string(), message: format!("落库失败：{}", e.human()) })?;

    if inserted == 0 {
        // 早就受理过（可能还在等批准、也可能已经跑完）—— **回 applied 但什么都不做**。
        eprintln!("[submit-queue] 重复推送：queueId={queue_id} 已受理过，不重复提交");
        return Ok(Accepted::Duplicate);
    }
    eprintln!(
        "[submit-queue] 受理排队提交：queueId={queue_id} conversation={} promptId={:?}{}（{}）",
        parsed.conversation_id,
        parsed.prompt_id,
        if parsed.auto.can_pick() {
            format!(
                " 自动选工作流：task={:?} 族={:?} 提示词{}",
                parsed.auto.task,
                parsed.auto.family,
                if parsed.auto.positive_prompt.is_some() {
                    "已给"
                } else {
                    "未给"
                }
            )
        } else {
            String::new()
        },
        if auto_approve { "自动提交，免桌面批准" } else { "等人批准" }
    );
    start(state, &parsed.queue_id);
    Ok(Accepted::Fresh)
}

/// 受理阶段的**永久性**拒绝（`code + message` → `push` 的 `rejected[]`）。
///
/// 判据只有一条（与 `SyncPushClient.isPermanentReject` 同义）：**重推一次结果会不一样吗**。
/// 形状错的 payload 重推一万次还是错 —— 所以这里**没有**暂时性的码（那种错误是 `DB_ERROR`）。
#[derive(Debug, Clone)]
pub struct Reject {
    pub code: String,
    pub message: String,
}

/// 校验过的 payload（受理时只做**形状**校验，见下）。
struct ParsedPayload {
    queue_id: String,
    conversation_id: String,
    run_id: Option<String>,
    prompt_id: Option<i64>,
    source_ref: Option<String>,
    /// 参数覆盖的 JSON 文本（落库那一列就是它；空对象 → `None`）
    overrides_text: Option<String>,
    nsfw: bool,
    requested_at: Option<String>,
    requested_by: Option<String>,
    payload_text: String,
    /// "只说族"那一路（`promptId` / `sourceRef` 都没给时才用它）
    auto: AutoPick,
}

impl ParsedPayload {
    fn call_id(&self) -> String {
        call_id_for(&self.queue_id)
    }
}

/// 审批闸门的调用号。`submit:` 前缀让它与模型的 `call_00_…` 永不撞车（也一眼看得懂）。
pub fn call_id_for(queue_id: &str) -> String {
    format!("submit:{queue_id}")
}

/// 校验 `payload` 的形状（契约 §4.1）。**只查形状，不查"这次提交能不能成"**。
///
/// 不在受理时一并检查"是否有 API 图 / 审核是否通过"的依据：那些属于**提交链路**自身的判断
/// （R10 的审计 + 预检），提前在这里判一遍等于**两份判断**，迟早漂移；而且真的拒了，
/// 相比显示"手机上说排队失败"，"桌面上批准后如实返回失败原因"更清楚。
/// 形状错不一样：它**永远**不可能成功，重推只会让手机永远卡在队首。
fn parse_payload(item: &PushItem) -> Result<ParsedPayload, Reject> {
    let bad = |code: &str, message: String| Reject {
        code: code.to_string(),
        message,
    };
    let Some(obj) = item.payload.as_object() else {
        return Err(bad(
            "BAD_PAYLOAD",
            "submit_job 的 payload 必须是对象（契约 §4.1）".to_string(),
        ));
    };

    let text_of = |key: &str| -> Option<String> {
        obj.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| v.chars().take(MAX_ID_CHARS).collect::<String>())
    };

    // `queueId`：payload 里给了就以它为准，但必须与 `refId` **一致**
    // （不一致的话"幂等键"就有两个说法，重推会变成两条完全不同的记录）
    let queue_id = text_of("queueId").unwrap_or_else(|| item.ref_id.trim().to_string());
    if queue_id.is_empty() {
        return Err(bad(
            "BAD_PAYLOAD",
            "submit_job 缺 queueId（refId 也是空的）".to_string(),
        ));
    }
    if queue_id != item.ref_id.trim() {
        return Err(bad(
            "REF_MISMATCH",
            format!(
                "refId（{}）与 payload.queueId（{queue_id}）不一致",
                item.ref_id
            ),
        ));
    }

    // `conversationId`：回执追加到哪条会话。**不得缺失** —— 缺失则无法返回结果，
    // 而"手机那条『已排队』永远停在排队中"是最坏的表现（契约 §4.1 最后一条）。
    let Some(conversation_id) = text_of("conversationId") else {
        return Err(bad(
            "MISSING_CONVERSATION",
            "submit_job 缺 conversationId：结果要追加到发起那条会话里，没有它就只能永远排队\
             （线上形状见契约 §4.1；手机侧要把 comfy_queue_submit 的意图**摊平到 payload 顶层**，\
             不能整个塞在一个 `args` 子对象里）"
                .to_string(),
        ));
    };

    let run_id = text_of("runId");
    let source_ref = text_of("sourceRef");
    let prompt_id = arg_i64(obj.get("promptId"));
    let auto = AutoPick::from_payload(obj);
    // 任务类型只认那七档：给了个认不出来的，重推一万次也还是错 —— 现场就拒。
    if let Some(task) = auto.task.as_deref() {
        if crate::workflow::workflow_recipe::Task::parse(task).is_none() {
            return Err(bad(
                "BAD_PAYLOAD",
                format!(
                    "submit_job 的 task「{task}」不支持：只能是 t2i / i2i / edit / inpaint / \
                     upscale / video / audio"
                ),
            ));
        }
    }
    // 宽高：0 / 负数 / 荒唐的大值都当形状错（别让它一路传进 ComfyUI 再炸在那边）
    for (key, value) in [("width", auto.width), ("height", auto.height)] {
        if let Some(v) = value {
            if !(64..=8192).contains(&v) {
                return Err(bad(
                    "BAD_PAYLOAD",
                    format!("submit_job 的 {key}={v} 不合理：要在 64~8192 之间（像素）"),
                ));
            }
        }
    }
    if prompt_id.is_none() && source_ref.is_none() && !auto.can_pick() {
        return Err(bad(
            "MISSING_WORKFLOW",
            "submit_job 不知道该提交哪条工作流：要么给 `promptId` / `sourceRef`（精确指定），\
             要么给 `task` + `modelFamily`（让桌面按\"同任务同族的最近成功 → 已验证的配方\"自己挑）\
             —— 只给一个工作流名字是不行的（手机没有那份库，名字解析不出 id）。"
                .to_string(),
        ));
    }

    let overrides = match obj.get("overrides") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => {
            return Err(bad(
                "BAD_PAYLOAD",
                "submit_job 的 overrides 必须是对象（键 = `节点id.输入名`）".to_string(),
            ))
        }
    };
    let overrides_text = if overrides.is_empty() {
        None
    } else {
        Some(Value::Object(overrides.clone()).to_string())
    };

    Ok(ParsedPayload {
        queue_id,
        conversation_id,
        run_id,
        prompt_id,
        source_ref,
        overrides_text,
        nsfw: obj.get("nsfw").and_then(Value::as_bool).unwrap_or(false),
        requested_at: text_of("requestedAt"),
        requested_by: text_of("requestedBy"),
        payload_text: truncate_chars(&item.payload.to_string(), MAX_PAYLOAD_CHARS),
        auto,
    })
}

/// **把 `sourceRef` 解析成 `prompt_id`**（受理时调；找不到就是**永久性**拒绝）。
///
/// **找不到就是永久性拒绝**：`prompts` 是桌面产生的（手机不许创建它，契约 §4），
/// 所以"桌面没有这条 sourceRef"意味着手机手里那串来源是错的/过期的，重推永远不会变。
fn resolve_source_ref(state: &AppState, source_ref: &str) -> Result<Option<i64>, Reject> {
    state
        .with(|conn| {
            conn.query_row(
                "SELECT id FROM prompts WHERE source_ref = ?1 ORDER BY id LIMIT 1",
                rusqlite::params![source_ref],
                |row| row.get::<_, i64>(0),
            )
            .optional()
        })
        .map_err(|e| Reject {
            code: "DB_ERROR".to_string(),
            message: format!("查工作流失败：{}", e.human()),
        })
}

// ---------------------------------------------------------------------------
//  后台任务：等审批 → 提交 → 落在账上
// ---------------------------------------------------------------------------

/// 一次 worker 跑完之后的结论（给日志与用例看）。
#[derive(Debug, Clone, PartialEq)]
pub enum JobReport {
    /// 行不在了（被清理 / queueId 错）。
    Missing,
    /// 这次调用没有需要它做的事（已经终态 / 别的 worker 正在提交它）。
    NotWaiting,
    /// 走完了整条链路（成功或失败都在账上，回执也会发）。
    Finished {
        state: String,
        ok: bool,
        media_ids: Vec<i64>,
    },
}

/// 起一个后台 worker（**只在 `accept` 里调**；测试直接调 [`run_job`] 自己控制时长）。
pub fn start(state: &AppState, queue_id: &str) {
    let state = state.clone();
    let queue_id = queue_id.to_string();
    tokio::spawn(async move {
        let report = run_job(
            &state,
            &queue_id,
            Duration::from_secs(APPROVAL_TIMEOUT_SECONDS),
        )
        .await;
        if let JobReport::Finished {
            state: terminal,
            ok,
            media_ids,
        } = &report
        {
            eprintln!(
                "[submit-queue] queueId={queue_id} 收尾：state={terminal} ok={ok} mediaIds={:?}",
                media_ids
            );
        }
    });
}

/// 跑完一条排队提交（**可测**：审批窗口由调用方给）。
///
/// 三段：① 等批准（人点了才算；超时按拒绝）→ ② 原子地认领 → ③ 调
/// [`crate::workflow::workflow_submit::tool_submit`] 走完 R10 那条链路，把结论落在账上。
pub async fn run_job(state: &AppState, queue_id: &str, approval_timeout: Duration) -> JobReport {
    // 租约心跳：**整个 worker 生命周期**都在推（既覆盖"等人批准"那几分钟，
    // 也覆盖 tool_submit 里"提交 + 等生成"那段 —— 两段都可能是分钟级）。
    let beat = start_heartbeat(state.clone(), queue_id.to_string());
    let report = run_inner(state, queue_id, approval_timeout).await;
    beat.abort();
    report
}

async fn run_inner(state: &AppState, queue_id: &str, approval_timeout: Duration) -> JobReport {
    let Some(job) = load(state, queue_id) else {
        return JobReport::Missing;
    };
    if job.state != STATE_AWAITING_APPROVAL {
        // 已经终态，或者另一个 worker 已经认领走了 —— 这一次什么都不用做。
        return JobReport::NotWaiting;
    }

    let deadline = Instant::now() + approval_timeout;
    loop {
        // ⚠ **先看库再判时间**：手快先点了批准也不许落空（R8 的老坑）——
        // 闸门那一行早就在库里（`accept` 是**同步**落的），所以这里一定读得到。
        match approval_of(state, queue_id) {
            Ok(Some((state_now, approval))) => {
                if state_now != STATE_AWAITING_APPROVAL {
                    return JobReport::NotWaiting;
                }
                match approval.as_str() {
                    // `auto` = 这台设备开了「免桌面批准」，受理时就已是批准态。
                    "approved" | "auto" => break,
                    "denied" => {
                        let done = finalize(
                            state,
                            queue_id,
                            STATE_AWAITING_APPROVAL,
                            // `elapsed_ms = 0` → 由 `finalize` 按 `created_at` 算真实读数
                            // （审批等了多久，就是这条排队提交"卡"了多久）
                            &Terminal::denied(
                                code::APPROVAL_DENIED,
                                "这次提交被桌面拒绝了，没有提交。",
                                0,
                            ),
                        );
                        if done {
                            deliver_now(state, queue_id);
                        }
                        return JobReport::NotWaiting;
                    }
                    _ => {}
                }
            }
            Ok(None) => return JobReport::NotWaiting,
            Err(error) => {
                eprintln!(
                    "[submit-queue] queueId={queue_id} 读审批状态失败：{}",
                    error.human()
                );
                return JobReport::NotWaiting;
            }
        }
        if Instant::now() >= deadline {
            // 超时 = 拒绝（与工具闸门同语义）。**同时把 `approval` 写成 denied**，
            // 否则 `/api/ai/pending` 上会一直挂着一张点了也没用的卡。
            let done = finalize(
                state,
                queue_id,
                STATE_AWAITING_APPROVAL,
                &Terminal::denied(
                    code::APPROVAL_TIMEOUT,
                    &format!(
                        "等了 {} 秒也没人批准，这次提交没有提交（超时按拒绝处理）。",
                        approval_timeout.as_secs()
                    ),
                    0,
                ),
            );
            if done {
                deliver_now(state, queue_id);
            }
            return JobReport::NotWaiting;
        }
        tokio::time::sleep(Duration::from_millis(APPROVAL_POLL_MS)).await;
    }

    // ★ 认领（**原子**）：只有把 `awaiting_approval + approved` 改成 `submitting` 的那一个
    //   才往下走。两个 worker 同时被起（重复推、看门狗复起）也只会提交一次。
    if !claim(state, queue_id) {
        return JobReport::NotWaiting;
    }

    let terminal = execute(state, &job).await;
    finalize(state, queue_id, STATE_SUBMITTING, &terminal);
    deliver_now(state, queue_id);
    JobReport::Finished {
        state: terminal.state.to_string(),
        ok: terminal.ok,
        media_ids: terminal.media_ids,
    }
}

/// 一条终态的结论（`state` / `ok` / 稳定码 / 人话 / mediaIds）。
#[derive(Debug, Clone)]
struct Terminal {
    state: &'static str,
    ok: bool,
    code: Option<String>,
    message: String,
    media_ids: Vec<i64>,
    comfy_prompt_id: Option<String>,
    elapsed_ms: i64,
}

impl Terminal {
    fn denied(code: &str, message: &str, elapsed_ms: i64) -> Self {
        Self {
            state: STATE_DENIED,
            ok: false,
            code: Some(code.to_string()),
            message: message.to_string(),
            media_ids: Vec::new(),
            comfy_prompt_id: None,
            elapsed_ms,
        }
    }

    fn failed(code: &str, message: &str, elapsed_ms: i64) -> Self {
        Self {
            state: STATE_FAILED,
            ok: false,
            code: Some(code.to_string()),
            message: message.to_string(),
            media_ids: Vec::new(),
            comfy_prompt_id: None,
            elapsed_ms,
        }
    }
}

/// 真的去提交：**复用 `comfy_submit` 的整条链路**（R10，一条捷径都不抄）。
///
/// 这里**只**负责把 payload 翻译成工具参数、并把工具的结论翻译成"给手机看的一句话"；
/// 审核 / 预检 / 提交 / 等 / `capture.captureRun()` 入库 / `mediaIds` 全在
/// [`crate::workflow::workflow_submit::tool_submit`] 里（同一份实现，别在这里再写一遍）。
async fn execute(state: &AppState, job: &Job) -> Terminal {
    let elapsed_start = Instant::now();
    let policy = match crate::tools::policy::ToolPolicy::load(state) {
        Ok(policy) => policy,
        Err(error) => {
            return Terminal::failed(
                code::TOOL_ERROR,
                &format!("读工具策略失败：{}", error.human()),
                0,
            )
        }
    };

    // ---- 先定"提交哪份工作流" ------------------------------------------------
    //
    // 两条路：手机精确给了 `promptId`（或 `sourceRef` 在受理时已解析成 promptId），
    // 或者只说了"用某族画一张" —— 后者按 §1.2 的① ②档在这里挑（T7）。
    let mut source_kind: Option<&'static str> = None;
    let mut picked_note: Option<String> = None;
    let mut pinned: Vec<(String, String)> = Vec::new();
    let prompt_id = match job.prompt_id {
        Some(id) => id,
        None => {
            let wanted = crate::workflow::auto_select::Wanted {
                task: job
                    .auto
                    .task
                    .as_deref()
                    .and_then(crate::workflow::workflow_recipe::Task::parse),
                family: job.auto.family.clone(),
            };
            let picked = match state.with(|conn| {
                crate::workflow::auto_select::pick(conn, &wanted)
            }) {
                Ok(outcome) => outcome,
                Err(error) => {
                    return Terminal::failed(
                        code::TOOL_ERROR,
                        &format!("挑工作流时读库失败：{}", error.human()),
                        0,
                    )
                }
            };
            let Some(picked) = picked.picked else {
                // **不猜**：把所有试过的档如实写进回执 —— 这正是"需要你确认 X"那句话。
                let tried = if picked.tried.is_empty() {
                    "（没有可用的历史记录）".to_string()
                } else {
                    picked.tried.join("\n")
                };
                return Terminal::failed(
                    "NO_WORKFLOW_MATCH",
                    &format!(
                        "没找到能直接开跑的工作流（这次没有提交任何东西）。\n{tried}\n\
                         要么在手机上指定一份工作流，要么先在电脑上把它跑通一次 / 学成配方并跑通一次。"
                    ),
                    0,
                );
            };
            source_kind = Some(picked.source_kind);
            picked_note = Some(picked.note);
            pinned = picked.overrides;
            picked.prompt_id
        }
    };

    // ---- 参数覆盖：已验证的槽位覆盖打底，调用方给的覆盖**最后说了算** ----
    let mut overrides: Map<String, Value> = Map::new();
    for (key, value) in pinned {
        overrides.insert(key, Value::String(value));
    }
    for (key, value) in &job.overrides {
        overrides.insert(key.clone(), value.clone());
    }

    // ---- 构图 / 分辨率：**默认一个字节都不改**（族与族的分辨率本来就不一样），
    //      只有手机明确给了宽高才写进这份工作流自己的键 ----
    let graph = crate::comfy::capture::prompt_api_graph(state, prompt_id)
        .ok()
        .flatten();
    let mut size_note: Option<String> = None;
    if let Some(graph) = graph.as_ref() {
        let (size, note) = crate::workflow::auto_select::size_overrides(
            graph,
            job.auto.width,
            job.auto.height,
        );
        for (key, value) in size {
            overrides.entry(key).or_insert(Value::String(value));
        }
        size_note = note;
    }

    // ---- 拼 `comfy_submit` 的参数 ----
    let mut arguments = Map::new();
    arguments.insert("promptId".into(), json!(prompt_id));
    if !overrides.is_empty() {
        arguments.insert("overrides".into(), Value::Object(overrides.clone()));
    }
    arguments.insert("nsfw".into(), json!(job.nsfw));
    // 等生成的上限与桌面工具同一条（`DEFAULT_WAIT_SECONDS`）；等不到就如实报 timeout。
    arguments.insert("wait".into(), json!(true));
    arguments.insert(
        "waitSeconds".into(),
        json!(crate::workflow::workflow_submit::DEFAULT_WAIT_SECONDS),
    );
    // 这一跑是哪一档挑出来的（T3 的账要能对上；精确指定的不声明 → 保持列默认）
    if let Some(kind) = source_kind {
        arguments.insert("sourceKind".into(), json!(kind));
    }
    // 手机写好的提示词由桌面写进工作流（`positivePrompt` 会把正面编码器那一路找出来，
    // 并把 TextGenerate 那一支从这次提交的图里摘掉）
    let mut prompt_note: Option<String> = None;
    if let Some(prompt) = job.auto.positive_prompt.as_deref() {
        arguments.insert("positivePrompt".into(), json!(prompt));
    }

    let mut attempt = crate::workflow::workflow_submit::tool_submit(
        state,
        &policy,
        &Value::Object(arguments.clone()),
    )
    .await;

    // 正面编码器认不出来时（比如这份工作流把提示词接在某条开关后面），
    // **退一步写进那个字面量旋钮** —— 但要把"写进了哪个键"说清楚，别假装走了正路。
    if let (Err(failure), Some(prompt)) = (&attempt, job.auto.positive_prompt.as_deref()) {
        if failure.code == "INVALID_ARGUMENT" {
            if let Some(key) = graph
                .as_ref()
                .and_then(crate::workflow::auto_select::prompt_override_key)
            {
                arguments.remove("positivePrompt");
                overrides.insert(key.clone(), Value::String(prompt.to_string()));
                arguments.insert("overrides".into(), Value::Object(overrides.clone()));
                match crate::workflow::workflow_submit::tool_submit(
                    state,
                    &policy,
                    &Value::Object(arguments),
                )
                .await
                {
                    Ok(outcome) => {
                        prompt_note = Some(format!(
                            "这份工作流没有可识别的正面编码器，提示词写进了 `{key}`。"
                        ));
                        attempt = Ok(outcome);
                    }
                    Err(retry) => attempt = Err(retry),
                }
            }
        }
    }

    let outcome = match attempt {
        Ok(outcome) => outcome,
        Err(failure) => {
            return Terminal::failed(
                &failure.code,
                &failure.message,
                elapsed_start.elapsed().as_millis() as i64,
            )
        }
    };

    let mut terminal = classify(&outcome.payload, elapsed_start.elapsed().as_millis() as i64);
    // 回执中如实写明"本次使用了哪份工作流" —— 经验路径选择错误可直接看出
    if let Some(note) = picked_note {
        terminal.message = format!("{}\n{note}", terminal.message);
    }
    if let Some(note) = size_note {
        terminal.message = format!("{}\n{note}", terminal.message);
    }
    if let Some(note) = prompt_note {
        terminal.message = format!("{}\n{note}", terminal.message);
    }
    terminal
}

/// 把 `comfy_submit` 的结构化结果翻译成回执的一档（**四种**：成功 / 无产物 / 超时 / 报错）。
///
/// ⚠ `timeout` **不是失败**（`workflow_submit` 的纪律：既不假装完成也不假装失败）：
/// 它已经进了 ComfyUI 队列，产物跑完会被后台捕获收进画廊，所以回执的 `ok = true` +
/// 稳定码 [`code::TIMEOUT`]，正文说清"还没跑完、稍后看画廊"。
fn classify(payload: &Value, elapsed_ms: i64) -> Terminal {
    let status = payload
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let media_ids: Vec<i64> = payload
        .get("mediaIds")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    let run_key = payload
        .get("runKey")
        .and_then(Value::as_str)
        .map(str::to_string);
    let tool_message = payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let error = payload
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();

    if !media_ids.is_empty() {
        return Terminal {
            state: STATE_DONE,
            ok: true,
            code: None,
            message: format!(
                "手机上排队的那次提交已经跑完了，产物已入库（{} 个），下面这张卡可以直接点开看。",
                media_ids.len()
            ),
            media_ids,
            comfy_prompt_id: run_key,
            elapsed_ms,
        };
    }
    match status {
        "error" => Terminal::failed(
            code::COMFY_RUN_ERROR,
            &if error.is_empty() {
                "这次提交跑了，但 ComfyUI 报错了，没有产物。".to_string()
            } else {
                format!("这次提交跑了，但 ComfyUI 报错了：{error}")
            },
            elapsed_ms,
        ),
        "timeout" => Terminal {
            state: STATE_DONE,
            ok: true,
            code: Some(code::TIMEOUT.to_string()),
            message: "这次提交已经送进 ComfyUI 队列了，但等到超时还没跑完；\
                      跑完之后会自动入库，稍后在画廊里就能看到。"
                .to_string(),
            media_ids: Vec::new(),
            comfy_prompt_id: run_key,
            elapsed_ms,
        },
        "success" | "empty" => Terminal::failed(
            code::NO_MEDIA,
            &if tool_message.is_empty() {
                "这次提交跑完了，但没有可入库的产物文件（工作流可能没有输出节点）。".to_string()
            } else {
                format!("这次提交跑完了，但没有可入库的产物：{tool_message}")
            },
            elapsed_ms,
        ),
        other => Terminal::failed(
            code::TOOL_ERROR,
            &format!(
                "这次提交没有拿到明确的结果（status={other}）：{}",
                if tool_message.is_empty() {
                    "没有更多信息"
                } else {
                    &tool_message
                }
            ),
            elapsed_ms,
        ),
    }
}

// ---------------------------------------------------------------------------
//  回执：往发起那条会话追加一条助手消息（契约 §4.1 第三条）
// ---------------------------------------------------------------------------

/// 回执消息的 id（**确定性的**：同一个 queueId 永远算出同一个 id）。
///
/// 它是"回执只发一次"的判据：投递前先查这个 id 在不在库里，在就算送过了 ——
/// 崩溃重试 / 投递循环重复扫，都不会在会话里冒出第二条气泡。
pub fn reply_message_id(queue_id: &str) -> String {
    format!("submit-reply-{queue_id}")
}

/// 立刻试着投递这条回执（终态之后就调它；失败也不急，[`sweep`] 会重试）。
fn deliver_now(state: &AppState, queue_id: &str) {
    match deliver_reply(state, queue_id) {
        Ok(true) => {}
        Ok(false) => {}
        Err(error) => {
            eprintln!(
                "[submit-queue] queueId={queue_id} 回执这次没送出去（已记在账上，稍后重试）：{}",
                error.human()
            )
        }
    }
}

/// 投递一条回执。返回 `Ok(true)` = 送出去了（或早就送过）。
///
/// 会话不存在 → `Ok(false)` + `reply_state='blocked'` + `reply_error`
/// （**保留队列**：手机那条会话多是还没上行到桌面，等它同步上来再补发）。
fn deliver_reply(state: &AppState, queue_id: &str) -> Result<bool, ApiError> {
    let Some(job) = load(state, queue_id) else {
        return Ok(false);
    };
    if job.reply_state == "delivered" {
        return Ok(true);
    }
    // 只有终态才有回执可发（还在等批准 / 正在提交 = 什么都没得说）
    if job.state == STATE_AWAITING_APPROVAL || job.state == STATE_SUBMITTING {
        return Ok(false);
    }
    let message_id = reply_message_id(queue_id);
    // ① 这个 id 已经在了 = 送过（上一轮成功但没来得及改 reply_state，或者循环重复扫到）
    let existed = state.with(|conn| {
        Ok(conn
            .query_row(
                "SELECT 1 FROM ai_messages WHERE id = ?1",
                rusqlite::params![message_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    })?;
    if existed {
        mark_reply(state, queue_id, "delivered", None, Some(&message_id))?;
        return Ok(true);
    }
    // ② 会话在不在：不在就**如实记一条**，一个字都不假（别假装成功）
    let conversation = job.conversation_id.clone();
    let has_conversation = state.with(|conn| {
        Ok(conn
            .query_row(
                "SELECT 1 FROM ai_conversations WHERE id = ?1",
                rusqlite::params![conversation],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    })?;
    if !has_conversation {
        let reason = format!(
            "会话 {} 还没同步到桌面（手机那条会话的行还没上行），回执先留着，等它上来再补发",
            job.conversation_id
        );
        eprintln!("[submit-queue] queueId={queue_id} 回执送不出去：{reason}");
        mark_reply(state, queue_id, "blocked", Some(&reason), None)?;
        return Ok(false);
    }

    // ③ 写消息 + 有序块（**同一个事务**，`append_message` 里已保证）
    let (text, result_text, result_payload) = compose_reply(&job, queue_id);
    let append = MessageAppend {
        role: "assistant".to_string(),
        text: text.clone(),
        status: "complete".to_string(),
        provider_id: None,
        model_id: None,
        id: Some(message_id.clone()),
        parts: vec![
            AiMessagePartDto {
                r#type: "text".to_string(),
                text: Some(text),
                attachment_id: None,
                tool_call_id: None,
                json_payload: None,
            },
            AiMessagePartDto {
                r#type: "tool_call".to_string(),
                text: None,
                attachment_id: None,
                tool_call_id: Some(job.call_id.clone()),
                json_payload: Some(json!({
                    "name": "comfy_submit",
                    "arguments": submit_arguments_json(&job),
                })),
            },
            AiMessagePartDto {
                r#type: "tool_result".to_string(),
                text: Some(result_text),
                attachment_id: None,
                tool_call_id: Some(job.call_id.clone()),
                json_payload: Some(result_payload),
            },
        ],
    };
    match crate::ai::conversations::append_message(state, &job.conversation_id, &append) {
        Ok(_) => {
            mark_reply(state, queue_id, "delivered", None, Some(&message_id))?;
            Ok(true)
        }
        Err(error) => {
            let reason = format!("写回执消息失败：{}", error.human());
            eprintln!("[submit-queue] queueId={queue_id} {reason}");
            mark_reply(state, queue_id, "blocked", Some(&reason), None)?;
            Ok(false)
        }
    }
}

/// 回执的正文 + 工具结果块（形状与执行器落的那一条**同形**：
/// `name` / `ok` / `code` / `elapsedMs` / `approval` + 有产物时的 `mediaIds`）。
fn compose_reply(job: &Job, queue_id: &str) -> (String, String, Value) {
    let text = job
        .outcome_message
        .clone()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| "手机排队的那次提交已经收尾了。".to_string());
    let mut payload = Map::new();
    payload.insert("name".into(), json!("comfy_submit"));
    payload.insert("ok".into(), json!(job.ok.unwrap_or(false)));
    if let Some(code) = job.outcome_code.as_ref().filter(|c| !c.trim().is_empty()) {
        payload.insert("code".into(), json!(code));
    }
    if let Some(message) = job
        .outcome_message
        .as_ref()
        .filter(|m| !m.trim().is_empty())
    {
        payload.insert("error".into(), json!(message));
    }
    payload.insert("elapsedMs".into(), json!(job_elapsed_ms(job)));
    payload.insert("approval".into(), json!(job.approval));
    payload.insert("mediaIds".into(), json!(job.media_ids));
    // 手机侧要能把这行回执对回自己那条 outbox（`queueId` 是它生成的）。
    // 契约 §4.1 没写这个键，但多一个键对两端都无害（Dart 侧按名字读已知键）。
    payload.insert("queueId".into(), json!(queue_id));

    let mut detail = format!("排队提交 {queue_id}（approval={}）", job.approval);
    if job.media_ids.is_empty() {
        if let Some(code) = job.outcome_code.as_ref() {
            detail.push_str(&format!("：{code}"));
        }
    } else {
        detail.push_str(&format!("：产物 {} 个", job.media_ids.len()));
    }
    (text, detail, Value::Object(payload))
}

/// 工具卡上显示的参数（与真正提交时用的那份**同源**，免得两处漂移）。
fn submit_arguments_json(job: &Job) -> String {
    let mut arguments = Map::new();
    if let Some(prompt_id) = job.prompt_id {
        arguments.insert("promptId".into(), json!(prompt_id));
    }
    if !job.overrides.is_empty() {
        arguments.insert("overrides".into(), Value::Object(job.overrides.clone()));
    }
    // "只说族"那一路：把手机说的那几项也带上，工具卡上才看得出这次是怎么挑的工作流
    if let Some(task) = &job.auto.task {
        arguments.insert("task".into(), json!(task));
    }
    if let Some(family) = &job.auto.family {
        arguments.insert("modelFamily".into(), json!(family));
    }
    if let Some(prompt) = &job.auto.positive_prompt {
        arguments.insert("positivePrompt".into(), json!(truncate_chars(prompt, 400)));
    }
    if let Some(width) = job.auto.width {
        arguments.insert("width".into(), json!(width));
    }
    if let Some(height) = job.auto.height {
        arguments.insert("height".into(), json!(height));
    }
    arguments.insert("nsfw".into(), json!(job.nsfw));
    Value::Object(arguments).to_string()
}

/// `elapsedMs`：受理到终态的实际耗时（回执中显示该值）。
fn job_elapsed_ms(job: &Job) -> i64 {
    job.elapsed_ms
}

// ---------------------------------------------------------------------------
//  库操作（申请 / 认领 / 收尾 / 回执账）
// ---------------------------------------------------------------------------

/// 读一行（不存在 → `None`）。
pub fn load(state: &AppState, queue_id: &str) -> Option<Job> {
    state.with(|conn| read_job(conn, queue_id)).ok().flatten()
}

fn read_job(conn: &Connection, queue_id: &str) -> rusqlite::Result<Option<Job>> {
    conn.query_row(
        "SELECT queue_id, call_id, conversation_id, run_id, prompt_id, source_ref, overrides_json,
                nsfw, state, approval, ok, outcome_code, outcome_message, media_ids_json,
                elapsed_ms, reply_state, reply_error, payload_json
           FROM submit_jobs WHERE queue_id = ?1",
        rusqlite::params![queue_id],
        |row| {
            let overrides_json: Option<String> = row.get("overrides_json")?;
            let media_ids_json: Option<String> = row.get("media_ids_json")?;
            // "只说族"那几项**存在 payload_json 里**（不另加列：它本来就是这次指令的原文）
            let payload_json: Option<String> = row.get("payload_json")?;
            let auto = payload_json
                .as_deref()
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
                .and_then(|value| value.as_object().cloned())
                .map(|obj| AutoPick::from_payload(&obj))
                .unwrap_or_default();
            Ok(Job {
                queue_id: row.get("queue_id")?,
                call_id: row.get("call_id")?,
                conversation_id: row.get("conversation_id")?,
                run_id: row.get("run_id")?,
                prompt_id: row.get("prompt_id")?,
                source_ref: row.get("source_ref")?,
                auto,
                overrides: overrides_json
                    .as_deref()
                    .and_then(|text| serde_json::from_str::<Value>(text).ok())
                    .and_then(|value| value.as_object().cloned())
                    .unwrap_or_default(),
                nsfw: row.get::<_, i64>("nsfw")? != 0,
                state: row.get("state")?,
                approval: row.get("approval")?,
                ok: row.get::<_, Option<i64>>("ok")?.map(|v| v != 0),
                outcome_code: row.get("outcome_code")?,
                outcome_message: row.get("outcome_message")?,
                media_ids: media_ids_json
                    .as_deref()
                    .and_then(|text| serde_json::from_str::<Vec<i64>>(text).ok())
                    .unwrap_or_default(),
                elapsed_ms: row.get::<_, Option<i64>>("elapsed_ms")?.unwrap_or(0),
                reply_state: row.get("reply_state")?,
                reply_error: row.get("reply_error")?,
            })
        },
    )
    .optional()
}

/// `(state, approval)` —— 审批闸门读的就是这两个字段。
fn approval_of(state: &AppState, queue_id: &str) -> Result<Option<(String, String)>, ApiError> {
    state.with(|conn| {
        conn.query_row(
            "SELECT state, approval FROM submit_jobs WHERE queue_id = ?1",
            rusqlite::params![queue_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
    })
}

/// 认领：`awaiting_approval + approved` → `submitting`（返回 `true` 的那一个才往下走）。
fn claim(state: &AppState, queue_id: &str) -> bool {
    let queue_id = queue_id.to_string();
    state
        .write(move |tx| {
            let n = tx.execute(
                "UPDATE submit_jobs
                    SET state = ?2,
                        heartbeat_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                        updated_at   = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                  WHERE queue_id = ?1 AND state = ?3 AND approval IN ('approved', 'auto')",
                rusqlite::params![queue_id, STATE_SUBMITTING, STATE_AWAITING_APPROVAL],
            )?;
            Ok(n)
        })
        .map(|n| n > 0)
        .unwrap_or(false)
}

/// 落终态（**单赢家**：`from_state` 必须还对得上，否则说明别人已经处理过这一条）。
///
/// `elapsed_ms` 在这里按"受理那一刻到现在的真实读数"重算（`created_at` 与 `now` 都是
/// SQLite 自己的时间，跨进程也不会算错）—— 审批等了两分钟，回执里就该写着两分钟。
fn finalize(state: &AppState, queue_id: &str, from_state: &str, terminal: &Terminal) -> bool {
    let queue_id = queue_id.to_string();
    let terminal = terminal.clone();
    let from_state = from_state.to_string();
    state
        .write(move |tx| {
            let n = tx.execute(
                "UPDATE submit_jobs
                    SET state = ?2, ok = ?3, outcome_code = ?4, outcome_message = ?5,
                        media_ids_json = ?6, comfy_prompt_id = ?7,
                        elapsed_ms = CASE
                            WHEN ?8 > 0 THEN ?8
                            ELSE CAST((julianday('now') - julianday(created_at)) * 86400000 AS INTEGER)
                        END,
                        approval = CASE WHEN ?2 = 'denied' THEN 'denied' ELSE approval END,
                        heartbeat_at = NULL,
                        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                  WHERE queue_id = ?1 AND state = ?9",
                rusqlite::params![
                    queue_id,
                    terminal.state,
                    if terminal.ok { 1 } else { 0 },
                    terminal.code,
                    terminal.message,
                    if terminal.media_ids.is_empty() {
                        None
                    } else {
                        Some(serde_json::to_string(&terminal.media_ids).unwrap_or_else(|_| "[]".to_string()))
                    },
                    terminal.comfy_prompt_id,
                    terminal.elapsed_ms,
                    from_state,
                ],
            )?;
            Ok(n)
        })
        .map(|n| n > 0)
        .unwrap_or(false)
}

/// 记回执那一笔账（`reply_state` / `reply_error` / `reply_message_id`）。
fn mark_reply(
    state: &AppState,
    queue_id: &str,
    reply_state: &str,
    error: Option<&str>,
    message_id: Option<&str>,
) -> Result<(), ApiError> {
    let (queue_id, reply_state, error, message_id) = (
        queue_id.to_string(),
        reply_state.to_string(),
        error.map(str::to_string),
        message_id.map(str::to_string),
    );
    state.write(move |tx| {
        tx.execute(
            "UPDATE submit_jobs
                SET reply_state = ?2, reply_error = ?3,
                    reply_message_id = COALESCE(?4, reply_message_id),
                    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
              WHERE queue_id = ?1",
            rusqlite::params![queue_id, reply_state, error, message_id],
        )?;
        Ok(())
    })?;
    Ok(())
}

/// 租约心跳（后台任务活着的凭据）。
fn start_heartbeat(state: AppState, queue_id: String) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(HEARTBEAT_SECONDS)).await;
            let beat = state.write(|tx| {
                let n = tx.execute(
                    "UPDATE submit_jobs
                        SET heartbeat_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                      WHERE queue_id = ?1 AND state IN ('awaiting_approval','submitting')",
                    rusqlite::params![queue_id],
                )?;
                Ok(n)
            });
            match beat {
                Ok(0) => break, // 已经终态（或行没了）：心跳没意义了
                Ok(_) => {}
                Err(error) => eprintln!(
                    "[submit-queue] queueId={queue_id} 心跳失败：{}",
                    error.human()
                ),
            }
        }
    })
}

// ---------------------------------------------------------------------------
//  审批通道（复用 /api/ai/pending 与 /api/ai/tool-calls/{id}/approve|deny）
// ---------------------------------------------------------------------------

/// 待批准的排队提交 → `/api/ai/pending` 的 `approvals[]` 条目（**形状与工具审批逐字同形**）。
///
/// 键集合必须是 `{callId, runId, conversationId, name, argumentsJson, askedAt}` ——
/// 少一个键，桌面上那张卡就少一块内容（`pending.rs` 的文件头写明了这条纪律）。
///
/// ⚠ 判据是 `state='awaiting_approval' **AND** approval='pending'`：
/// 人一点批准（`approval` 变 `approved`，`state` 还要等到 worker 认领才动）这一条就
/// **立刻从待办里消失** —— 否则"我的决定"与"它还在等我点"会同时成立，
/// 那张卡点了也没用（与工具审批那边"批完就等 invoke 收尾"的窗口是同一类问题，
/// 只是这里的提交要跑分钟级，不能让它一直挂在列表上）。
pub fn pending_approvals(conn: &Connection) -> rusqlite::Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT queue_id, call_id, conversation_id, run_id, prompt_id, overrides_json, nsfw, created_at
           FROM submit_jobs
          WHERE state = 'awaiting_approval' AND approval = 'pending'
          ORDER BY created_at, queue_id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let prompt_id: Option<i64> = row.get("prompt_id")?;
            let overrides_json: Option<String> = row.get("overrides_json")?;
            let mut arguments = Map::new();
            if let Some(id) = prompt_id {
                arguments.insert("promptId".into(), json!(id));
            }
            if let Some(text) = overrides_json.as_deref() {
                if let Ok(value) = serde_json::from_str::<Value>(text) {
                    arguments.insert("overrides".into(), value);
                }
            }
            arguments.insert("nsfw".into(), json!(row.get::<_, i64>("nsfw")? != 0));

            let mut out = Map::new();
            out.insert("callId".into(), json!(row.get::<_, String>("call_id")?));
            out.insert(
                "runId".into(),
                json!(row.get::<_, Option<String>>("run_id")?.unwrap_or_default()),
            );
            out.insert(
                "conversationId".into(),
                json!(row.get::<_, String>("conversation_id")?),
            );
            // 名字就是它真正要干的事：批准 = 让桌面**真的提交**（R10 那条链路）。
            out.insert("name".into(), json!("comfy_submit"));
            out.insert(
                "argumentsJson".into(),
                json!(Value::Object(arguments).to_string()),
            );
            out.insert(
                "askedAt".into(),
                match crate::infra::iso_time::iso_time(
                    row.get::<_, Option<String>>("created_at")?.as_deref(),
                ) {
                    Some(text) => Value::String(text),
                    None => Value::Null,
                },
            );
            Ok(Value::Object(out))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 已选择「批准 / 拒绝」（由 [`crate::ai::runs::resolve_approval`] 调用，**不再另开端点**）。
///
/// 与工具审批**同一条语义**：`WHERE approval='pending'` 那一句是"决定只能下一次" ——
/// 重复点返回 0，而且**没人能把已经批过的提交改成拒绝**（否则那种"我点错了、快撤"
/// 会撤回一个已经在跑的命令）。
pub fn resolve_approval_on(
    tx: &Transaction<'_>,
    call_id: &str,
    approved: bool,
) -> rusqlite::Result<usize> {
    tx.execute(
        "UPDATE submit_jobs
            SET approval = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
          WHERE call_id = ?2 AND state = 'awaiting_approval' AND approval = 'pending'",
        rusqlite::params![if approved { "approved" } else { "denied" }, call_id],
    )
}

// ---------------------------------------------------------------------------
//  重启恢复 / 看门狗 / 回执投递（第 9 条：受理了但还没跑完不会丢）
// ---------------------------------------------------------------------------

/// **进程启动时的恢复**（`main.rs` 在三条既有恢复之后调一次）。
///
/// 三条规则，每条都对应一种"上次进程死得不明不白"的残局：
///
/// | 残局 | 处置 | 为什么 |
/// | --- | --- | --- |
/// | `awaiting_approval` + `approval='pending'` | 按**拒绝**收尾（`APPROVAL_INTERRUPTED`）+ 回执 | 与 `expire_stale_pending_tool_calls` **同一语义**：后端重启，内存里的等待随进程消失，"永远不会有人点"。留着一张点了没用的卡才是最坏的表现 |
/// | `awaiting_approval` + `approval IN ('approved','auto')` | **复起 worker**（人已经批了 / 设备放权过，只是进程在中间没了） | 认领是原子的，复起不会重复提交 |
/// | `submitting` | 按 `SUBMIT_INTERRUPTED` **如实**收尾，**不重投** | 是否已送入 ComfyUI、执行到哪一步均未知。重投会额外消耗一次生成资源；若已完成，后台捕获会写入画廊 |
pub fn resume_in_flight(state: &AppState) -> usize {
    let interrupted = state
        .write(|tx| {
            // ① 还在等人的（没人点过了）→ 拒绝
            let n = tx.execute(
                "UPDATE submit_jobs
                    SET state = 'denied', approval = 'denied', ok = 0,
                        outcome_code = ?1,
                        outcome_message = '桌面重启了，那次审批等待被打断（按拒绝处理），这次提交没有提交。',
                        elapsed_ms = CAST((julianday('now') - julianday(created_at)) * 86400000 AS INTEGER),
                        heartbeat_at = NULL,
                        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                  WHERE state = 'awaiting_approval' AND approval = 'pending'",
                rusqlite::params![code::APPROVAL_INTERRUPTED],
            )?;
            // ② 提交中途没了的（**不重投**，如实说"结果不确定"）
            let m = tx.execute(
                "UPDATE submit_jobs
                    SET state = 'failed', ok = 0, outcome_code = ?1,
                        outcome_message = '桌面在提交过程中中断了，这次结果不确定：可能已经送进 ComfyUI 队列，也可能还没送出去。请到画廊 / ComfyUI 队列核对；真跑完了产物会被自动捕获。',
                        elapsed_ms = CAST((julianday('now') - julianday(created_at)) * 86400000 AS INTEGER),
                        heartbeat_at = NULL,
                        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                  WHERE state = 'submitting'",
                rusqlite::params![code::SUBMIT_INTERRUPTED],
            )?;
            Ok(n + m)
        })
        .unwrap_or(0);
    if interrupted > 0 {
        eprintln!("[submit-queue] 启动恢复：{interrupted} 条排队提交按终态收尾（审批被打断 / 提交被中断）");
    }

    // ③ 人已经批过、只是进程在中间没了的：复起 worker（认领原子，不会重复提交）
    let approved = awaiting_ids(state, "AND approval IN ('approved', 'auto')").unwrap_or_default();
    for queue_id in &approved {
        eprintln!("[submit-queue] 启动恢复：queueId={queue_id} 已经批准过，复起提交");
        start(state, queue_id);
    }

    // ④ 回执还没送出去的（含上面刚收尾的那批）
    let delivered = deliver_pending(state);
    if delivered > 0 {
        eprintln!("[submit-queue] 启动恢复：补发 {delivered} 条回执");
    }

    interrupted + approved.len()
}

/// 起后台清扫循环（**只在 `main.rs` 调一次**；测试直接调 [`sweep`]）。
///
/// 这个循环**永不退出**：它同时是"看门狗"（worker 猝死 / 审批被中断的兜底）、
/// "孤儿复起"（批了但没人认领）与"回执投递"（会话晚到 / 写库失败的重试）。
pub fn spawn_sweeper(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(SWEEP_INTERVAL_SECONDS)).await;
            let report = sweep(&state);
            for queue_id in &report.need_worker {
                start(&state, queue_id);
            }
            if report.touched() > 0 {
                eprintln!(
                    "[submit-queue] 清扫：收尾 {} 条 / 复起 {} 条 / 回执送达 {} 条 / 回执仍阻塞 {} 条",
                    report.finalized,
                    report.need_worker.len(),
                    report.delivered,
                    report.blocked,
                );
            }
        }
    });
}

/// 一次清扫的结果（给日志与用例看）。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SweepReport {
    /// 被看门狗按终态收尾的条数（审批租约过期 / 提交租约过期 / 决定但没人认领）。
    pub finalized: usize,
    /// **需要复起 worker** 的 queueId（`approved` 但还没被认领）。
    pub need_worker: Vec<String>,
    /// 这一轮送出去的回执条数。
    pub delivered: usize,
    /// 这一轮仍然送不出去的（会话还没同步上来 / 写库失败）。
    pub blocked: usize,
}

impl SweepReport {
    pub fn touched(&self) -> usize {
        self.finalized + self.need_worker.len() + self.delivered + self.blocked
    }
}

/// 清扫一轮：**看门狗 + 孤儿复起 + 回执投递**（同步实现，便于用例直接调）。
///
/// 看门狗判据是**租约**（`heartbeat_at` 比现在早过 [`LEASE_SECONDS`]），不是"等了多久"：
/// 一个正常的提交可以在 `tool_submit` 里待满 `DEFAULT_WAIT_SECONDS`（240 秒），
/// 用"创建时间"判会把**正在跑的**打成失败。租约只会在"那个 worker 真的不在了"时过期。
pub fn sweep(state: &AppState) -> SweepReport {
    let mut report = SweepReport::default();
    let lease = format!("-{LEASE_SECONDS} seconds");

    // ① 决定已经落了、但没人在等它（worker 死了 / 上个进程留下的）
    report.finalized += finalize_decided_without_worker(state);
    // ② 审批租约过期（worker 猝死，没人给它超时）→ 按超时拒绝。
    //    ⚠ 只看 `approval='pending'`：已经批过的那种是第 ④ 步的**复起**，不是拒绝。
    report.finalized += finalize_expired(
        state,
        STATE_AWAITING_APPROVAL,
        "AND approval = 'pending'",
        &lease,
        &Terminal::denied(
            code::APPROVAL_TIMEOUT,
            "等了很久也没人批准（桌面上那个等待它的进程已经不在了），这次提交没有提交。",
            0,
        ),
    );
    // ③ 提交租约过期（worker 在提交中途没了）→ 如实说"结果不确定"，**不重投**
    report.finalized += finalize_expired(
        state,
        STATE_SUBMITTING,
        "",
        &lease,
        &Terminal::failed(
            code::SUBMIT_INTERRUPTED,
            "桌面在提交过程中中断了，这次结果不确定：可能已经送进 ComfyUI 队列，也可能还没送出去。\
             请到画廊 / ComfyUI 队列核对；真跑完了产物会被自动捕获。",
            0,
        ),
    );
    // ④ 人已经批过、只是没人认领 → 交给调用方复起（认领是原子的，复起不会重复提交）
    report.need_worker = awaiting_ids(state, "AND approval IN ('approved', 'auto')").unwrap_or_default();
    // ⑤ 回执投递（终态 + 还没送达）：会话晚到 / 写库失败都在这里重试
    let delivered = deliver_pending(state);
    report.delivered = delivered;
    report.blocked = blocked_reply_count(state).unwrap_or(0);
    report
}

/// 终态 + 回执还没送达的，全都再试一遍。
pub fn deliver_pending(state: &AppState) -> usize {
    let ids = state
        .with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT queue_id FROM submit_jobs
                  WHERE state NOT IN ('awaiting_approval','submitting') AND reply_state <> 'delivered'
                  ORDER BY updated_at, queue_id LIMIT 200",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap_or_default();
    let mut delivered = 0;
    for queue_id in ids {
        if matches!(deliver_reply(state, &queue_id), Ok(true)) {
            delivered += 1;
        }
    }
    delivered
}

fn blocked_reply_count(state: &AppState) -> Result<usize, ApiError> {
    state.with(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM submit_jobs
              WHERE state NOT IN ('awaiting_approval','submitting') AND reply_state = 'blocked'",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize)
    })
}

/// 还在等批准、且 `extra` 条件成立的那些 queueId。
fn awaiting_ids(state: &AppState, extra: &str) -> Result<Vec<String>, ApiError> {
    state.with(|conn| {
        let sql =
            format!("SELECT queue_id FROM submit_jobs WHERE state = 'awaiting_approval' {extra}");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// "决定已落（approved / denied）但状态还停在 `awaiting_approval`、而且**没人在等**"的收尾。
///
/// 两种情况：① `denied` —— 人点了拒绝，但当时那个 worker 已经不在了（或上个进程的残留）；
/// ② `approved` —— 那是 [`sweep`] 第 ④ 步的复起（**不在这里收尾**，所以只处理 denied）。
fn finalize_decided_without_worker(state: &AppState) -> usize {
    let ids = awaiting_ids(state, "AND approval = 'denied'").unwrap_or_default();
    let mut done = 0;
    for queue_id in ids {
        // 单赢家：能改到那一行才轮到我们写回执
        if finalize(
            state,
            &queue_id,
            STATE_AWAITING_APPROVAL,
            &Terminal::denied(code::APPROVAL_DENIED, "这次提交被桌面拒绝了，没有提交。", 0),
        ) {
            deliver_now(state, &queue_id);
            done += 1;
        }
    }
    done
}

/// 租约过期的那些按给定终态收尾。
fn finalize_expired(
    state: &AppState,
    from_state: &str,
    extra: &str,
    lease: &str,
    terminal: &Terminal,
) -> usize {
    let from_state_owned = from_state.to_string();
    let lease = lease.to_string();
    let sql = format!(
        "SELECT queue_id FROM submit_jobs
          WHERE state = ?1 {extra}
            AND (heartbeat_at IS NULL
                 OR heartbeat_at < strftime('%Y-%m-%dT%H:%M:%fZ','now', ?2))"
    );
    let ids = state
        .with(move |conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(rusqlite::params![from_state_owned, lease], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap_or_default();
    let mut done = 0;
    for queue_id in ids {
        if finalize(state, &queue_id, from_state, terminal) {
            deliver_now(state, &queue_id);
            done += 1;
        }
    }
    done
}

// ---------------------------------------------------------------------------
//  小工具
// ---------------------------------------------------------------------------

fn now_iso() -> String {
    crate::infra::iso_time::from_system_time(std::time::SystemTime::now())
}

fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// 数字或数字字符串 → `i64`（手机上 `promptId` 可能是 JSON 数字，也可能是字符串）。
fn arg_i64(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests;

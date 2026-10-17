//! 工具结果缓存 —— 「同一个工具 + 同一组参数」在短时间内**不重复跑第二遍**。
//!
//! # 为什么要有它（2026-10）
//!
//! 工具往返循环里最贵的三类重复**不是模型乱调**，而是它**合理地再确认一次**：
//!
//! | 重复 | 现在的代价 | 缓存之后 |
//! | --- | --- | --- |
//! | `comfy_get_status` 连问三次队列 | 三次真实查询，且吃 `MAX_COMFY_QUERIES_PER_RUN` | 一次真实查询，后两次 0 请求 |
//! | 同一个 `web_search` 查询词搜两遍 | 两次真实出网 + 吃 `MAX_WEB_QUERIES_PER_RUN` | 第二遍 0 请求 |
//! | `read_file` 同一个文件读两遍 | 两次磁盘读（大文件还有 2MB 上限的截断） | 第二次 0 读 |
//!
//! 两条**独立**的预算（联网 / ComfyUI）本来就是为这件事立的栅栏（AIH-036：「不许把
//! ComfyUI 当轮询器」）—— 栅栏拦下的是"真请求"，而缓存拦下的是"重复"。
//! 于是同参数再问一次从"被拒 + 白等"变成"立刻拿到同一份结果"，
//! **而那两条预算只被真正的请求消耗**。
//!
//! # 六条纪律
//!
//! 1. **只缓存纯读工具，名单逐条写在 [`freshness`] 里** —— 不用
//!    `!is_mutating` 一刀切：那会把 `comfy_load_workflow`（会写库）和
//!    `ask_user_question`（闸门）一起卷进来。名单是可审计的，一刀切不是。
//! 2. **命中一律如实说**：正文开头那句 `[缓存命中] …（没有重新执行）` 是
//!    [`Cached::into_wire`] 拼的，**由模型与用户一起看见**（工具卡上的摘要也是它）。
//!    悄悄返回旧数据 = 模型把几分钟前的队列状态当成现在的事实。
//! 3. **审批语义一个字都不改**：查缓存发生在**审批之后**（见
//!    [`crate::ai::executor::handle_call`] 的步骤 ⑥）。所以"用户没批准"永远优先 ——
//!    缓存只免掉"再跑一遍"，不免掉"再问一次"。
//! 4. **写操作一律清空缓存**（[`invalidates`]）：内存里的重建成本是几次重复调用，
//!    比"读到旧文件"便宜得多。**别为省这一次清空去搞精细失效** —— 那正是
//!    AGENTS §10 里"加缓存要有失效策略"那条纪律的反面教材。
//! 5. **有界**：最多 [`MAX_ENTRIES`] 条、单条正文最多 [`MAX_CONTENT_CHARS`] 字符。
//! 6. **失败不进缓存**：一次超时 / 429 / 越界被拒是"这一次"的事实，
//!    存下来等于把偶发故障变成长达几分钟的"必然失败"。
//!
//! # 磁盘类工具为什么按 `stat` 校验而不是只认 TTL
//!
//! `read_file` / `list_dir` 的答案随时可能被**我们之外的人**改掉：用户在自己的编辑器里
//! 存一次、AI 上一轮用 `write_file` 写一次、别的工具投一张图进 `input\`。TTL 只能把窗口
//! 缩小，而 `mtime + 长度` 能把它关掉 —— 文件被动过一个字节，下一次调用立刻重新读。
//! 顺带还买到一个更重要的性质：**路径要过 [`ToolPolicy::resolve_read`]**，
//! 所以"用户把某个目录从读白名单里删掉之后，缓存还在供那份内容"这件事不会发生。
//!
//! # 与 Kotlin 的关系
//!
//! **没有对应物**（Kotlin 的 `ToolRegistry` 每次都真跑）。这是 Rust 侧新增的一层，
//! 所以它**不在 run-kit 的任何工件里**：手机 harness 走自己的工具循环，
//! 缓存是桌面后端的进程内状态，两端行为不一致是**有意**的（手机不缓存 = 更保守）。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

use crate::tools::policy::ToolPolicy;

/// 缓存条目数上限。
///
/// 有界是硬要求：工具结果最长 32k 字符（[`crate::tools::tools::MAX_RESULT_CHARS`]），
/// 无界就是一个"跑一整天之后占几百 MB"的内存泄漏。
pub const MAX_ENTRIES: usize = 96;

/// 单条正文能进缓存的上限（与结果截断同一档）。
///
/// 比它长的结果是"整个世界倒进来"的那种（一份 2MB 的文件正文），
/// 本来就会在回给模型之前被截断，**不值得再存一份**。
pub const MAX_CONTENT_CHARS: usize = 32_000;

/// 一条缓存的新鲜度判据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// 只认时间：`TTL` 之内一律命中。
    ///
    /// 适合"答案只跟外部服务/库有关，而我们改不动它"的那些工具（活体探测、联网、库读）。
    Ttl(Duration),
    /// 时间 + **目标路径的 `mtime` 与长度**：被动过就作废。
    ///
    /// 适合"答案就是磁盘上的那份内容"的那些工具。见模块头"磁盘类工具为什么按 stat 校验"。
    File(Duration),
}

impl Freshness {
    fn ttl(self) -> Duration {
        match self {
            Freshness::Ttl(d) | Freshness::File(d) => d,
        }
    }
}

const fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// **哪些工具的结果可以复用**（`None` = 一律现跑）。
///
/// 名单是逐条写死的，理由逐条写在下面 —— 加一条之前先问三个问题：
/// ① 它会不会改世界（改 → 不许进）；② 它的答案会不会被我们之外的人改（会 → 用
/// [`Freshness::File`] 或短 TTL）；③ 它有没有**自己更严的去重**（有 → 不进，见 `load_skill`）。
pub fn freshness(name: &str) -> Option<Freshness> {
    use Freshness::{File, Ttl};
    Some(match name {
        // ---- 磁盘上的那份内容：按 stat 校验 ----
        //
        // 30 秒的 TTL 只是"兜底"（`list_dir` 的目录 mtime 在**子文件内容**变化时不动），
        // 真正的判据是 mtime + 长度。
        "read_file" | "list_dir" => File(secs(30)),

        // ---- 本机库 / 本机目录：读起来便宜，但用户（或另一台端）随时会改 ----
        "list_skills" | "search_prompts" | "search_character_cards" => Ttl(secs(20)),
        "comfy_list_workflows" | "comfy_find_workflow" | "comfy_list_recipes" => Ttl(secs(20)),
        "comfy_env_report" => Ttl(secs(60)),

        // ---- 活体探测：**短 TTL**，它答的是"此刻" ----
        //
        // 10 秒是刻意短的：这一档的价值不是"省时间"，而是让"再确认一下"这句话
        // **不必吃 [`crate::tools::policy::MAX_COMFY_QUERIES_PER_RUN`]** ——
        // 同一毫秒内连问两次（模型把一句话拆成两个调用是常态）一定命中，
        // 而真正的轮询（隔几秒问一次）照样拿到新答案。
        "comfy_get_status" | "comfy_get_run" => Ttl(secs(10)),

        // ---- 纯计算：输入相同，结果必然相同（不碰世界、不读库） ----
        "comfy_check_inputs" | "comfy_fit_workflow" | "comfy_audit_workflow"
        | "comfy_prune_workflow" | "comfy_bind_recipe" => Ttl(secs(60)),

        // ---- 出网：一次请求就是一次真实的外部调用 ----
        //
        // `find_model` 最长（它会挨个问三家模型站）；`web_fetch` 的正文也不该 5 分钟内重复拉。
        "web_search" | "web_fetch" | "civitai_search" | "find_model" | "danbooru_lookup" => {
            Ttl(secs(300))
        }

        // ⚠ `load_skill` **刻意不在名单里**：它自己有一条更严的去重
        //   （`ToolSession.loaded_skills`，同一轮第二次直接回 `SKILL_ALREADY_LOADED`）。
        //   进了缓存就会变成"第二次又把整篇正文灌一遍"—— 去重失效、上下文白烧。
        //   跨 Run 重读一份 skill 是常态，但它本来就只有一次磁盘读，省不出什么。
        //
        // ⚠ `ask_user_question` 同理：它是**闸门**，不是查询（缓存一个提问 = 吞掉用户的选择）。
        //
        // 其余的（`write_file` / `register_skill` / `delete_skill` / `remember` /
        // `comfy_submit` / `comfy_use_*` / `comfy_make_mask` / `comfy_start` / `comfy_stop` /
        // `comfy_sync_history` / `comfy_learn_workflow` / `save_character_card` /
        // `comfy_load_workflow`…）**全部不进**：它们改世界，见 [`invalidates`]。
        _ => return None,
    })
}

/// 这次调用会不会改动"缓存所描述的世界"（→ 成功之后一律清空缓存）。
///
/// 判据是工具的静态 `mutating` 位**加上**一条显式的例外：
/// `comfy_load_workflow` 的 `mutating` 是 `false`（它不改用户机器上的任何文件），
/// 但它**会往库里写一条提示词** —— 不清缓存的话，"导入 → 立刻搜库"会搜不到刚导进来的那条。
pub fn invalidates(name: &str) -> bool {
    crate::tools::tools::is_mutating(name) || name == "comfy_load_workflow"
}

/// 清空缓存，返回清掉的条数。
///
/// 调用点只有一个：[`crate::ai::executor::handle_call`] 在写类工具**执行成功之后**。
pub fn invalidate_all() -> usize {
    let mut guard = cache().lock().unwrap_or_else(|e| e.into_inner());
    let cleared = guard.len();
    guard.clear();
    cleared
}

/// 当前缓存条数（用例与排错用；业务代码不该依赖它）。
pub fn entry_count() -> usize {
    cache().lock().unwrap_or_else(|e| e.into_inner()).len()
}

/// 一次命中。
#[derive(Debug, Clone)]
pub struct Cached {
    content: String,
    payload: Value,
    /// 这份结果是什么时候存进来的（"N 秒前"那句话用它）
    age: Duration,
    /// 它还能用多久（缓存对自己寿命的**如实交代**，别让模型猜）
    ttl: Duration,
}

impl Cached {
    /// 命中 → 喂回模型 / 落块的那三件（正文 / 摘要 / 结构化负载）。
    ///
    /// 纪律 2 就在这几行里：正文开头**必须**写明这是哪一次的结果、以及它还能用多久 ——
    /// 少了这句话，模型会把几分钟前的队列状态当成现在的事实，而且**它没有任何线索**。
    ///
    /// 摘要（工具卡上那一行小字）从**加了标注的正文**重新算，所以用户也看得见"缓存命中"；
    /// `payload` 里补 `cached` / `cachedAgeSeconds` 两个键给界面（它们是**追加**的，
    /// 老的读取方一个都不受影响）。
    pub fn into_wire(self) -> (String, String, Value) {
        let note = format!(
            "[缓存命中] 同一工具 + 同一组参数在 {} 前跑过一次，下面是**那一次**的结果\
             （没有重新执行；这份缓存还能用 {}）。要拿最新数据请换一个参数。\n",
            human_age(self.age),
            human_age(self.ttl.saturating_sub(self.age)),
        );
        let content = format!("{note}{}", self.content);
        let preview = crate::tools::tools::preview_of(&content);
        let payload = match self.payload {
            Value::Object(mut map) => {
                map.insert("cached".into(), Value::Bool(true));
                map.insert("cachedAgeSeconds".into(), json!(self.age.as_secs()));
                Value::Object(map)
            }
            // 非对象负载（工具本可以返回数组/字符串）：**不吞**，包一层再说
            other => json!({
                "cached": true,
                "cachedAgeSeconds": self.age.as_secs(),
                "result": other,
            }),
        };
        (content, preview, payload)
    }
}

/// 查一次缓存。`None` = 没命中（**照旧真跑**）。
///
/// 只在 [`crate::ai::executor::handle_call`] 的步骤 ⑥ 调（审批之后）——
/// 见模块头纪律 3。
pub fn lookup(policy: &ToolPolicy, name: &str, arguments: &Value) -> Option<Cached> {
    lookup_at(policy, name, arguments, Instant::now())
}

/// 与 [`lookup`] 同一份实现，"现在"作为参数收进来 —— 用例要能把钟往前拨
/// （TTL 最短的一档也有 10 秒，`thread::sleep` 不该出现在用例里）。
fn lookup_at(
    policy: &ToolPolicy,
    name: &str,
    arguments: &Value,
    now: Instant,
) -> Option<Cached> {
    let fresh = freshness(name)?;
    let probe = probe(policy, name, arguments, fresh)?;
    let mut guard = cache().lock().unwrap_or_else(|e| e.into_inner());
    let index = guard.iter().position(|entry| entry.key == probe.key)?;
    let age = now.saturating_duration_since(guard[index].stored);
    // 时间与 stat **两条都要过**（`Ttl` 档的 stamp 两边都是 `None`，所以一条比较就够）
    if age > fresh.ttl() || guard[index].stamp != probe.stamp {
        guard.remove(index);
        return None;
    }
    let entry = guard.remove(index)?;
    let hit = Cached {
        content: entry.content.clone(),
        payload: entry.payload.clone(),
        age,
        ttl: fresh.ttl(),
    };
    // LRU：命中的挪到队尾（淘汰从队首走）
    guard.push_back(entry);
    Some(hit)
}

/// 存一条（**只许在真的跑成功之后调**，且只对 [`freshness`] 认的工具）。
///
/// `preview` 不用传：命中时的摘要从**加了标注的正文**重新算
/// （[`crate::tools::tools::preview_of`] 本来就是"正文压平取前 N 个字符"，
/// 工具自己的 `preview` 也是这么来的）—— 少一个能跟正文漂移的副本。
pub fn store(
    policy: &ToolPolicy,
    name: &str,
    arguments: &Value,
    content: &str,
    payload: &Value,
) -> bool {
    let Some(fresh) = freshness(name) else {
        return false;
    };
    if content.chars().count() > MAX_CONTENT_CHARS {
        return false;
    }
    let Some(probe) = probe(policy, name, arguments, fresh) else {
        return false;
    };
    let mut guard = cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(index) = guard.iter().position(|entry| entry.key == probe.key) {
        guard.remove(index);
    }
    while guard.len() >= MAX_ENTRIES {
        guard.pop_front();
    }
    guard.push_back(Entry {
        key: probe.key,
        content: content.to_string(),
        payload: payload.clone(),
        stored: Instant::now(),
        stamp: probe.stamp,
    });
    true
}

// ---------------------------------------------------------------------------
//  内部
// ---------------------------------------------------------------------------

struct Entry {
    key: String,
    content: String,
    payload: Value,
    stored: Instant,
    /// `File` 档的校验值（`Ttl` 档恒为 `None`）
    stamp: Option<Stamp>,
}

/// 文件/目录的"被动过没有"指纹：`mtime` + 长度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    mtime: Option<SystemTime>,
    len: u64,
}

/// 一次调用的"用来查表的键 + 用来校验的新鲜度"。
struct Probe {
    key: String,
    stamp: Option<Stamp>,
}

fn cache() -> &'static Mutex<VecDeque<Entry>> {
    static CACHE: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// 算出一个调用的 [`Probe`]；`None` = 这一次不进缓存（参数缺了 / 路径越界 / 文件不在）。
///
/// ⚠ `File` 档**拿不到 `stat` 就当没缓存**（既不命中也不存）：那说明路径过不了
/// [`ToolPolicy::resolve_read`]，或者文件此刻不在 —— 两种情况下"用一份旧结果顶上"
/// 都是错的（前者是安全边界，后者会让模型看不到 `read_file` 该报的那个错）。
fn probe(policy: &ToolPolicy, name: &str, arguments: &Value, fresh: Freshness) -> Option<Probe> {
    let mut canonical = String::new();
    canonical_into(arguments, &mut canonical);
    let mut raw = format!("{name}\u{1}{canonical}");
    let mut stamp = None;
    if matches!(fresh, Freshness::File(_)) {
        let path = target_path(policy, name, arguments)?;
        // 路径也进指纹：`list_dir` 不带 `path` 时的默认目录取自写白名单，
        // 光看参数会把"两个不同目录"认成同一组参数
        raw.push('\u{1}');
        raw.push_str(&path.display().to_string());
        stamp = Some(stat_of(&path)?);
    }
    Some(Probe { key: format!("{name}\u{1}{}", fingerprint(&raw)), stamp })
}

/// `File` 档工具的目标路径（**一律过策略**，不许自己拼）。
fn target_path(policy: &ToolPolicy, name: &str, arguments: &Value) -> Option<PathBuf> {
    match name {
        "read_file" => {
            let raw = arguments.get("path").and_then(Value::as_str)?;
            policy.resolve_read(raw).ok()
        }
        // 与 `list_dir` 自己的默认值**同一处口径**：写白名单的第一个
        "list_dir" => {
            let raw = arguments
                .get("path")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| policy.write_roots.first().map(|p| p.display().to_string()))?;
            policy.resolve_read(&raw).ok()
        }
        _ => None,
    }
}

fn stat_of(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp { mtime: meta.modified().ok(), len: meta.len() })
}

/// 参数指纹（**规范化之后**再算）：对象键排序，所以 `{"a":1,"b":2}` 与
/// `{"b":2,"a":1}` 是同一组参数（模型两次拼 JSON 的键序经常不一样）。
///
/// 为什么存指纹而不是原串：`comfy_prune_workflow` 的参数里可能装着**整张节点图**
/// （几十 KB），96 条就是好几 MB。两遍不同盐的 SipHash（128 位）把碰撞概率压到可以忽略 ——
/// 而"碰撞"在这里的后果是"给模型一份张冠李戴的结果"，所以不省这一步。
///
/// 缓存是**进程内**的，所以 `DefaultHasher` 的跨进程稳定性不在考虑范围内。
fn fingerprint(text: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut first = std::collections::hash_map::DefaultHasher::new();
    "comfyhub-tool-cache-a".hash(&mut first);
    text.hash(&mut first);
    let mut second = std::collections::hash_map::DefaultHasher::new();
    "comfyhub-tool-cache-b".hash(&mut second);
    text.hash(&mut second);
    format!("{:016x}{:016x}", first.finish(), second.finish())
}

/// 规范化 JSON 文本（对象键排序、数组保序、其余原样）。
fn canonical_into(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String((*key).clone()).to_string());
                out.push(':');
                canonical_into(&map[*key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                canonical_into(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// "N 秒" / "M 分 SS 秒"（给用户与模型看的人话，不用 `Debug` 输出 `12.34s`）。
fn human_age(age: Duration) -> String {
    let seconds = age.as_secs();
    if seconds < 60 {
        format!("{seconds} 秒")
    } else {
        format!("{} 分 {:02} 秒", seconds / 60, seconds % 60)
    }
}

#[cfg(test)]
mod tests;

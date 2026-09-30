//! Danbooru 查询（**Rust 侧对照 Kotlin `DanbooruSearch.kt`**，O8）。
//!
//! # 为什么要有这条工具
//!
//! Anima / SD 这类 **Danbooru tag 模型**的提示词是有"正确写法"的：角色 tag 必须与
//! danbooru 上的**同一个 tag**（`shinku_(nte)` 而不是 `shinku` 或中文名），
//! 外观 / 服装 / 动作要写成 danbooru 认的那几个词。凭印象编 tag 的后果不是报错，
//! 是**出一张别人**（现场教训：把 `shinku` 当成 `iroi` 写了两版废图）。
//! LoRA 的触发词只覆盖"这条 LoRA 自己训出来的那部分"，它既不告诉你角色的**其他规范 tag**，
//! 也不告诉你 danbooru 上有没有**别名 / 换装版本**。
//!
//! # 三条实测事实（Kotlin 侧 2026-09-25 量到，Rust 侧照守）
//!
//! 1. **必须带 User-Agent**，而且**必须短**：本机实测同一个 URL、同一个代理只换 UA ——
//!    `ComfyHub/1.0 (+local)` → **200**，而描述型 / Mozilla 形状的 UA → **403 + Cloudflare 挑战页**。
//!    所以 UA 写死成常量并留 `COMFYHUB_DANBOORU_UA` 口子（被拦时用户自己能换，不必等发版）。
//! 2. **本机必须走代理**：`danbooru.donmai.us` 直连不稳（大量 connect timeout），
//!    经 `127.0.0.1:7890` 稳定 200 —— 走 [`crate::outbound::route`]（公网走代理、本机/内网直连）。
//! 3. **`wiki_pages/<tag>.json` 的 404 是正常情况**（这个 tag 没有 wiki 页，不是错误）：
//!    照旧把 tags / related 给出去，只在正文里说明"没有 wiki 页"。
//!
//! # 一次查询最多 4 个请求（有界）
//!
//! ① 精确 `name_matches=<tag>`（查这个 tag 本身，含改名 / 弃用标记）；
//! ② 模糊 `name_matches=*<q>*`（换装版本 / 同名角色的候选，也用来判断"你写的 tag 不存在"）；
//! ③ `wiki_pages/<tag>.json`（别名 `other_names` + 简介）；
//! ④ `related_tag.json?query=<tag>`（**同现 tag**：danbooru 上这个角色最常配的外观 / 服装 tag，
//!    按 frequency 排序 —— 这就是"规范 tag 栈"的来源）。
//! ③④ 只在 ② 里真的解析出一个角色 tag 时才发。**别在这里做"多轮追问式"查询**。
//!
//! # 形状 / 语义上几处"照抄不许漂移"的地方
//!
//! * `tags.json` 回的是**数组**，`wiki_pages` / `related_tag` 回的是**对象** ——
//!   一律当对象解析会在第一个请求上就报"返回了无法解析的内容"（看着像"danbooru 挂了"）。
//! * 选角色用 `maxByOrNull { postCount }` = **并列取第一个**；Rust 的 `max_by_key`
//!   并列取**最后一个**（`docs/pitfalls.md` 记过这个差异），所以这里显式实现"取第一个最大"。
//! * 404 / 429 / 5xx 与"没有这个 tag"必须分清：**绝不把"没查成"说成"没有这个角色"**。

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::tools::ToolFailure;

/// 默认站点（`DANBOORU_BASE_URL` 可覆盖，给本机假服务留的口子）。
pub const DEFAULT_BASE_URL: &str = "https://danbooru.donmai.us";

/// User-Agent：**必须带、必须短**（长得像脚本的反而被 Cloudflare 拦，见模块头）。
pub const USER_AGENT: &str = "ComfyHub/1.0 (+local)";

/// 覆盖 User-Agent 的环境变量（被 Cloudflare 拦时用户自己能换一个）。
pub const ENV_UA: &str = "COMFYHUB_DANBOORU_UA";

/// 覆盖站点地址的环境变量（本机假服务用；真用户不会设它）。
pub const ENV_BASE_URL: &str = "DANBOORU_BASE_URL";

pub const CONNECT_TIMEOUT_SECONDS: u64 = 5;
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 20;
pub const MAX_TIMEOUT_SECONDS: u64 = 60;

/// 一次最多回几条候选 tag（再多对"挑对那个 tag"没有帮助，只烧 token）。
pub const MAX_LIMIT: i64 = 20;
/// 同现 tag 最多回几条（20 条已覆盖"外观 + 服装 + 常见动作"）。
pub const MAX_RELATED: i64 = 50;
/// wiki 正文截断（wiki 里有大量 `!post #…` 的图例，正文本身很短）。
pub const MAX_WIKI_CHARS: usize = 1200;

/// 一次查询的结果：`text` 给模型看，`meta` 给界面与"写进角色卡"用。
#[derive(Debug)]
pub struct Outcome {
    pub text: String,
    pub meta: Value,
}

/// 一条 danbooru tag。
#[derive(Debug, Clone)]
struct Tag {
    name: String,
    category: Option<i64>,
    post_count: i64,
    deprecated: bool,
}

/// 一条同现 tag。
#[derive(Debug, Clone)]
struct RelatedTag {
    name: String,
    post_count: i64,
    frequency: f64,
}

/// danbooru 的 tag 分类码 → 人话。
///
/// 0 普通 / 1 画师 / 3 作品（版权）/ 4 角色 / 5 meta。**2 目前没有定义**，
/// 所以别写成 `0..5` 的数组下标（那会把 2 说成 copyright）。
pub fn category_name(code: Option<i64>) -> &'static str {
    match code {
        Some(0) => "general",
        Some(1) => "artist",
        Some(3) => "copyright",
        Some(4) => "character",
        Some(5) => "meta",
        _ => "unknown",
    }
}

/// 把用户 / 模型的写法归一成 danbooru 的 tag 写法。
///
/// 空白 → 下划线（danbooru 的 tag 不允许空格；`shinku (nte)` 与 `shinku_(nte)` 是同一个 tag）、
/// 全角括号 → 半角且前面**保证有一个下划线**（danbooru 的消歧写法是 `name_(work)`）、小写。
/// **不动下划线本身**（`shinku_(nte)` 里的下划线是 tag 的一部分）。
pub fn normalize_tag(raw: &str) -> String {
    // ① 小写 + 空白连成一串 → 一个下划线
    let mut out = String::new();
    let mut in_space = false;
    for ch in raw.trim().chars() {
        if ch.is_whitespace() {
            in_space = true;
            continue;
        }
        if in_space && !out.is_empty() {
            out.push('_');
        }
        in_space = false;
        out.push(ch.to_lowercase().next().unwrap_or(ch));
    }
    // ② 开括号：去掉前面已有的下划线，保证正好一个 `_(`
    let mut normalized = String::new();
    for ch in out.chars() {
        if ch == '(' || ch == '（' {
            while normalized.ends_with('_') {
                normalized.pop();
            }
            normalized.push_str("_(");
        } else if ch == ')' || ch == '）' {
            normalized.push(')');
        } else {
            normalized.push(ch);
        }
    }
    normalized.trim_matches('_').to_string()
}

/// 一次 danbooru 查询的客户端。
///
/// `base_url` 显式传（不读环境变量）—— 用例要能指向本机的假服务，
/// 而环境变量是**进程级**的、并行用例之间会互相污染。
pub struct DanbooruClient {
    base: String,
    user_agent: String,
    timeout: Duration,
}

impl DanbooruClient {
    pub fn new(base_url: &str, user_agent: &str, timeout_seconds: u64) -> Result<Self, ToolFailure> {
        let base = base_url.trim().trim_end_matches('/').to_string();
        if base.is_empty() {
            return Err(ToolFailure::new("DANBOORU_FAILED", "danbooru 地址是空的（这是本机的 bug，请如实告诉用户）"));
        }
        Ok(Self {
            base,
            user_agent: user_agent.to_string(),
            timeout: Duration::from_secs(timeout_seconds.clamp(1, MAX_TIMEOUT_SECONDS)),
        })
    }

    /// 按环境变量造一个（`DANBOORU_BASE_URL` / `COMFYHUB_DANBOORU_UA`）。
    pub fn from_env() -> Result<Self, ToolFailure> {
        let base = std::env::var(ENV_BASE_URL).unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        let ua = std::env::var(ENV_UA)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| USER_AGENT.to_string());
        Self::new(&base, &ua, DEFAULT_TIMEOUT_SECONDS)
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// 查一个角色 / tag：`text` 给模型，`meta` 给界面与角色卡。
    pub async fn lookup(&self, query: &str, limit: i64, related_limit: i64) -> Result<Outcome, ToolFailure> {
        let q = normalize_tag(query);
        if q.is_empty() {
            return Err(ToolFailure::new("INVALID_ARGUMENT", "查询词不能为空"));
        }
        let lim = limit.clamp(1, MAX_LIMIT);
        let rel_lim = related_limit.clamp(1, MAX_RELATED);

        let exact = self.fetch_tags(&q, false, lim).await?;
        // 模糊那一趟**总是要发**：换装版本（`shinku_(dawn_patrol)_(nte)`）只出现在模糊结果里，
        // 而"这个角色有哪些版本"正是"别自己编 tag"最需要的信息。
        let fuzzy = self.fetch_tags(&q, true, lim).await?;

        let mut all: Vec<Tag> = Vec::new();
        for tag in exact.iter().chain(fuzzy.iter()) {
            if !all.iter().any(|t| t.name == tag.name) {
                all.push(tag.clone());
            }
        }
        // ⚠ `maxByOrNull` 是**并列取第一个**（Rust 的 `max_by_key` 取最后一个）
        let character = all
            .iter()
            .filter(|t| t.category == Some(4))
            .fold(None::<&Tag>, |best, tag| match best {
                Some(current) if current.post_count >= tag.post_count => Some(current),
                _ => Some(tag),
            })
            .cloned();
        let variants: Vec<String> = all
            .iter()
            .filter(|t| t.category == Some(4) && Some(&t.name) != character.as_ref().map(|c| &c.name))
            .map(|t| t.name.clone())
            .collect();

        let mut wiki: Option<Value> = None;
        let mut related: Vec<RelatedTag> = Vec::new();
        if let Some(character) = character.as_ref() {
            wiki = self.fetch_wiki(&character.name).await?;
            related = self.fetch_related(&character.name, rel_lim).await?;
        }

        let mut meta = Map::new();
        meta.insert("query".into(), json!(query));
        meta.insert("normalizedTag".into(), json!(q));
        meta.insert("exact".into(), tags_json(&exact));
        if character.is_none() {
            meta.insert("candidates".into(), tags_json(&all.iter().take(lim as usize).cloned().collect::<Vec<_>>()));
        }
        if let Some(character) = character.as_ref() {
            let mut body = Map::new();
            body.insert("tag".into(), json!(character.name));
            body.insert("postCount".into(), json!(character.post_count));
            body.insert("deprecated".into(), json!(character.deprecated));
            if !variants.is_empty() {
                body.insert("variants".into(), json!(variants));
            }
            match wiki.as_ref().and_then(Value::as_object) {
                Some(wiki) => {
                    body.insert(
                        "otherNames".into(),
                        wiki.get("other_names").cloned().unwrap_or_else(|| json!([])),
                    );
                    body.insert(
                        "wiki".into(),
                        json!(wiki.get("body").and_then(Value::as_str).map(body_text).unwrap_or_default()),
                    );
                    body.insert("wikiMissing".into(), json!(false));
                }
                None => {
                    body.insert("wikiMissing".into(), json!(true));
                }
            }
            body.insert("relatedTags".into(), related_json(&related));
            meta.insert("character".into(), Value::Object(body));
            meta.insert(
                "wikiUrl".into(),
                json!(format!("{}/wiki_pages/{}", self.base, encode(&character.name))),
            );
        }
        meta.insert("verifiedAt".into(), json!(now_iso()));
        meta.insert("provider".into(), json!("danbooru"));

        Ok(Outcome {
            text: self.render(query, &q, &exact, &all, character.as_ref(), &variants, wiki.as_ref(), &related),
            meta: Value::Object(meta),
        })
    }

    // -----------------------------------------------------------------------
    //  正文（给模型看的那一份）
    // -----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn render(
        &self,
        query: &str,
        q: &str,
        exact: &[Tag],
        all: &[Tag],
        character: Option<&Tag>,
        variants: &[String],
        wiki: Option<&Value>,
        related: &[RelatedTag],
    ) -> String {
        let mut out = String::new();
        out.push_str(&format!("danbooru 查询：{query}（归一成 `{q}`）\n"));
        out.push_str("分类码：0=general 1=artist 3=copyright 4=character 5=meta\n");

        if exact.is_empty() {
            out.push_str(&format!("· 精确 tag `{q}` **不存在**（danbooru 上没这个 tag）——别照这个写法出图。\n"));
        } else {
            let listed: Vec<String> = exact
                .iter()
                .map(|t| format!("`{}`（{}，{} 张）", t.name, category_name(t.category), t.post_count))
                .collect();
            out.push_str(&format!("· 精确命中：{}\n", listed.join("、")));
        }
        let others: Vec<&Tag> = all.iter().filter(|t| !exact.iter().any(|e| e.name == t.name)).collect();
        if !others.is_empty() {
            out.push_str("· 名字相近的 tag（可能是同一角色的换装版本 / 同名角色）：\n");
            for tag in others.iter().take(12) {
                out.push_str(&format!(
                    "   - `{}`（{}，{} 张）\n",
                    tag.name,
                    category_name(tag.category),
                    tag.post_count
                ));
            }
        }

        let Some(character) = character else {
            out.push('\n');
            out.push_str("**没有解析出角色 tag**：上面没有 category=4 的命中。\n");
            out.push_str(
                "下一步：拿一个真正的角色 tag 再查一次（`角色名_(作品名)` 那种），\
                 或者问用户这个角色出自哪部作品。\n",
            );
            return out;
        };

        out.push('\n');
        out.push_str(&format!(
            "★ 角色 tag：`{}`（{} 张{}）\n",
            character.name,
            character.post_count,
            if character.deprecated { "，**已弃用**" } else { "" }
        ));
        let other_names: Vec<String> = wiki
            .and_then(|w| w.get("other_names"))
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(|n| n.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        if !other_names.is_empty() {
            out.push_str(&format!("· 别名（danbooru 记的 other names）：{}\n", other_names.join("、")));
        }
        match wiki.and_then(|w| w.get("body")).and_then(Value::as_str) {
            Some(body) if !body.trim().is_empty() => {
                out.push_str("· wiki 正文（截断）：\n```\n");
                out.push_str(&body_text(body));
                out.push_str("\n```\n");
            }
            _ => {
                out.push_str("· wiki：这个 tag 没有 wiki 页（正常情况，不影响 tag 本身可用）。\n");
            }
        }
        if !variants.is_empty() {
            out.push_str(&format!(
                "· 同角色的**换装 / 其他版本 tag**（要换衣服就换这个 tag，别自己编）：{}\n",
                variants.join("、")
            ));
        }
        if !related.is_empty() {
            out.push_str(
                "· 同现 tag（danbooru 上这个角色最常配的 tag，按频率降序）——**这就是规范 tag 栈的来源**：\n",
            );
            let names: Vec<&str> = related.iter().map(|t| t.name.as_str()).collect();
            out.push_str(&format!("   {}\n", names.join(" ")));
        } else {
            out.push_str("· 同现 tag：没拿到（该 tag 的图太少或接口不可用）——这时才凭 LoRA 触发词 + 常识写。\n");
        }
        out.push('\n');
        out.push_str(&format!(
            "写角色卡时：`characterTag` = `{}`，`aliases` = 上面的别名，\
             `tags` = 上面挑出来的同现 tag（外观/服装/动作那几类），\
             `source` = `{}/wiki_pages/{}`。\n",
            character.name,
            self.base,
            encode(&character.name)
        ));
        out
    }

    // -----------------------------------------------------------------------
    //  出网
    // -----------------------------------------------------------------------

    async fn fetch_tags(&self, query: &str, fuzzy: bool, limit: i64) -> Result<Vec<Tag>, ToolFailure> {
        let pattern = if fuzzy { format!("*{query}*") } else { query.to_string() };
        let url = format!(
            "{}/tags.json?search%5Bname_matches%5D={}&search%5Border%5D=count&limit={limit}",
            self.base,
            encode(&pattern)
        );
        // ⚠ `tags.json` 回的是**数组**（`wiki_pages` / `related_tag` 回的是对象）
        let Some(body) = self.get_json(&url, false).await? else { return Ok(Vec::new()) };
        let Some(arr) = body.as_array() else { return Ok(Vec::new()) };
        Ok(arr
            .iter()
            .filter_map(|item| {
                let name = item.get("name").and_then(Value::as_str)?.to_string();
                Some(Tag {
                    name,
                    category: item.get("category").and_then(Value::as_i64),
                    post_count: item.get("post_count").and_then(Value::as_i64).unwrap_or(0),
                    deprecated: item.get("is_deprecated").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect())
    }

    /// wiki 页：**404 是正常情况**（没有 wiki 页），回 `None` 而不是报错。
    async fn fetch_wiki(&self, tag: &str) -> Result<Option<Value>, ToolFailure> {
        let url = format!("{}/wiki_pages/{}.json", self.base, encode(tag));
        Ok(self.get_json(&url, true).await?.filter(Value::is_object))
    }

    async fn fetch_related(&self, tag: &str, limit: i64) -> Result<Vec<RelatedTag>, ToolFailure> {
        let url = format!(
            "{}/related_tag.json?query={}&category=General&limit={limit}",
            self.base,
            encode(tag)
        );
        // 404 也当"没有同现 tag"：这一步是**增值信息**，它挂了不该把已经解析出来的
        // 角色 tag / 别名一起丢掉（正文里会如实写"同现 tag 没拿到"）。
        // 429 / 5xx 仍然抛错 —— 那是"没查成"，与"没有"是两回事。
        let Some(body) = self.get_json(&url, true).await? else { return Ok(Vec::new()) };
        let Some(arr) = body.get("related_tags").and_then(Value::as_array) else { return Ok(Vec::new()) };
        Ok(arr
            .iter()
            .filter_map(|item| {
                let tag_obj = item.get("tag")?;
                let name = tag_obj.get("name").and_then(Value::as_str)?.to_string();
                Some(RelatedTag {
                    name,
                    post_count: tag_obj.get("post_count").and_then(Value::as_i64).unwrap_or(0),
                    frequency: item.get("frequency").and_then(Value::as_f64).unwrap_or(0.0),
                })
            })
            .collect())
    }

    /// 发一次 GET 并解析 JSON（**形状由调用方判**：`tags.json` 是数组、其余是对象）。
    ///
    /// 404 在 `not_found_ok` 下回 `None`；其余非 2xx 一律 [`ToolFailure`]（带稳定码），
    /// **绝不把"没查成"说成"没有这个 tag"**。
    async fn get_json(&self, url: &str, not_found_ok: bool) -> Result<Option<Value>, ToolFailure> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|_| ToolFailure::new("DANBOORU_FAILED", "查询地址拼装失败（这是本机的 bug，请如实告诉用户）"))?;
        let host = parsed.host_str().unwrap_or("danbooru.donmai.us").to_string();
        let route = crate::outbound::route(&host).describe();

        let client = self.http_client(&host)?;
        let response = client
            .get(parsed)
            .header("Accept", "application/json")
            .header("User-Agent", &self.user_agent)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() || e.is_timeout() {
                    ToolFailure::new(
                        "DANBOORU_TIMEOUT",
                        format!(
                            "连不上 {host}（{}；本次出网路径「{route}」）。\
                             danbooru 在国内通常**必须走代理** —— 如实告诉用户，别反复重试，\
                             也不要凭印象编 tag。",
                            if e.is_connect() { "连接被拒 / 超时" } else { "读超时" }
                        ),
                    )
                } else {
                    ToolFailure::new("DANBOORU_FAILED", format!("连不上 {host}（{e}；路径「{route}」）"))
                }
            })?;

        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        if status == 404 && not_found_ok {
            return Ok(None);
        }
        if !(200..300).contains(&status) {
            // Cloudflare 的 managed challenge 长这样：403 + 一整页 HTML（标题 "Just a moment..."）。
            // 必须**单独认出来**：把它报成"UA 被拒"或"没有这个 tag"都会把用户带偏
            // （前者让人以为换个 UA 就好，后者直接让人以为角色不存在）。
            if status == 403 && looks_like_cloudflare_challenge(&text) {
                return Err(ToolFailure::new(
                    "DANBOORU_CLOUDFLARE",
                    format!(
                        "danbooru 前面的 Cloudflare 拦住了这次请求（403 + 挑战页，不是 danbooru 自己回的）。\
                         本机实测**短 UA 才过得去**（当前用的是 `{}`）。\
                         换一个 User-Agent（环境变量 {ENV_UA}）再试，或者让用户自己在浏览器里查这个 tag —— \
                         **不要**把这个结果说成「danbooru 上没有这个角色」。",
                        self.user_agent
                    ),
                ));
            }
            let (code, hint) = match status {
                403 => (
                    "DANBOORU_UA_REJECTED",
                    format!(
                        "danbooru 拒绝了这次请求（403）：它的机器人政策要求带说明身份的 User-Agent\
                         （当前用的是 `{}`）",
                        self.user_agent
                    ),
                ),
                404 => ("DANBOORU_NOT_FOUND", "danbooru 上没有这个 tag（404）——**别照这个写法出图**".to_string()),
                429 => ("DANBOORU_RATE_LIMIT", "danbooru 限流了（429）；等一下再查，不要连着查".to_string()),
                500..=599 => ("DANBOORU_FAILED", format!("danbooru 暂时不可用（{status}）")),
                _ => ("DANBOORU_FAILED", format!("danbooru 返回 {status}")),
            };
            return Err(ToolFailure::new(code, hint));
        }
        match serde_json::from_str::<Value>(&text) {
            Ok(value) => Ok(Some(value)),
            Err(_) => {
                // 2xx 也可能是挑战页（Cloudflare 有时用 200 送挑战页）——同样别报成"内容坏了"
                if looks_like_cloudflare_challenge(&text) {
                    return Err(ToolFailure::new(
                        "DANBOORU_CLOUDFLARE",
                        format!(
                            "danbooru 前面的 Cloudflare 送回了挑战页（HTTP {status} 但不是 JSON）。\
                             可用环境变量 {ENV_UA} 换一个 User-Agent 再试；**不要**说成「没有这个角色」。"
                        ),
                    ));
                }
                Err(ToolFailure::new(
                    "DANBOORU_FAILED",
                    format!("danbooru 返回了无法解析的内容（status={status}）"),
                ))
            }
        }
    }

    /// 与 [`crate::upstream::client_for`] 同一条代理纪律（公网走代理、本机/内网直连），
    /// 只是超时按 danbooru 的口径（连接 5 秒 / 整次 20 秒）。
    fn http_client(&self, host: &str) -> Result<reqwest::Client, ToolFailure> {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECONDS))
            .timeout(self.timeout)
            .redirect(reqwest::redirect::Policy::none());
        match crate::outbound::route(host) {
            crate::outbound::Route::Proxy(addr) => {
                let proxy = reqwest::Proxy::all(format!("http://{addr}"))
                    .map_err(|e| ToolFailure::new("DANBOORU_FAILED", format!("代理地址不合法：{e}")))?;
                builder = builder.proxy(proxy);
            }
            crate::outbound::Route::Direct => {
                // 环境变量里的 `HTTPS_PROXY` reqwest **是看的** —— 显式覆盖掉，
                // 否则本机/内网目标会被它接走。
                builder = builder.no_proxy();
            }
        }
        builder
            .build()
            .map_err(|e| ToolFailure::new("DANBOORU_FAILED", format!("HTTP 客户端创建失败：{e}")))
    }
}

/// Cloudflare 挑战页的判据：HTML 里的那几个标记（比"看 Content-Type"更耐改版）。
fn looks_like_cloudflare_challenge(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("just a moment")
        || body.contains("challenge-platform")
        || body.contains("Enable JavaScript and cookies to continue")
}

/// wiki 正文截断（`\r\n` 归一 + 超长补一句，**不静默砍掉**）。
fn body_text(body: &str) -> String {
    let cleaned = body.replace("\r\n", "\n").trim().to_string();
    let chars: Vec<char> = cleaned.chars().collect();
    if chars.len() <= MAX_WIKI_CHARS {
        cleaned
    } else {
        let head: String = chars[..MAX_WIKI_CHARS].iter().collect();
        format!("{head}\n…（wiki 过长已截断）")
    }
}

/// 查询参数编码（**Java `URLEncoder.encode` 的口径**：保留字母数字与 `-_.*`，
/// 空格变 `%20`）。
///
/// ⚠ `*` 必须**保留**：模糊那一路要发 `name_matches=*shinku*`，
/// 把通配符编码成 `%2A` 就不是通配了（表现是"模糊搜索一条都没有"）。
fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '*') {
            out.push(ch);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn tags_json(tags: &[Tag]) -> Value {
    Value::Array(
        tags.iter()
            .map(|tag| {
                let mut out = Map::new();
                out.insert("name".into(), json!(tag.name));
                out.insert("category".into(), json!(category_name(tag.category)));
                out.insert("postCount".into(), json!(tag.post_count));
                if tag.deprecated {
                    out.insert("deprecated".into(), json!(true));
                }
                Value::Object(out)
            })
            .collect(),
    )
}

fn related_json(tags: &[RelatedTag]) -> Value {
    Value::Array(
        tags.iter()
            .map(|tag| {
                json!({ "name": tag.name, "postCount": tag.post_count, "frequency": tag.frequency })
            })
            .collect(),
    )
}

/// 现在（ISO-8601，`Instant.toString()` 的口径 —— 与 `verifiedAt` 的可追溯性同一条）。
pub fn now_iso() -> String {
    crate::iso_time::from_system_time(std::time::SystemTime::now())
}

// ---------------------------------------------------------------------------
//  工具入口
// ---------------------------------------------------------------------------

/// `danbooru_lookup`：查 tag 的规范写法。
///
/// ⚠ **调用方要把"这一轮真查过哪个 tag"记进 `ToolSession.danbooru_verified`**
/// （`save_character_card` 靠它决定写不写核对时间）—— 那一步在 `tools::execute` 里做，
/// 因为"查到的角色 tag"要从这里的 `meta` 里读回来。
pub async fn tool_lookup(arguments: &Value) -> Result<crate::tools::ToolOutcome, ToolFailure> {
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .ok_or_else(|| ToolFailure::new("INVALID_ARGUMENT", "缺少参数 query"))?;
    let limit = crate::tools::int_arg(arguments, "limit").unwrap_or(12);
    let related_limit = crate::tools::int_arg(arguments, "relatedLimit").unwrap_or(20);
    let client = DanbooruClient::from_env()?;
    let outcome = client.lookup(query, limit, related_limit).await?;
    let payload = outcome.meta.clone();
    // ⚠ 这一行是**补上的**（P4 第 64 步）：Kotlin 的 `danbooru_lookup` 处理体写的是
    // `ToolOutput(untrustedBanner("danbooru 页面", query) + "\n\n" + outcome.text, …)`，
    // 而这一批之前 Rust 侧把那段**数据区出处标记**漏掉了 —— 于是 danbooru 的正文进了
    // 上下文却没有"这是资料"的标记，而系统提示第 4 条那套纪律是**靠工具结果里这一行**
    // 才落到实处的（RSK-004）。文案、`kind` 与"只在有查询词时带第二行"都照抄 Kotlin。
    let text = format!(
        "{}\n\n{}",
        crate::tools::untrusted_banner("danbooru 页面", Some(query)),
        outcome.text
    );
    Ok(crate::tools::ToolOutcome::new(text, payload))
}

#[cfg(test)]
mod tests;

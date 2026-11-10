//! 产物**集合**的成组判定：哪几张图属于"同一批、只做了一点点微调"。
//!
//! 设计依据（逐条固定，不得随意修改）：
//!
//! 1. **判据主要来自提示词** —— "它决定了产物的走向"。
//!    所以 [`group_key`] 里**只有 kind + 正提示词 + 负提示词**，别的什么都不进。
//! 2. **自然语言和权重表达说的是同一样东西** —— `(masterpiece:1.2), 1girl`
//!    与 `masterpiece, (1girl:1.1)` 必须判成同一组（[`normalize`] 负责削平）。
//!    ⚠ 但**只削"带数字权重的括号"**：裸括号 `(x)` / `((x))` 在 danbooru / anima 里是
//!    **加强权重**、是真实语义，剥掉就等于改了提示词 —— 所以原样留着。
//! 3. **种子不同当然也算同一组** —— 所以 `seed` **根本不在输入里**。
//! 4. **`steps` / `cfg` 不是影响产物内容的主要因素** —— 同理不进输入。
//!    一句话：**提示词相同就都算同一组**。
//!
//! # 为什么判定要"只认提示词、不认参数"
//!
//! 这条不是偷懒，是**唯一自洽**的选择：只要拉开一点参数阈值（steps 差多少算新组、
//! cfg 差多少算新组），就会出现"同一句提示词、同一份工作流，只因为调了 cfg 就被拆成两组"，
//! 即"step/cfg 不是主要因素"。反过来把参数全部忽略，判定会退化为
//! **一个纯函数、只看文本** —— 可解释、可回填、可测。
//!
//! # 代价（写在这里，别当没发生）
//!
//! * **同模板不同采样器的图会被并成一组**（`euler a` 与 `dpmpp_2m` 出的图确实会不一样）。
//!   这是刻意接受的：语义为"持续调整同一张图"，而不是"参数完全一致"。
//! * `extra_params`（自由 JSON）同样不进输入 —— 它没有稳定语义，进去只会让判定飘。
//! * **孤儿产物（没关联提示词）不进组**（[`Assigned::Orphan`]）。2026-10-11 在真库上量出来
//!   才定的：那次回填把 21 张互不相干的手动导入图按"都是 IMAGE"并成了一组。
//!   判据是提示词，没有提示词就没有"走向"可判 —— 宁可不分组。
//!
//! # 两道防线
//!
//! * [`normalize`] 只做"表述削平"（权重/大小写/空白/顺序），**不做任何近似匹配** ——
//!   削平之后还必须**逐元素完全相等**。所以不会有"两个八竿子打不着的提示词被模糊匹配到一组"。
//! * [`group_key`] 是**纯函数**：同一份输入永远同一个 key。入库时算得出、回填时算得出，
//!   两者必须一致 —— 这也是"回填与入库共用同一份实现"那条纪律的落点。
//!
//! # 拼接 key 的问题（避免重复出现）
//!
//! 归一化后的片段**可能含 ASCII 控制字符**（提示词中可能粘贴 `\u{1}`、`\r`），
//! 所以**不能**拿控制字符当分隔符 —— 那样 `["a", "b\u{1}c"]` 与 `["a\u{1}b", "c"]`
//! 会拼出同一个 key（**造出假分组，而且不报错**）。
//! 现在的写法是**长度前缀**（`len ⊗ "+"'len2 0x1F 片段 ⊗ ...`），消除歧义且不用防转义。

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};

use crate::library::repo::WriteError;
use crate::web::models::{MediaDto, PromptDto};

/// 判定规则的版本号。
///
/// **改判定规则（归一化方式 / key 的组成）必须 +1** —— 回填任务据此重新分一遍，
/// 否则老库停在旧规则的分组上，而实测出来的组与算法算出来的组会**长期不一致**（且不报错）。
pub const RULE_VERSION: i64 = 1;

/// 段分隔符（拼接用，不是提示词分隔符）。
const FIELD_SEP: char = '\u{1F}';
/// 空文本的占位（跟"有一个空片段"区分开）。
const EMPTY: &str = "\u{0}empty";

/// 成组判定的输入：**只有三个字段**。
///
/// 故意做成"少字段"的结构体：以后有人想往里加 `steps`、`cfg`、`seed`，
/// 会先看到这段注释与 §1.4 的用例。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub kind: String,
    pub positive: String,
    pub negative: Option<String>,
}

impl Endpoint {
    pub fn new(
        kind: &str,
        positive: impl Into<String>,
        negative: Option<impl Into<String>>,
    ) -> Self {
        Self {
            kind: kind.trim().to_uppercase(),
            positive: positive.into(),
            negative: negative.map(Into::into),
        }
    }
}

impl From<&PromptDto> for Endpoint {
    fn from(p: &PromptDto) -> Self {
        Self::new(
            &p.kind,
            p.positive_prompt.clone(),
            p.negative_prompt.clone(),
        )
    }
}

impl From<&MediaDto> for Endpoint {
    fn from(m: &MediaDto) -> Self {
        // 产物的正提示词来自它关联的提示词；没关联就是空。
        // `prompt_positive` 为 None 时不能套用 `Endpoint::new` 的 `Option<impl Into<String>>`
        // （类型推断不出来），所以直接把负提示词也算好再构造。
        let negative = m.prompt_negative.clone().filter(|s| !s.trim().is_empty());
        let mut ep = Self::new(
            &m.kind,
            m.prompt_positive.clone().unwrap_or_default(),
            negative,
        );
        ep.kind = m.kind.trim().to_uppercase();
        ep
    }
}

/// 归一化：把"同一件事的不同写法"削平。
///
/// 步骤（顺序有意义，别调）：
/// 1. 按 `,` 与换行切段 —— **转义的 `\,` 不算分隔符**；
/// 2. **剥权重语法**：`(x:1.2)` → `x`、`((x:1.2):1.1)` → `x`（含嵌套）；
/// 3. 解转义 `\(` `\)` `\,`，去掉其他反斜杠；
/// 4. 连续空白压成一个、去首尾空白、**大小写不敏感**；
/// 5. **排序 + 去重**。
///
/// ⚠ **只剥"带数字权重的括号"**。裸括号（`(x)` / `((x))`）在 danbooru / anima 语法里
/// 是**加强权重**、是真实语义，剥掉就等于改了内容 —— 所以原样留着。
/// 这也是"认不出来就绝不猜"那条纪律：`(x:abc)` 里的 `:` 不是权重，括号就不能动。
pub fn normalize(text: &str) -> Vec<String> {
    let mut out: Vec<String> = split_segments(text)
        .iter()
        .map(|seg| normalize_segment(seg))
        .filter(|s| !s.is_empty())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// 按 `,` 与换行切段，但**尊重转义**：`a\,b` 是一段，不是两段。
///
/// 为什么不能先 `split(',')` 再解转义：那样 `a\,b` 会在切段时就被劈开，
/// 解转义之后变成两段 `a` / `b`，与真实的两段提示词 `a, b` 撞成同一个 key。
fn split_segments(text: &str) -> Vec<String> {
    let mut segs: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            // 转义序列整对搬进当前段（含反斜杠），交给 `unescape` 处理
            '\\' => {
                cur.push('\\');
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            }
            ',' | '\n' | '\r' => {
                segs.push(std::mem::take(&mut cur));
            }
            _ => cur.push(ch),
        }
    }
    segs.push(cur);
    segs
}

fn normalize_segment(seg: &str) -> String {
    let stripped = strip_weights(seg.trim());
    let unescaped = unescape(&stripped);
    let mut out = String::with_capacity(unescaped.len());
    let mut prev_space = false;
    for ch in unescaped.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_lowercase()
}

/// 剥掉**带数字权重的**括号，然后递归（`((x:1.2):1.1)` → `x`）。
///
/// **认不出来就原样返回 —— 绝不猜**：
/// * `(x)` / `((x))` 是 danbooru 的加强权重语法，**不是**排版噪音，剥掉就是改内容；
/// * `(x:abc)` 的 `:` 不是权重（tag 里的冒号很常见），同理不动。
fn strip_weights(mut s: &str) -> String {
    loop {
        let t = s.trim();
        let Some(inner) = t.strip_prefix('(').and_then(|rest| rest.strip_suffix(')')) else {
            return t.to_string();
        };
        // 括号必须真的包住整段：`(a)+(b)` 的 `strip_prefix/suffix` 也会命中，
        // 所以要在**顶层**重新数一遍括号，确认第一个 `(` 与最后那个 `)` 是一对。
        if !encloses_all(inner) {
            return t.to_string();
        }
        let inner = inner.trim();
        // 只有"顶层最后那个冒号后面是纯数字"才算权重；否则连括号一起留着。
        let Some((head, tail)) = last_colon(inner) else {
            return t.to_string();
        };
        if !is_weight_number(tail.trim()) || head.trim().is_empty() {
            return t.to_string();
        }
        s = head.trim();
    }
}

/// `inner`（已去掉最外层括号）里，括号是否在顶层闭合。
///
/// `x` → true；`a)+(b` → false（这其实是 `(a)+(b)` 被 `strip_*` 切出来的东西）。
fn encloses_all(inner: &str) -> bool {
    let mut depth = 0i32;
    let mut chars = inner.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                chars.next();
            }
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

/// 找**顶层**的最后一个 `:`（`(x:1.2)` 里那个；`((a:b):1.2)` 要的是外层那个）。
fn last_colon(s: &str) -> Option<(&str, &str)> {
    let mut depth = 0i32;
    let mut found: Option<usize> = None;
    let mut chars = s.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        match ch {
            '\\' => {
                chars.next();
            }
            '(' => depth += 1,
            ')' => depth -= 1,
            ':' if depth == 0 => found = Some(i),
            _ => {}
        }
    }
    found.map(|i| (&s[..i], &s[i + 1..]))
}

/// 权重是不是纯数字（`1.2` / `0.8` / `1`）。
///
/// **只认十进制数字与一个小数点**：`1.2e3`、`nan`、`inf`、`abc` 一律不认
/// —— 提示词里的 `:` 更多时候是别的东西（`girl:1.2` 是 tag 不是权重）。
fn is_weight_number(s: &str) -> bool {
    if s.is_empty() || s.starts_with('-') {
        return false;
    }
    let mut dots = 0;
    let mut digits = 0;
    for ch in s.chars() {
        match ch {
            '.' => {
                dots += 1;
                if dots > 1 {
                    return false;
                }
            }
            c if c.is_ascii_digit() => digits += 1,
            _ => return false,
        }
    }
    digits > 0
}

/// 解转义：`\(` `\)` `\,` `\\` 还原成字符本身；其他反斜杠**直接丢掉**。
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('(') => out.push('('),
            Some(')') => out.push(')'),
            Some(',') => out.push(','),
            Some('\\') => out.push('\\'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// 把归一化后的片段拼成一个**无歧义**的串（长度前缀，见文件头"拼接 key 的问题"）。
fn join_fragments(frags: &[String]) -> String {
    if frags.is_empty() {
        return EMPTY.to_string();
    }
    let mut out = String::new();
    for (i, f) in frags.iter().enumerate() {
        if i > 0 {
            out.push(FIELD_SEP);
        }
        out.push_str(&f.chars().count().to_string());
        out.push('\u{2}');
        out.push_str(f);
    }
    out
}

/// 成组判定用的 key：**同一份输入永远同一个 key**（入库与回填必须一致）。
///
/// 组成：`kind ⊗ 归一化正提示词 ⊗ 归一化负提示词`。
/// `None` 与 `Some("")` **归一化后是同一个东西**（两者都表示"没有负面提示词"）。
pub fn group_key(ep: &Endpoint) -> String {
    let pos = join_fragments(&normalize(&ep.positive));
    let neg = match ep.negative.as_deref() {
        Some(t) => join_fragments(&normalize(t)),
        None => EMPTY.to_string(),
    };
    format!(
        "{}{sep}{}{sep}{}",
        ep.kind.trim().to_uppercase(),
        pos,
        neg,
        sep = FIELD_SEP
    )
}

/// 两个端点是不是同一组。等价于"两组 key 相等"，但省掉一次拼接。
pub fn same_group(a: &Endpoint, b: &Endpoint) -> bool {
    a.kind.trim().eq_ignore_ascii_case(b.kind.trim())
        && normalize(&a.positive) == normalize(&b.positive)
        && normalize_negative(a.negative.as_deref()) == normalize_negative(b.negative.as_deref())
}

/// 负提示词的归一化：`None` 与空白串等价。
fn normalize_negative(text: Option<&str>) -> Vec<String> {
    text.map(normalize).unwrap_or_default()
}

// ---------------------------------------------------------------------------
//  落库那一半：算 key → 查/建集合 → 挂上去
// ---------------------------------------------------------------------------

/// 集合是怎么来的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// 算法分出来的。
    Auto,
    /// 手动拆开 / 合并产生的 —— **重算只处理 Auto**，不得把手动拆分的重新合并。
    Manual,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Auto => "auto",
            Origin::Manual => "manual",
        }
    }
}

/// 一条产物"算不算进组"的结论。
///
/// ⚠ **孤儿产物（没关联提示词）不进组** —— 这条是 2026-10-11 在真库上量出来才定的：
/// 那次回填把 **21 张互不相干的手动导入图**（全都没有提示词）按"都是 IMAGE"并成了一组。
/// 这不属于"同批微调"，浏览 21 张无关图片没有意义。
///
/// 判据应为**提示词**（它决定产物的走向）——
/// 没有提示词就没有"走向"可判，所以**宁可不分组**：详情页箭头禁用，退化成普通格子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assigned {
    /// 进了某个集合。
    Grouped(i64),
    /// 没关联提示词 → 刻意不分组。
    Orphan,
}

/// "这条产物能不能进组"的唯一判据：能进就给它的成组输入，不能进就 `None`。
///
/// 两档都算"不进组"：
/// 1. **没关联提示词**（`prompt_id` 为空）—— 见 [`Assigned`] 的注释；
/// 2. **关联了提示词，但正负正文归一化后都是空的** —— 它同样没有"走向"可判。
///
/// ⚠ 第 2 档是 2026-10-18 补的。少了它，所有"提示词解析不出来"的运行（自建工作流、
/// 非标准文本节点、特殊图生图链路）会**全部并进同一个 `IMAGE ⊗ ∅` 集合** ——
/// 即"生成的内容被自动分类到错误的集合"。捕获入库时每次运行都会插入一条
/// prompts 行（`capture_ingest.rs` 的 `create_captured_prompt`），正文原样写入、不判空，
/// 所以这一档在真机上很常见，而不是理论情况。
///
/// 判据用的是与 [`group_key`] **同一个** [`normalize`]：`"   "`、`",,,"`、只有换行
/// 都算空 —— 否则"看起来空、算起来不空"会让同一件事有两个答案。
fn endpoint_for(
    kind: &str,
    prompt_id: Option<i64>,
    positive: Option<String>,
    negative: Option<String>,
) -> Option<Endpoint> {
    prompt_id?;
    let ep = Endpoint::new(
        kind,
        positive.unwrap_or_default(),
        negative.filter(|s| !s.trim().is_empty()),
    );
    if normalize(&ep.positive).is_empty() && normalize_negative(ep.negative.as_deref()).is_empty() {
        return None;
    }
    Some(ep)
}

/// 读一条产物的成组输入：`Ok(None)` = 不进组（孤儿 / 正文空白），`Err(NotFound)` = 产物不在。
///
/// 没关联提示词**不是**"正提示词为空"，两者必须分开 —— 见 [`Assigned`] 的注释。
fn endpoint_of(conn: &Connection, media_id: i64) -> Result<Option<Endpoint>, WriteError> {
    let row = conn
        .query_row(
            "SELECT m.kind, m.prompt_id, p.positive_prompt, p.negative_prompt
               FROM media_assets m LEFT JOIN prompts p ON p.id = m.prompt_id
              WHERE m.id = ?1",
            params![media_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((kind, prompt_id, pos, neg)) = row else {
        return Err(WriteError::NotFound);
    };
    Ok(endpoint_for(&kind, prompt_id, pos, neg))
}

/// 按 `group_key` 查/建集合并把产物挂上去。
///
/// 匹配规则：**`group_key` 相等**，且**优先挂到最近建的 `auto` 集合**。
/// 优先 auto 的依据：拆分后留下 `manual` 集合，而新产物应进入"算法那一组"，
/// 而不是拆分出的那一份 —— 否则拆分一次后，所有新图都进入该小集合。
fn attach(conn: &Connection, media_id: i64, ep: &Endpoint) -> Result<i64, WriteError> {
    let key = group_key(ep);
    let existing: Option<(i64, i64)> = conn
        .query_row(
            "SELECT id, rule_version FROM media_collections
              WHERE group_key = ?1
              ORDER BY (origin = 'auto') DESC, id DESC
              LIMIT 1",
            params![key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let collection_id = match existing {
        Some((id, rule_version)) => {
            if rule_version != RULE_VERSION {
                // 不静默：规则变了要有人知道老集合还停在旧规则上
                eprintln!(
                    "[collection] 集合 #{id} 的 rule_version={rule_version} 与当前 {RULE_VERSION} 不一致：\
                     该重算一遍（`collection::backfill`）"
                );
            }
            id
        }
        None => {
            conn.execute(
                "INSERT INTO media_collections (group_key, kind, origin, rule_version)
                 VALUES (?1, ?2, ?3, ?4)",
                params![key, ep.kind, Origin::Auto.as_str(), RULE_VERSION],
            )?;
            conn.last_insert_rowid()
        }
    };

    conn.execute(
        "UPDATE media_assets SET collection_id = ?1 WHERE id = ?2",
        params![collection_id, media_id],
    )?;
    Ok(collection_id)
}

/// 给一条产物定组（入库那条路调的入口）。**幂等**。
///
/// 孤儿产物**不会**被建出一个集合来 —— 它只是留在"没有集合"的状态（`collection_id` 为 NULL）。
pub fn assign(conn: &Connection, media_id: i64) -> Result<Assigned, WriteError> {
    match endpoint_of(conn, media_id)? {
        Some(ep) => Ok(Assigned::Grouped(attach(conn, media_id, &ep)?)),
        None => Ok(Assigned::Orphan),
    }
}

/// 把"还没分组"的产物读出来，按**成组判据**分桶（同一份实现：回填与启动探针共用）。
///
/// 桶键就是 [`group_key`] —— 与入库时 [`attach`] 找"算法那一组"用的是同一个 key，
/// 所以"探针说有几组 / 几张"与"回填真的会建几组 / 挂几张"**必然一致**；
/// 两处各写一遍分桶就会漂（探针说有活、回填却什么都没干，反过来也一样）。
///
/// 三条口径（与 [`backfill`] 逐字一致）：
/// * 只取 `collection_id IS NULL AND collection_opt_out = 0` 的行；
/// * [`endpoint_for`] 判为"不进组"的（孤儿 / 正文空白）**不进任何桶**；
/// * 行序 `created_at ASC, id ASC`，桶序 = 桶里第一条的读入顺序 —— 这样 [`attach`]
///   建集合的先后与"逐行处理"的老实现完全一样，集合 `id` 因此保持稳定。
///
/// 返回 `(端点, 成员 id)`；桶内按读入顺序排，且**桶可以只有一条**（独苗，见 [`backfill`]）。
fn pending_buckets(conn: &Connection) -> Result<Vec<(Endpoint, Vec<i64>)>, WriteError> {
    // `(id, prompt_id, kind, 正面提示词, 负面提示词)`
    type PendingRow = (i64, Option<i64>, String, Option<String>, Option<String>);
    let rows: Vec<PendingRow> = {
        let mut stmt = conn.prepare(
            "SELECT m.id, m.prompt_id, m.kind, p.positive_prompt, p.negative_prompt
               FROM media_assets m LEFT JOIN prompts p ON p.id = m.prompt_id
              WHERE m.collection_id IS NULL AND m.collection_opt_out = 0
              ORDER BY m.created_at ASC, m.id ASC",
        )?;
        let mapped = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;
        mapped.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut buckets: Vec<(Endpoint, Vec<i64>)> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (id, prompt_id, kind, pos, neg) in rows {
        // 孤儿 / 正文空白：不分组，也不建集合（判据全在 [`endpoint_for`] 里，这里不重写一遍）
        let Some(ep) = endpoint_for(&kind, prompt_id, pos, neg) else {
            continue;
        };
        let key = group_key(&ep);
        match seen.get(&key) {
            Some(&i) => buckets[i].1.push(id),
            None => {
                seen.insert(key, buckets.len());
                buckets.push((ep, vec![id]));
            }
        }
    }
    Ok(buckets)
}

/// "这次启动有多少活儿"：**只数桶大小 >= 2 的**（独苗不建组，见 [`backfill`]）。
///
/// 返回 `(组数, 张数)`。它与 [`backfill`] 调的是**同一个** [`pending_buckets`]，
/// 所以这个数字就是"回填马上要做的事"，不是估算 —— 启动日志只报它，
/// 报 0 组就代表这一趟真的什么都不用干。
pub fn pending_grouped(conn: &Connection) -> Result<(usize, usize), WriteError> {
    let mut groups = 0usize;
    let mut items = 0usize;
    for (_, ids) in pending_buckets(conn)? {
        if ids.len() >= 2 {
            groups += 1;
            items += ids.len();
        }
    }
    Ok((groups, items))
}

/// 把**还没分组**的产物按同一套规则分一遍（存量归并 / 规则改了之后重算）。
///
/// 五条纪律：
/// 1. **只处理 `collection_id IS NULL` 的行** —— 已手动拆分/合并的（`origin='manual'`）
///    以及已分组的行一律不改动。拆分后被重新合并属于需避免的问题。
/// 2. **`collection_opt_out = 0`** —— "已明确移出集合"的行
///    `collection_id` 也是 NULL（那正是"移出"的写法），只判第 1 条等于**把刚拖出去的那张
///    又吸回来**，而且一个错都不报。这两条判据必须成对出现，少了任何一条都是静默失败。
/// 3. **只给"同款 >= 2 张"的桶建组**（2026-10-18 加）—— 一个组只有一张图没有意义
///    （见 [`prune_singleton_auto`]），因此独苗**不创建分组**，而不是"先创建再解散"：
///    后者每次启动都会做一次无效操作（创建后立即拆除），日志也会随之产生输出。桶由 [`pending_buckets`] 计算，
///    判据与入库 / 探针**共用同一份**（这里不重写一遍）。
/// 4. **按 `created_at` 升序** —— 同一组的成员顺序就是"调参的先后"，集合的
///    `id` 也据此稳定（详情页左右切换靠它）。
/// 5. 返回"这次分了多少**张**"（孤儿 / 已移出 / 正文空白 / 独苗都不算）。
///    调用方据此决定要不要写日志。
pub fn backfill(conn: &Connection) -> Result<usize, WriteError> {
    let mut done = 0usize;
    for (ep, ids) in pending_buckets(conn)? {
        // 独苗不建组（纪律 3）：它保持 `collection_id = NULL` 留在散图里。
        // ⚠ `collection_opt_out` **不写** —— 该行并非"主动移出"，同类新条目出现时
        // 回填该把它们并起来（写了就被第 2 条永久挡住）。
        if ids.len() < 2 {
            continue;
        }
        for id in ids {
            attach(conn, id, &ep)?;
            done += 1;
        }
    }
    Ok(done)
}

/// **显式「重新自动整理」**：把所有 `auto` 集合解散，再按当前规则全量重分一遍。
///
/// 与 [`backfill`] 的分工（这两件事**不能**合并成一个函数）：
/// * [`backfill`] 只补 `collection_id IS NULL` 的行 —— 已经分好组的**永远不会**被重算，
///   所以它是"老库补课"，不是"按现在的规则整理一遍"；
/// * 这是「重新自动整理」触发的全量重算：算法分组先全部解散，再按当前
///   [`RULE_VERSION`] 的规则重新分。
///
/// 五条纪律（顺序与判据都有理由，别调）：
/// 1. **先解散 `auto`、再 [`backfill`]** —— 反过来那些还没解散的老组在 `backfill` 眼里
///    是"已分组"，会**整组被跳过**：表现是"点了没反应"，而且一个错都不报。
/// 2. **只解散 `origin = 'auto'`** —— `manual` 集合（新建的、以及移入移出过的，
///    见 [`mark_manual`]）的成员 `collection_id` **不得改动**。已整理好的分组
///    被算法合回去是这条功能最气人的那种 bug。
/// 3. **`collection_opt_out` 原样保留** —— 解散只把 `collection_id` 置 NULL；已明确
///    移出的那几条仍然被 [`backfill`] 的 `AND collection_opt_out = 0` 挡在外面，
///    不会被吸回来（判据只有那一处，这里不重写一遍）。
/// 4. **收尾是"结构上不可能出现"的，所以这里没有收尾调用**（2026-10-18 改）——
///    ① 先解散 `auto` 是**DELETE 整个 `origin='auto'`**，所以此刻全库一条 auto 集合都没有；
///    ② [`backfill`] 只对"同款 >= 2 张"的桶建 auto 组，所以它新建的每一条 auto 集合
///    成员数都 >= 2；③ `attach` 只**加**成员、从不摘成员，而且它即便命中一条已存在的
///    同 key 集合，那条也绝不可能是 auto（① 已删光），只能是 `manual`（`origin` 不在
///    收尾范围内）。三条合起来：这一步之后再出现"单成员 auto 组"是不可能的。
///    ⚠ 以前这里调过一次 [`prune_singleton_auto`]——那会让"单成员 auto 组"这个
///    **不变式**被一次防御性收尾悄悄兜住；现在由 §13 的
///    `重新整理之后_单成员自动集合也不留` 用例正面盯着：规则一退化，它就红。
/// 5. 返回"这次分了多少条"，口径与 [`backfill`] **完全一致**（孤儿 / 已移出 / 正文空白 /
///    单成员集合不计入），调用方据此汇报数量。
pub fn regroup_auto(conn: &Connection) -> Result<usize, WriteError> {
    // ① 解散 auto：先把成员摘下来（`collection_id = NULL`），再删集合行。
    //
    // ⚠ 顺序不能反。虽然 `media_assets.collection_id` 是 `ON DELETE SET NULL`，但外键
    // 开关（`PRAGMA foreign_keys`）不是每个连接都保证打开 —— 显式置 NULL 才是"一定
    // 会被摘除"的路径。`collection_opt_out` **不写**，保持原有选择。
    conn.execute(
        "UPDATE media_assets SET collection_id = NULL
          WHERE collection_id IN (SELECT id FROM media_collections WHERE origin = ?1)",
        params![Origin::Auto.as_str()],
    )?;
    conn.execute(
        "DELETE FROM media_collections WHERE origin = ?1",
        params![Origin::Auto.as_str()],
    )?;

    // ② 再按当前规则分一遍。判据（孤儿 / 正文空白 / 已移出 / 独苗）全在 `backfill` 里，复用。
    //    它只对 >= 2 张的桶建组，所以这一步之后**结构上不可能**留下单成员 auto 组
    //    （推演见纪律 4）—— 这里因此没有第三次调用做收尾。
    let done = backfill(conn)?;
    Ok(done)
}

// ---------------------------------------------------------------------------
//  手动修改归属：移出 / 移进
// ---------------------------------------------------------------------------

/// 读一条产物当前所属集合（不在任何集合 = `None`）。产物不存在回 [`WriteError::NotFound`]。
fn collection_of(conn: &Connection, media_id: i64) -> Result<Option<i64>, WriteError> {
    conn.query_row(
        "SELECT collection_id FROM media_assets WHERE id = ?1",
        params![media_id],
        |row| row.get::<_, Option<i64>>(0),
    )
    .optional()?
    .ok_or(WriteError::NotFound)
}

/// **手动把一条产物移出集合**（"已手动处理，归属为手动"）。
///
/// ⚠ 必须同时写 `collection_opt_out = 1`。只把 `collection_id` 置成 NULL 的话，
/// **下次启动**的 [`backfill`] 会按提示词把它重新算回原来那一组 ——
/// 否则会表现为"已拖出但重启后恢复"，且**不报任何错误**。
///
/// 移出之后**源集合**要收尾（[`prune_singleton_auto`]，唯一实现）：
/// 搬空了 → 删掉（不留点不开的空格子）；是 `auto` 且只剩 1 个成员 → 一并解散；
/// 是 `manual` → **不改动**（手动创建的组，仅剩一张也保留）。
pub fn detach(conn: &Connection, media_id: i64) -> Result<(), WriteError> {
    // 同时作为 NotFound 的判据（产物不存在时不去改动集合）
    let from = collection_of(conn, media_id)?;
    let affected = conn.execute(
        "UPDATE media_assets SET collection_id = NULL, collection_opt_out = 1 WHERE id = ?1",
        params![media_id],
    )?;
    if affected == 0 {
        return Err(WriteError::NotFound);
    }
    // 本来就不在任何集合里 → 没有哪个集合的成员数会变，白扫一遍不值当。
    if from.is_some() {
        prune_singleton_auto(conn)?;
    }
    Ok(())
}

/// 给一个还没名字的集合起个默认名（`集合 1`、`集合 2`…）。
///
/// 仅在"创建组但未填名字"或"修改算法组使其转为手动"时使用。选取第一个未被
/// 占用的编号，避免界面出现同名组；手动填写的名称**原样保留**。
fn default_title(conn: &Connection) -> Result<String, WriteError> {
    let mut n = 1i64;
    loop {
        let candidate = format!("集合 {n}");
        let used: i64 = conn.query_row(
            "SELECT COUNT(*) FROM media_collections WHERE title = ?1",
            params![candidate],
            |row| row.get(0),
        )?;
        if used == 0 {
            return Ok(candidate);
        }
        n += 1;
    }
}

/// 移进之后要把 `origin` 从 `auto` 翻成 `manual`。
///
/// 为什么必须翻：`auto` 集合的 `group_key` 是"算法算出来的那一组"的指纹 ——
/// 加入一张提示词不同的图后，该指纹**不再描述其内容**，
/// 而 `attach` 还在按这个 key 往它里面吸新图（把不相关的图越吸越多）。
/// 已修改即归为手动；恢复算法分组只能通过"重新自动分组"。
///
/// 转为手动时**一并给出默认名称**：否则界面会显示为"未命名集合"，
/// 否则需要再手动修改一次（"新建集合要有默认编组名"）。
fn mark_manual(conn: &Connection, collection_id: i64) -> Result<(), WriteError> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT origin, title FROM media_collections WHERE id = ?1",
            params![collection_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((origin, title)) = row else {
        return Ok(());
    };
    if origin == Origin::Manual.as_str() {
        return Ok(());
    }
    let title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or(default_title(conn)?);
    conn.execute(
        "UPDATE media_collections
            SET origin = ?1, title = ?2,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
          WHERE id = ?3",
        params![Origin::Manual.as_str(), title, collection_id],
    )?;
    Ok(())
}

/// **手动把一条产物移进某个集合**。
///
/// 判据两条，都是"不猜、不将就"：
/// 1. 目标集合必须存在；
/// 2. **kind 必须相同** —— 图像集合中放入视频会被明确拒绝（
///    "手动集合不允许混图像和视频"）。拒绝要说人话，不许静默不动。
///
/// 移入会清除 `collection_opt_out`（归属已改变），源集合随后收尾
/// （[`prune_singleton_auto`]：空了删掉、`auto` 且只剩一张的一并解散），
/// 目标若是算法分组出来的则**就地转成手动**（见 [`mark_manual`]）。
pub fn move_into(conn: &Connection, media_id: i64, target: i64) -> Result<(), WriteError> {
    let media_kind: Option<String> = conn
        .query_row(
            "SELECT kind FROM media_assets WHERE id = ?1",
            params![media_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(media_kind) = media_kind else {
        return Err(WriteError::NotFound);
    };
    let target_kind: Option<String> = conn
        .query_row(
            "SELECT kind FROM media_collections WHERE id = ?1",
            params![target],
            |row| row.get(0),
        )
        .optional()?;
    let Some(target_kind) = target_kind else {
        return Err(WriteError::Validation(format!("集合 {target} 不存在")));
    };
    if !media_kind.eq_ignore_ascii_case(&target_kind) {
        return Err(WriteError::Validation(format!(
            "那个集合是 {target_kind}，装不下 {media_kind} 的产物"
        )));
    }

    let from = collection_of(conn, media_id)?;
    conn.execute(
        "UPDATE media_assets SET collection_id = ?1, collection_opt_out = 0 WHERE id = ?2",
        params![target, media_id],
    )?;
    // ⚠ 顺序有意义：**先翻手动、再收尾**。翻完之后目标集合（哪怕只剩 1 个成员）
    // 已经是 `manual`，下面的收尾绝不会把它当成"单成员 auto 组"解散掉 ——
    // 手动操作的结果不能被算法覆盖。
    mark_manual(conn, target)?;
    // 源集合收尾（唯一实现见 [`prune_singleton_auto`]）：搬空的删掉、
    // `auto` 且只剩 1 个成员的一并解散（"旧集合元素为 1 未自动解散"）。
    if from.is_some() {
        prune_singleton_auto(conn)?;
    }
    Ok(())
}

/// 选择器要的一份集合摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionSummary {
    pub id: i64,
    pub kind: String,
    /// `auto` / `manual`
    pub origin: String,
    pub title: Option<String>,
    /// 成员数
    pub size: i64,
    /// 选择器里给这一组显示一张封面（第一张成员的 media id）。
    pub cover_media_id: Option<i64>,
    /// 这张封面是不是 NSFW（选择器里要照样打码）。
    pub cover_nsfw: bool,
}

/// 列出所有集合（手动在前、最近建的在前），给"移到集合…"的选择器用。
pub fn list(conn: &Connection) -> Result<Vec<CollectionSummary>, WriteError> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.kind, c.origin, c.title,
                (SELECT COUNT(*) FROM media_assets m WHERE m.collection_id = c.id),
                (SELECT m.id FROM media_assets m WHERE m.collection_id = c.id
                  ORDER BY m.id ASC LIMIT 1),
                COALESCE((SELECT m.nsfw FROM media_assets m WHERE m.collection_id = c.id
                           ORDER BY m.id ASC LIMIT 1), 0)
           FROM media_collections c
          ORDER BY (c.origin = 'manual') DESC, c.id DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(CollectionSummary {
            id: row.get(0)?,
            kind: row.get(1)?,
            origin: row.get(2)?,
            title: row.get(3)?,
            size: row.get(4)?,
            cover_media_id: row.get(5)?,
            cover_nsfw: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 取一个集合的摘要（名字 / kind / origin / 成员数）；不存在回 [`WriteError::NotFound`]。
pub fn summary(conn: &Connection, id: i64) -> Result<CollectionSummary, WriteError> {
    conn.query_row(
        "SELECT c.id, c.kind, c.origin, c.title,
                (SELECT COUNT(*) FROM media_assets m WHERE m.collection_id = c.id),
                (SELECT m.id FROM media_assets m WHERE m.collection_id = c.id
                  ORDER BY m.id ASC LIMIT 1),
                COALESCE((SELECT m.nsfw FROM media_assets m WHERE m.collection_id = c.id
                           ORDER BY m.id ASC LIMIT 1), 0)
           FROM media_collections c WHERE c.id = ?1",
        params![id],
        |row| {
            Ok(CollectionSummary {
                id: row.get(0)?,
                kind: row.get(1)?,
                origin: row.get(2)?,
                title: row.get(3)?,
                size: row.get(4)?,
                cover_media_id: row.get(5)?,
                cover_nsfw: row.get(6)?,
            })
        },
    )
    .optional()?
    .ok_or(WriteError::NotFound)
}

/// **给集合改名**（集合详情中的「重命名」）。
///
/// 两条纪律：
/// 1. **空名字要明确拒绝**：界面上一旦允许清空，展开详情又回到"这一组"——
///    约束："每个集合都有名字"；
/// 2. **改名同时把 `origin` 置为 `manual`**：已设置名称，该组转为手动；
///    否则下次「重新自动整理」会把它当 `auto` 解散，名字跟着没。
pub fn rename(conn: &Connection, id: i64, title: &str) -> Result<CollectionSummary, WriteError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(WriteError::Validation("集合名不能为空".into()));
    }
    let changed = conn.execute(
        "UPDATE media_collections
            SET title = ?1, origin = ?2,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
          WHERE id = ?3",
        params![title, Origin::Manual.as_str(), id],
    )?;
    if changed == 0 {
        return Err(WriteError::NotFound);
    }
    summary(conn, id)
}

/// **新建一个手动集合**，并把 `media_ids` 全部移进去；返回新集合 id。
///
/// 选中的产物 **kind 必须一致**（同上：不混图像和视频），不一致就明确拒绝。
///
/// 成员从**老集合**搬走之后，老集合要收尾（[`prune_singleton_auto`]，唯一实现）：
/// 搬空的删除、`auto` 且只剩 1 个成员的一并解散。
/// "将内容从一个集合调到另一个集合的时候，如果旧集合元素为 1，旧集合没有自动解散"
/// 在"新建集合并移入"这条路上同样成立 —— 所以这里也要收。
pub fn create_manual(
    conn: &Connection,
    media_ids: &[i64],
    title: Option<&str>,
) -> Result<i64, WriteError> {
    if media_ids.is_empty() {
        return Err(WriteError::Validation("至少要选一个产物".into()));
    }
    let mut kind: Option<String> = None;
    for media_id in media_ids {
        let current: Option<String> = conn
            .query_row(
                "SELECT kind FROM media_assets WHERE id = ?1",
                params![media_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(current) = current else {
            return Err(WriteError::NotFound);
        };
        match kind.as_deref() {
            None => kind = Some(current),
            Some(existing) if existing.eq_ignore_ascii_case(&current) => {}
            Some(existing) => {
                return Err(WriteError::Validation(format!(
                    "选中的产物里有 {existing} 也有 {current}，一个集合只能装一种"
                )))
            }
        }
    }
    let kind = kind.ok_or(WriteError::NotFound)?;
    // 未填名字（或仅空白）时使用默认名 —— 展开详情不能显示"这一组"
    // 显式名称原样保留，只去除首尾空白。
    let title = match title.map(str::trim).filter(|t| !t.is_empty()) {
        Some(title) => title.to_string(),
        None => default_title(conn)?,
    };

    conn.execute(
        "INSERT INTO media_collections (group_key, kind, origin, rule_version, title)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            manual_key(media_ids),
            kind,
            Origin::Manual.as_str(),
            RULE_VERSION,
            title,
        ],
    )?;
    let created = conn.last_insert_rowid();

    for media_id in media_ids {
        conn.execute(
            "UPDATE media_assets SET collection_id = ?1, collection_opt_out = 0 WHERE id = ?2",
            params![created, media_id],
        )?;
    }
    // 老集合的收尾（唯一实现）：搬空的删掉、`auto` 且只剩 1 个成员的一并解散。
    // ⚠ 放在循环**之后**：循环里每搬一条就收尾一次的话，中间态会把"在本次搬选中
    // 迟早也要被搬空"的 auto 集合提前解散（结果一样，纯白干）。
    prune_singleton_auto(conn)?;
    Ok(created)
}

/// 手动集合的 `group_key`。
///
/// ⚠ 表上 `group_key` 是 `NOT NULL`，而且 [`attach`] 靠它找"算法那一组" ——
/// 所以手动集合的 key 必须以 `manual:` 开头：算法的 key 一定以 **kind** 开头
/// （见 [`group_key`]），两者**永远不可能相等**，手动集合因此天然不会被新图吸走。
fn manual_key(media_ids: &[i64]) -> String {
    let anchor = media_ids.iter().copied().min().unwrap_or(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("manual:{anchor}:{millis}")
}

/// 清掉**所有**空集合（存量收拾 / 批量删除之后的收尾）。
///
/// 为什么必须有这一步：`media_assets.collection_id` 是 `ON DELETE SET NULL`，
/// 所以**删产物不会带走集合行**。少了它会有两个后果：
/// ① "删到 0 张仍保留空组"（界面显示为无法点开的空格子）；
/// ② 集合行**永久累积**（每删一批就多一个孤儿集合，而且它会跟着同步到手机）。
///
/// ⚠ **写路径不要直接调它**（删产物 / 移出 / 移进 / 新建手动集合一律走
/// [`prune_singleton_auto`]，它把"空了删掉"和"`auto` 只剩一张解散"合成了一条收尾路径）：
/// 单独调这个只做了前半件，于是"删一张"与"移走一张"两条入口的终态就不一致了
/// 它现在只作为 [`prune_singleton_auto`] 的最后一步存在。
pub fn prune_all_empty(conn: &Connection) -> Result<usize, WriteError> {
    Ok(conn.execute(
        "DELETE FROM media_collections
          WHERE NOT EXISTS (SELECT 1 FROM media_assets WHERE collection_id = media_collections.id)",
        [],
    )?)
}

/// 解散**算法分出来的、只剩 1 个成员**的集合：把最后那张放回散图
/// （`collection_id = NULL`），再把集合行删掉。返回解散了几个。**幂等**。
///
/// # "只剩一张"也解散的依据
///
/// "将内容从一个集合调到另一个集合时，如果旧集合元素为 1，
/// 旧集合没有自动解散。" 一个 `auto` 集合的语义是"同一句提示词的一批微调"（见文件头），
/// 只剩一张时它不再是一批，而界面上仍然是一个叠放封面 + 数量角标、点进去只有一张的格子。
/// 判据是**集合的语义**，不是"空 / 非空" —— 所以只清 0 成员的 [`prune_all_empty`]
/// 无法覆盖它，需单独处理（"只剩一张"这一档因此属于**收尾**的职责，而不是"不创建"
/// 一句话能覆盖的：`assign` 那条入库路会先建出单成员组，要等这里的收尾或 [`backfill`]）。
///
/// # 只碰 `origin = 'auto'`（**硬纪律**，别放宽）
///
/// `manual` 集合是**手动创建的**（可能已改标题），只剩一张也必须保留 ——
/// 解散该集合等同于删除其数据。已修改的 `auto` 集合在 [`move_into`] 中已就地转为
/// `manual`（[`mark_manual`]），所以这里天然放过它。
///
/// # 不变量（三条，顺序有意义，别调）
///
/// 1. **先摘成员、再删集合行**：`media_assets.collection_id` 虽然是 `ON DELETE SET NULL`，
///    但外键开关（`PRAGMA foreign_keys`）不保证每个连接都打开 —— 显式置 NULL 才是
///    "一定会被摘下来"的那条路；
/// 2. **`collection_opt_out` 不写入**：该图并非"主动移出"，只是在当前分组中
///    不够成组。写了它就会被 [`backfill`] 永久挡在外面（以后同款提示词再来一张，
///    它也回不去那个本该 >= 2 的组）；
/// 3. **最后再 [`prune_all_empty`]**：不变量 1、2 之间可能留下 0 成员的残留
///    （成员先被别的语句删掉之类的极端边界），一次扫干净，顺带把空壳带走。
///
/// ⚠ 调用时机（写在这里免得被搬错）：**任何"把产物搬离集合"的写路径**都要调它
/// （[`detach`] / [`move_into`] / [`create_manual`] / 删产物），以及**启动回填之后**
/// （`main.rs`；历史遗留的单成员 auto 组、以及半截回填留下的，只可能在那里被收拾）。
/// **入库那条路（[`assign`]）不能调** —— 一批同款图是一条一条进来的，中途每条都是
/// "单成员组"，当场解散会让它们永远并不到一起（第 2 张来时又新建一个单成员组，如此循环）。
/// **[`regroup_auto`] 也不需要调** —— 它先删光所有 auto、再只对 >= 2 张的桶建组，
/// 结构上不可能留下单成员组（推演写在它的纪律 4 里）。
pub fn prune_singleton_auto(conn: &Connection) -> Result<usize, WriteError> {
    // 判据只写这一遍：先把名单取出来，下面两条语句共用。
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare(
            "SELECT c.id FROM media_collections c
              WHERE c.origin = ?1
                AND (SELECT COUNT(*) FROM media_assets m WHERE m.collection_id = c.id) = 1",
        )?;
        let rows = stmt.query_map(params![Origin::Auto.as_str()], |row| row.get::<_, i64>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let dissolved = ids.len();
    if !ids.is_empty() {
        let placeholders = vec!["?"; ids.len()].join(", ");
        // ① 先摘成员（不碰 `collection_opt_out`，见不变量 2）
        conn.execute(
            &format!(
                "UPDATE media_assets SET collection_id = NULL WHERE collection_id IN ({placeholders})"
            ),
            rusqlite::params_from_iter(ids.iter()),
        )?;
        // ② 再删集合行
        conn.execute(
            &format!("DELETE FROM media_collections WHERE id IN ({placeholders})"),
            rusqlite::params_from_iter(ids.iter()),
        )?;
    }
    // ③ 0 成员残留（幂等；空壳一个都不留）
    prune_all_empty(conn)?;
    Ok(dissolved)
}

/// 找出**与这条预设同款提示词**的产物 id（按时间升序）。
///
/// 使用同一提示词的产物需自动关联预设，并显示在关联产物中。
///
/// # 为什么不建一张关联表
///
/// 关联的判据就是**归一化后的提示词相同** —— 而那是 [`normalize`] 一个纯函数，
/// 现算的代价是"一次列表查询 + 内存里比字符串"。建表要维护三件事：
/// ① 每次入库都要回填；② 提示词改了要重算；③ 规则改了要全量重算。
/// 现算没有这些尾巴，而且**天然正确**（改完提示词立刻反映）。
///
/// ⚠ 代价（写清楚，别当 bug）：产物多到几万条时这个列表查询会变慢。
/// 到那时再加索引/缓存，**别提前**。
///
/// # 判据必须与集合判定**共用同一个** [`Endpoint`]
///
/// 否则会出现"两张图在同一集合里，但只有一张被认成这个预设的关联产物"——
/// 而那是同一个归一化问题，不该有两份答案。
pub fn media_of_preset(conn: &Connection, prompt_id: i64) -> Result<Vec<i64>, WriteError> {
    let preset = conn
        .query_row(
            "SELECT kind, positive_prompt, negative_prompt FROM prompts WHERE id = ?1",
            params![prompt_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((kind, positive, negative)) = preset else {
        return Err(WriteError::NotFound);
    };
    let want = Endpoint::new(&kind, positive, negative.filter(|s| !s.trim().is_empty()));

    // 候选：同 kind 的产物 + 它们关联的提示词正文。
    // **同 kind 是一道便宜的先筛**（判据里 kind 必须相同），把行数先砍一大截。
    let mut stmt = conn.prepare(
        "SELECT m.id, p.positive_prompt, p.negative_prompt
           FROM media_assets m JOIN prompts p ON p.id = m.prompt_id
          WHERE m.kind = ?1
          ORDER BY m.created_at ASC, m.id ASC",
    )?;
    let rows = stmt.query_map(params![want.kind], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (id, positive, negative) = row?;
        let other = Endpoint::new(
            &want.kind,
            positive,
            negative.filter(|s| !s.trim().is_empty()),
        );
        if same_group(&want, &other) {
            out.push(id);
        }
    }
    Ok(out)
}

/// 找出与这条提示词**同款**的预设 id（给入库时"自动关联"用）。
///
/// 入库那条路上要的是"这条自动捕获的提示词，有没有对应的预设" ——
/// 也就是 [`media_of_preset`] 的反方向。判据同一份（[`same_group`]）。
///
/// 只比 `Manual` 那一边（预设库的内容）：自动捕获的行之间互相关联没有意义。
pub fn presets_matching(conn: &Connection, prompt_id: i64) -> Result<Vec<i64>, WriteError> {
    let row = conn
        .query_row(
            "SELECT kind, positive_prompt, negative_prompt FROM prompts WHERE id = ?1",
            params![prompt_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((kind, positive, negative)) = row else {
        return Err(WriteError::NotFound);
    };
    let want = Endpoint::new(&kind, positive, negative.filter(|s| !s.trim().is_empty()));

    let mut stmt = conn.prepare(
        "SELECT id, kind, positive_prompt, negative_prompt FROM prompts
          WHERE id <> ?1 AND (source IS NULL OR source = 'Manual')
          ORDER BY id DESC",
    )?;
    let rows = stmt.query_map(params![prompt_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (id, kind, positive, negative) = row?;
        let other = Endpoint::new(&kind, positive, negative.filter(|s| !s.trim().is_empty()));
        if same_group(&want, &other) {
            out.push(id);
        }
    }
    Ok(out)
}

/// 与这条提示词**同款**的产物数量（= [`media_of_preset`] 的长度，但不多取那些行）。
pub fn media_count_of_preset(conn: &Connection, prompt_id: i64) -> Result<i64, WriteError> {
    Ok(media_of_preset(conn, prompt_id)?.len() as i64)
}

/// **有**关联产物的提示词 id（"同款"判据，与 [`media_of_preset`] 同一份）。
///
/// 「未关联产物」筛选吃它：一个预设的"有没有关联产物"不能只看
/// `media_assets.prompt_id == 预设 id` —— 从画廊存下来的预设，产物指向的是
/// **自动捕获的那条**提示词行，直连判定会返回"未关联"，而该图正是从该行保存的
/// （现象："已与产物关联，但主界面卡片仍显示未关联"）。
///
/// 一次遍历算完所有预设，避免"每个预设各查一遍"的 N 次查询。
pub fn prompt_ids_with_media(conn: &Connection) -> Result<Vec<i64>, WriteError> {
    // 先把"提示词 id → 它的端点"读进内存（提示词数量级是几百，没问题）
    let mut stmt =
        conn.prepare("SELECT id, kind, positive_prompt, negative_prompt FROM prompts")?;
    let prompts = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                Endpoint::new(
                    &row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?
                        .filter(|s| !s.trim().is_empty()),
                ),
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    // 再看每个提示词被哪些产物引用（同 kind 的先筛一道）
    let mut refs = conn.prepare(
        "SELECT p.id, p.kind, p.positive_prompt, p.negative_prompt
           FROM media_assets m JOIN prompts p ON p.id = m.prompt_id",
    )?;
    let referenced = refs
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                Endpoint::new(
                    &row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?
                        .filter(|s| !s.trim().is_empty()),
                ),
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut out = Vec::new();
    for (id, want) in &prompts {
        if referenced.iter().any(|(_, got)| same_group(want, got)) {
            out.push(*id);
        }
    }
    out.sort_unstable();
    Ok(out)
}

#[cfg(test)]
mod tests;

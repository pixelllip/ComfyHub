//! 产物**集合**的成组判定：哪几张图属于"同一批、只做了一点点微调"。
//!
//! 设计依据（用户 2026-10-11 拍板，逐条别改）：
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
//! 而那正是用户说的"step/cfg 不是主要因素"。反过来把参数全忽略，判定就退化成
//! **一个纯函数、只看文本** —— 可解释、可回填、可测。
//!
//! # 代价（写在这里，别当没发生）
//!
//! * **同模板不同采样器的图会被并成一组**（`euler a` 与 `dpmpp_2m` 出的图确实会不一样）。
//!   这是刻意接受的：用户要的语义是"我一直在调同一张图"，不是"参数完全一致"。
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
//! # 拼 key 的坑（踩过就别再踩）
//!
//! 归一化后的片段**可能含 ASCII 控制字符**（用户提示词里粘贴进 `\u{1}`、`\r` 都可能），
//! 所以**不能**拿控制字符当分隔符 —— 那样 `["a", "b\u{1}c"]` 与 `["a\u{1}b", "c"]`
//! 会拼出同一个 key（**造出假分组，而且不报错**）。
//! 现在的写法是**长度前缀**（`len ⊗ "+"'len2 0x1F 片段 ⊗ ...`），消除歧义且不用防转义。

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
    pub fn new(kind: &str, positive: impl Into<String>, negative: Option<impl Into<String>>) -> Self {
        Self {
            kind: kind.trim().to_uppercase(),
            positive: positive.into(),
            negative: negative.map(Into::into),
        }
    }
}

impl From<&PromptDto> for Endpoint {
    fn from(p: &PromptDto) -> Self {
        Self::new(&p.kind, p.positive_prompt.clone(), p.negative_prompt.clone())
    }
}

impl From<&MediaDto> for Endpoint {
    fn from(m: &MediaDto) -> Self {
        // 产物的正提示词来自它关联的提示词；没关联就是空。
        // `prompt_positive` 为 None 时不能套用 `Endpoint::new` 的 `Option<impl Into<String>>`
        // （类型推断不出来），所以直接把负提示词也算好再构造。
        let negative = m.prompt_negative.clone().filter(|s| !s.trim().is_empty());
        let mut ep = Self::new(&m.kind, m.prompt_positive.clone().unwrap_or_default(), negative);
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

/// 把归一化后的片段拼成一个**无歧义**的串（长度前缀，见文件头"拼 key 的坑"）。
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
    /// 用户拆开 / 合并出来的 —— **重算只动 Auto**，别把用户刚拆开的又合回去。
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
/// 那不是用户要的"同批微调"，点进去左右翻 21 张不认识的图毫无意义。
///
/// 判据本来就该是**提示词**（用户原话"它决定了产物的走向"）——
/// 没有提示词就没有"走向"可判，所以**宁可不分组**：详情页箭头禁用，退化成普通格子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assigned {
    /// 进了某个集合。
    Grouped(i64),
    /// 没关联提示词 → 刻意不分组。
    Orphan,
}

/// 读一条产物的成组输入：`Ok(None)` = 孤儿（没关联提示词），`Err(NotFound)` = 产物不在。
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
    if prompt_id.is_none() {
        return Ok(None);
    }
    Ok(Some(Endpoint::new(
        &kind,
        pos.unwrap_or_default(),
        neg.filter(|s| !s.trim().is_empty()),
    )))
}

/// 按 `group_key` 查/建集合并把产物挂上去。
///
/// 匹配规则：**`group_key` 相等**，且**优先挂到最近建的 `auto` 集合**。
/// 为什么优先 auto：用户拆开之后会留下 `manual` 集合，而新产物该进的是"算法那一组"，
/// 不是被拆出来的那一份 —— 否则用户拆一次，之后所有新图都往那个小堆里塞。
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

/// 把**还没分组**的产物按同一套规则分一遍（存量回填 / 规则改了之后重算）。
///
/// 三条纪律：
/// 1. **只动 `collection_id IS NULL` 的行** —— 用户手动拆开/合并过的（`origin='manual'`）
///    以及已经分好组的，一律不碰。用户刚拆开又被合回去是最气人的那种 bug。
/// 2. **按 `created_at` 升序** —— 同一组的成员顺序就是"调参的先后"，集合的
///    `id` 也据此稳定（详情页左右切换靠它）。
/// 3. 返回"这次分了多少条"（孤儿不算）。调用方据此决定要不要写日志。
pub fn backfill(conn: &Connection) -> Result<usize, WriteError> {
    let rows: Vec<(i64, Option<i64>, String, Option<String>, Option<String>)> = {
        let mut stmt = conn.prepare(
            "SELECT m.id, m.prompt_id, m.kind, p.positive_prompt, p.negative_prompt
               FROM media_assets m LEFT JOIN prompts p ON p.id = m.prompt_id
              WHERE m.collection_id IS NULL
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
    let mut done = 0usize;
    for (id, prompt_id, kind, pos, neg) in rows {
        // 孤儿产物：不分组，也不建集合（见 `Assigned` 的注释）
        if prompt_id.is_none() {
            continue;
        }
        let ep = Endpoint::new(&kind, pos.unwrap_or_default(), neg.filter(|s| !s.trim().is_empty()));
        attach(conn, id, &ep)?;
        done += 1;
    }
    Ok(done)
}

/// 集合空了就把它删掉；还有成员就什么都不做。**幂等**（集合不在了也不报错）。
///
/// 为什么必须有这一步：`media_assets.collection_id` 是 `ON DELETE SET NULL`，
/// 所以**删产物不会带走集合行**。少了它会有两个后果：
/// ① "删到 0 张还留一个空组"（用户看到的是一个点不开的空格子）；
/// ② 集合行**永久累积**（每删一批就多一个孤儿集合，而且它会跟着同步到手机）。
pub fn prune_if_empty(conn: &Connection, collection_id: i64) -> Result<(), WriteError> {
    conn.execute(
        "DELETE FROM media_collections
          WHERE id = ?1
            AND NOT EXISTS (SELECT 1 FROM media_assets WHERE collection_id = ?1)",
        params![collection_id],
    )?;
    Ok(())
}

/// 清掉**所有**空集合（存量收拾 / 批量删除之后的收尾）。
///
/// 返回删了几个。批量删除（如"清除未关联产物"）之后调一次，比逐条 prune 便宜。
pub fn prune_all_empty(conn: &Connection) -> Result<usize, WriteError> {
    Ok(conn.execute(
        "DELETE FROM media_collections
          WHERE NOT EXISTS (SELECT 1 FROM media_assets WHERE collection_id = media_collections.id)",
        [],
    )?)
}

/// 找出**与这条预设同款提示词**的产物 id（按时间升序）。
///
/// 用户 2026-10-11："如果有用这套预设同款提示词的产物，自动关联预设，显示在关联产物中。"
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
        let other = Endpoint::new(&want.kind, positive, negative.filter(|s| !s.trim().is_empty()));
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
/// **自动捕获那条**提示词行，直连判定会说"未关联"，而用户明明是从那张图存的
/// （2026-10-11 用户报的"实际已经跟产物关联，但是主界面卡片依然显示未关联"）。
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
                    row.get::<_, Option<String>>(3)?.filter(|s| !s.trim().is_empty()),
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
                    row.get::<_, Option<String>>(3)?.filter(|s| !s.trim().is_empty()),
                ),
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut out = Vec::new();
    for (id, want) in &prompts {
        if referenced
            .iter()
            .any(|(_, got)| same_group(want, got))
        {
            out.push(*id);
        }
    }
    out.sort_unstable();
    Ok(out)
}

#[cfg(test)]
mod tests;

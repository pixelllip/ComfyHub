//! 成组判定的用例。
//!
//! 这份用例的定位：**它是"产物会不会被错误地并成一组"的唯一防线**。
//! 分组判错的后果不是报错，是界面上把两张不相干的图放进同一个格子 —— 静默、难查。
//!
//! 所以每一类都至少有一条：
//! 排版差异**必须**并组（`(x:1.2)` ↔ `x`）；内容差异**必须**拆组；
//! 参数差异（seed / steps / cfg / checkpoint / 尺寸）**必须不影响**；
//! 负提示词的 `None` 与空白**必须**等价；拼 key 不许出现假相等。

use super::*;
use crate::web::models::{MediaDto, PromptDto};

/// 造一个只填了成组相关字段的端点。
fn ep(kind: &str, pos: &str, neg: Option<&str>) -> Endpoint {
    Endpoint::new(kind, pos, neg)
}

// ---------------------------------------------------------------------------
//  1. 核心那条：自然语言与权重表达是同一件事
// ---------------------------------------------------------------------------

#[test]
fn 权重表达与自然语言算同一组() {
    // 用户 2026-10-11 原话："自然语言和权重表达很多时候说的是同一样东西"。
    let a = ep("IMAGE", "(masterpiece:1.2), 1girl", None);
    let b = ep("IMAGE", "masterpiece, (1girl:1.1)", None);
    assert!(same_group(&a, &b), "权重写法不同不该拆组");
    assert_eq!(group_key(&a), group_key(&b), "key 也必须一致（入库与回填要用它）");
}

#[test]
fn 归一化的全部削平手段一起生效() {
    // 大小写 / 换行 / 顺序 / 多余空白 / **带数字的权重**，一起削平。
    let a = ep(
        "IMAGE",
        "(masterpiece:1.2), 1girl,  (detailed background:1.4) ",
        None,
    );
    let b = ep(
        "IMAGE",
        "detailed background,1GIRL,\n masterpiece",
        None,
    );
    assert!(
        same_group(&a, &b),
        "大小写 / 换行 / 顺序 / 多余空白 / 权重，都该被削平"
    );
}

#[test]
fn 裸括号参与比较_不是噪音() {
    // 与上一条对照：裸括号是加强权重，所以它必须让两条**不相等**。
    // 早期实现把裸括号当排版噪音剥掉，这条会红。
    let a = ep("IMAGE", "(detailed background), 1girl", None);
    let b = ep("IMAGE", "detailed background, 1girl", None);
    assert!(!same_group(&a, &b));
}

#[test]
fn 内容不同必须拆组() {
    let a = ep("IMAGE", "1girl, masterpiece", None);
    let b = ep("IMAGE", "2girls, masterpiece", None);
    assert!(!same_group(&a, &b), "换了主体绝不能并成一组");
    assert_ne!(group_key(&a), group_key(&b));
}

#[test]
fn 多一个词也算不同组() {
    let a = ep("IMAGE", "1girl, masterpiece", None);
    let b = ep("IMAGE", "1girl, masterpiece, smile", None);
    assert!(!same_group(&a, &b), "削平不等于模糊匹配");
}

// ---------------------------------------------------------------------------
//  2. 参数一律不影响成组（seed / steps / cfg / checkpoint / 尺寸 / 采样器）
// ---------------------------------------------------------------------------

#[test]
fn 种子不同算同一组() {
    // 用户 2026-10-11 原话："种子不同当然也算同一组"。
    // ⚠ 这条用例**证明的是"seed 不在判定输入里"** —— Endpoint 根本没有 seed 字段。
    // 谁要是以后把 seed 加进 Endpoint，这条会立刻红。
    let a = ep("IMAGE", "1girl, masterpiece", None);
    let b = ep("IMAGE", "1girl, masterpiece", None);
    assert!(same_group(&a, &b));
    assert_eq!(group_key(&a), group_key(&b));
}

#[test]
fn 步数与cfg不影响成组() {
    // 用户 2026-10-11 原话："step/cfg 并不是影响产物内容的主要因素"。
    // 同上的证明方式：这两个值进不了 key。
    let a = ep("IMAGE", "1girl, masterpiece", None);
    let b = ep("IMAGE", "1girl, masterpiece", None);
    assert_eq!(group_key(&a), group_key(&b), "steps/cfg 不参与判定");
}

#[test]
fn 端点的字段只有三个() {
    // 这是**结构性**防线：以后有人想加参数维度，会先撞上这条用例。
    // 真需要加（比如"用户可选的严格模式"）时，改这里 + 文件头说明 + RULE_VERSION +1。
    let a = ep("IMAGE", "x", None);
    let expected = Endpoint {
        kind: "IMAGE".to_string(),
        positive: "x".to_string(),
        negative: None,
    };
    assert_eq!(a, expected);
}

// ---------------------------------------------------------------------------
//  3. 负提示词：None 与空白等价；有内容的必须拆组
// ---------------------------------------------------------------------------

#[test]
fn 负提示词缺失与空白等价() {
    let a = ep("IMAGE", "1girl", None);
    let b = ep("IMAGE", "1girl", Some(""));
    let c = ep("IMAGE", "1girl", Some("   ,  \n "));
    assert!(same_group(&a, &b));
    assert!(same_group(&a, &c));
    assert_eq!(group_key(&a), group_key(&b));
    assert_eq!(group_key(&a), group_key(&c));
}

#[test]
fn 负提示词不同必须拆组() {
    let a = ep("IMAGE", "1girl", Some("bad hands"));
    let b = ep("IMAGE", "1girl", Some("bad hands, extra fingers"));
    assert!(!same_group(&a, &b));
    assert_ne!(group_key(&a), group_key(&b));
}

#[test]
fn 负提示词也享受同一套削平() {
    let a = ep("IMAGE", "1girl", Some("(bad hands:1.3), extra fingers"));
    let b = ep("IMAGE", "1girl", Some("extra fingers, bad hands"));
    assert!(same_group(&a, &b));
}

// ---------------------------------------------------------------------------
//  4. 归一化的边界：嵌套括号、转义、假权重
// ---------------------------------------------------------------------------

#[test]
fn 嵌套的权重括号被递归剥掉() {
    assert_eq!(normalize("(a:1.2)"), normalize("a"));
    assert_eq!(normalize("((a:1.2):1.1)"), normalize("a"));
    assert_eq!(normalize("(((a:1.1):1.1):1.1)"), normalize("a"));
}

#[test]
fn 裸括号是加强权重_不许剥() {
    // danbooru / anima 里 `(x)` 与 `((x))` 是**加强权重**，是真实语义不是排版噪音。
    // 剥掉就等于改了提示词 —— 这条曾经写错过（早期实现会递归剥光所有括号）。
    assert_eq!(normalize("(x)"), vec!["(x)".to_string()]);
    assert_eq!(normalize("((x))"), vec!["((x))".to_string()]);
    assert_ne!(normalize("(x)"), normalize("x"));
}

#[test]
fn 转义的括号是内容不是权重() {
    // `\(x\)` 是"字面上带括号的 tag"，剥掉就等于改了内容。
    assert_eq!(normalize(r"\(x\)"), vec!["(x)".to_string()]);
    assert_ne!(normalize(r"\(x\)"), normalize("x"));
}

#[test]
fn 转义的逗号不切段() {
    assert_eq!(normalize(r"a\,b"), vec!["a,b".to_string()]);
    // 而且它与"真的两段"必须区分开 —— 否则拼 key 会撞出假分组。
    assert_ne!(normalize(r"a\,b"), normalize("a, b"));
}

#[test]
fn 权重不是数字就原样留着() {
    // 提示词里的 `:` 更多时候不是权重（`girl:1.2` 这种 tag 很常见）。
    // 认不出来**绝不猜** —— 猜错就是把真实文本吃掉。
    assert_eq!(normalize("(x:abc)"), vec!["(x:abc)".to_string()]);
    assert_eq!(normalize("(x:1.2e3)"), vec!["(x:1.2e3)".to_string()]);
}

#[test]
fn 括号没包住整段就不当权重() {
    // `strip_prefix('(') + strip_suffix(')')` 对 `(a)+(b)` 也会命中 —— 必须靠顶层括号数挡掉。
    assert_eq!(normalize("(a)+(b)"), vec!["(a)+(b)".to_string()]);
}

#[test]
fn 空串与只有逗号都是空() {
    assert!(normalize("").is_empty());
    assert!(normalize(" , ,\n, ").is_empty());
}

// ---------------------------------------------------------------------------
//  5. kind 隔离：图 / 视频 / 音频不许并组
// ---------------------------------------------------------------------------

#[test]
fn 类型不同不许并组() {
    // 用户 2026-10-11："集合对于图、视频、音频独立讨论，不存在不同类型产物混合进一个集合"。
    let a = ep("IMAGE", "1girl", None);
    let b = ep("VIDEO", "1girl", None);
    assert!(!same_group(&a, &b));
    assert_ne!(group_key(&a), group_key(&b));
}

#[test]
fn 类型大小写与空白被削平() {
    let a = ep("image", "1girl", None);
    let b = ep("  IMAGE  ", "1girl", None);
    assert!(same_group(&a, &b));
    assert_eq!(group_key(&a), group_key(&b));
}

// ---------------------------------------------------------------------------
//  6. 拼 key：假相等是静默故障，必须挡住
// ---------------------------------------------------------------------------

#[test]
fn 片段切分不同不会拼出同一个key() {
    // 朴素拼接（拿分隔符 join）在这里会撞：["a", "b\x1fc"] 与 ["a\x1fb", "c"]。
    // 归一化后的片段**可能含控制字符**（用户粘贴进来的），所以不能拿控制字符当分隔符。
    let a = ep("IMAGE", "a, b\u{1}c", None);
    let b = ep("IMAGE", "a\u{1}b, c", None);
    assert_ne!(
        group_key(&a),
        group_key(&b),
        "拼 key 有歧义 = 假分组，而且不报错"
    );
}

#[test]
fn 空片段列表有独立占位() {
    // "正提示词为空" 与 "正提示词是一个空片段" 不该撞（后者其实归一化后也是空，
    // 但占位符的存在让这条规则不依赖那个巧合）。
    let empty = ep("IMAGE", "", None);
    assert!(group_key(&empty).contains(EMPTY));
}

#[test]
fn key是纯函数_同输入永远同key() {
    let a = ep("IMAGE", "1girl, (masterpiece:1.2)", Some("bad hands"));
    let first = group_key(&a);
    for _ in 0..5 {
        assert_eq!(group_key(&a), first);
    }
    // 换个等价写法，key 也必须一样（回填与入库靠这个对齐）
    let b = ep("IMAGE", "masterpiece, 1girl", Some("bad hands"));
    assert_eq!(group_key(&b), first);
}

// ---------------------------------------------------------------------------
//  7. DTO 转换：入库那条路实际用的入口
// ---------------------------------------------------------------------------

fn media_dto(kind: &str, pos: Option<&str>, neg: Option<&str>) -> MediaDto {
    MediaDto {
        id: 1,
        prompt_id: Some(1),
        kind: kind.to_string(),
        title: String::new(),
        original_name: "a.png".into(),
        stored_name: "a.png".into(),
        mime_type: None,
        size_bytes: 0,
        width: None,
        height: None,
        duration_ms: None,
        sha256: None,
        source: None,
        favorite: false,
        notes: None,
        nsfw: false,
        created_at: None,
        updated_at: None,
        has_workflow: false,
        prompt_title: None,
        prompt_positive: pos.map(str::to_string),
        prompt_negative: neg.map(str::to_string),
        checkpoint: None,
        seed: None,
        collection_id: None,
        collection_size: 1,
        collection_preview: Vec::new(),
        prompt_tags: Vec::new(),
        file_url: String::new(),
        thumb_url: String::new(),
    }
}

#[test]
fn 从产物_dto取端点_没关联提示词就是空() {
    let a = Endpoint::from(&media_dto("IMAGE", None, None));
    let b = ep("IMAGE", "", None);
    assert_eq!(group_key(&a), group_key(&b), "孤儿产物也要能算出一个稳定的 key");

    let c = Endpoint::from(&media_dto("IMAGE", Some("1girl"), Some("bad hands")));
    let d = ep("IMAGE", "1girl", Some("bad hands"));
    assert_eq!(group_key(&c), group_key(&d));
}

#[test]
fn 从产物_dto取端点_负提示词空白当没有() {
    let a = Endpoint::from(&media_dto("IMAGE", Some("1girl"), Some("  ")));
    let b = ep("IMAGE", "1girl", None);
    assert_eq!(group_key(&a), group_key(&b));
}

fn prompt_dto(kind: &str, pos: &str, neg: Option<&str>) -> PromptDto {
    PromptDto {
        id: 1,
        title: String::new(),
        kind: kind.to_string(),
        positive_prompt: pos.to_string(),
        negative_prompt: neg.map(str::to_string),
        checkpoint: None,
        loras: Vec::new(),
        sampler: None,
        scheduler: None,
        steps: Some(20),
        cfg_scale: Some(7.0),
        seed: Some(12345),
        width: Some(1024),
        height: Some(1024),
        batch_size: Some(1),
        extra_params: serde_json::Value::Null,
        notes: None,
        negative_mode: "keep".to_string(),
        favorite: false,
        source: None,
        source_ref: None,
        has_workflow: false,
        created_at: None,
        updated_at: None,
        tags: Vec::new(),
        media_count: 0,
    }
}

#[test]
fn 从提示词_dto取端点() {
    let a = Endpoint::from(&prompt_dto("IMAGE", "(masterpiece:1.2), 1girl", Some("bad hands")));
    let b = ep("IMAGE", "1girl, masterpiece", Some("bad hands"));
    assert!(same_group(&a, &b));
}

// ---------------------------------------------------------------------------
//  8. 记录当前取舍（这些"会并组"是刻意的，不是漏了）
// ---------------------------------------------------------------------------

#[test]
fn 采样器与底模不同也会并组_这是刻意取舍() {
    // 判定输入里没有 checkpoint / sampler / 尺寸，所以它们不同**不影响**成组。
    // 这是用户定的"提示词相同就都算同一组"的直接后果，写在用例里免得以后被当 bug 修。
    let a = Endpoint::from(&prompt_dto("IMAGE", "1girl", None));
    let mut other = prompt_dto("IMAGE", "1girl", None);
    other.checkpoint = Some("sd_xl_base_1.0.safetensors".into());
    other.sampler = Some("dpmpp_2m".into());
    other.steps = Some(60);
    other.cfg_scale = Some(3.5);
    other.seed = Some(999);
    other.width = Some(832);
    let b = Endpoint::from(&other);
    assert!(
        same_group(&a, &b),
        "参数不同不影响成组（用户拍板：提示词相同就都算同一组）"
    );
}

// ---------------------------------------------------------------------------
//  9. 落库那一半：查/建集合、挂上去、回填不覆盖用户的手动拆并
// ---------------------------------------------------------------------------

/// 一个真的（空的）库，用来测 `assign` / `backfill`。
fn temp_db(label: &str) -> (Connection, std::path::PathBuf) {
    let dir = crate::infra::testpaths::scratch(&format!("collection-{label}"));
    let path = dir.join("comfyhub.db");
    let conn = crate::infra::db::open_or_create(&path).expect("建测试库");
    (conn, dir)
}

/// 建一条提示词，返回 id。
fn insert_prompt(conn: &Connection, kind: &str, pos: &str, neg: Option<&str>) -> i64 {
    conn.execute(
        "INSERT INTO prompts (title, kind, positive_prompt, negative_prompt)
         VALUES ('t', ?1, ?2, ?3)",
        params![kind, pos, neg],
    )
    .unwrap();
    conn.last_insert_rowid()
}

/// 建一条产物，返回 id。
fn insert_media(conn: &Connection, prompt_id: Option<i64>, kind: &str) -> i64 {
    conn.execute(
        "INSERT INTO media_assets (prompt_id, kind, original_name, stored_name)
         VALUES (?1, ?2, 'a.png', 'a.png')",
        params![prompt_id, kind],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn collection_of(conn: &Connection, media_id: i64) -> Option<i64> {
    conn.query_row(
        "SELECT collection_id FROM media_assets WHERE id = ?1",
        params![media_id],
        |r| r.get(0),
    )
    .unwrap()
}

/// 断言"这条产物进了组"，并返回集合 id（用例里绝大多数场景都是这一类）。
fn group_id(conn: &Connection, media_id: i64) -> i64 {
    match assign(conn, media_id).expect("定组不该失败") {
        Assigned::Grouped(id) => id,
        Assigned::Orphan => panic!("这条产物有提示词，不该被判成孤儿"),
    }
}

#[test]
fn 落库_权重写法不同会被分到同一个集合() {
    let (conn, dir) = temp_db("same");
    let p1 = insert_prompt(&conn, "IMAGE", "(masterpiece:1.2), 1girl", None);
    let p2 = insert_prompt(&conn, "IMAGE", "1girl, masterpiece", None);
    let m1 = insert_media(&conn, Some(p1), "IMAGE");
    let m2 = insert_media(&conn, Some(p2), "IMAGE");

    let c1 = group_id(&conn, m1);
    let c2 = group_id(&conn, m2);
    assert_eq!(c1, c2, "归一化后相同 → 必须挂到同一个集合");
    assert_eq!(collection_of(&conn, m2), Some(c1));
    // 只该有**一个**集合行
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM media_collections", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "不许每次入库都新建一个集合");
    crate::infra::testpaths::cleanup(&dir);
}

#[test]
fn 落库_内容不同会分成两个集合() {
    let (conn, dir) = temp_db("diff");
    let p1 = insert_prompt(&conn, "IMAGE", "1girl", None);
    let p2 = insert_prompt(&conn, "IMAGE", "2girls", None);
    let m1 = insert_media(&conn, Some(p1), "IMAGE");
    let m2 = insert_media(&conn, Some(p2), "IMAGE");
    assert_ne!(group_id(&conn, m1), group_id(&conn, m2));
    crate::infra::testpaths::cleanup(&dir);
}

#[test]
fn 落库_类型不同分成两个集合() {
    let (conn, dir) = temp_db("kind");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let m1 = insert_media(&conn, Some(p), "IMAGE");
    let m2 = insert_media(&conn, Some(p), "VIDEO");
    assert_ne!(group_id(&conn, m1), group_id(&conn, m2));
    crate::infra::testpaths::cleanup(&dir);
}

/// ★ **孤儿产物不进组**（2026-10-11 真库上量出来才定的）。
///
/// 旧实现把 21 张互不相干的手动导入图并成了一组 —— 那不是"同批微调"。
/// 判据是提示词，没有提示词就没有"走向"可判，宁可不分组（详情页箭头禁用）。
#[test]
fn 落库_孤儿产物不进组也不建集合() {
    let (conn, dir) = temp_db("orphan");
    let m1 = insert_media(&conn, None, "IMAGE");
    let m2 = insert_media(&conn, None, "IMAGE");
    assert_eq!(assign(&conn, m1).unwrap(), Assigned::Orphan);
    assert_eq!(assign(&conn, m2).unwrap(), Assigned::Orphan);
    assert_eq!(collection_of(&conn, m1), None);
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM media_collections", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "孤儿产物连集合行都不该有");
    crate::infra::testpaths::cleanup(&dir);
}

/// ★ **有关联、但正文空白**的产物同样不进组（2026-10-18 补的那一档）。
///
/// 少了它，所有"提示词解析不出来"的运行会全部并进同一个 `IMAGE ⊗ ∅` 集合 ——
/// 那就是用户报的"生成的内容自动分类到错误的集合"。
#[test]
fn 落库_关联了提示词但正文空白的也不进组() {
    let (conn, dir) = temp_db("blank-prompt");
    // 三种"看起来空"的写法：空格、只有逗号、只有换行（负向也给一份）
    let p1 = insert_prompt(&conn, "IMAGE", "   ", None);
    let p2 = insert_prompt(&conn, "IMAGE", ",,,", Some(" \n "));
    let m1 = insert_media(&conn, Some(p1), "IMAGE");
    let m2 = insert_media(&conn, Some(p2), "IMAGE");

    assert_eq!(assign(&conn, m1).unwrap(), Assigned::Orphan);
    assert_eq!(assign(&conn, m2).unwrap(), Assigned::Orphan);
    assert_eq!(collection_of(&conn, m1), None);
    assert_eq!(collection_of(&conn, m2), None);

    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM media_collections", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "正文空白的产物连集合行都不该有 —— 有了就会把不相干的图并成一组");
    crate::infra::testpaths::cleanup(&dir);
}

/// 回填守同一条判据：老库里没分组的空白产物不能被"补"进同一个集合。
#[test]
fn 回填_正文空白的产物不被并组() {
    let (conn, dir) = temp_db("blank-backfill");
    let p = insert_prompt(&conn, "IMAGE", "", None);
    let m1 = insert_media(&conn, Some(p), "IMAGE");
    let m2 = insert_media(&conn, Some(p), "IMAGE");

    let done = backfill(&conn).unwrap();
    assert_eq!(done, 0, "空白正文一条都不该被分进组");
    assert_eq!(collection_of(&conn, m1), None);
    assert_eq!(collection_of(&conn, m2), None);
    crate::infra::testpaths::cleanup(&dir);
}

/// ★ **移出集合之后，重启回填不许把它吸回去**（2026-10-18 手动集合的核心正确性）。
///
/// 只把 `collection_id` 置成 NULL 是**不够**的 —— `backfill` 的判据就是 NULL。
/// 少了 `collection_opt_out`，用户刚拖出去的那张会在下次启动回到原组，**一个错都不报**。
#[test]
fn 移出集合_回填不许再吸回来() {
    let (conn, dir) = temp_db("detach");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let m1 = insert_media(&conn, Some(p), "IMAGE");
    let m2 = insert_media(&conn, Some(p), "IMAGE");
    // 第三张是**刻意**的：只剩两张时移出一张会让原组变成"单成员 auto 组"而被解散
    // （见 §13），那样就测不出"别的成员不受影响"了。
    let m3 = insert_media(&conn, Some(p), "IMAGE");
    let original = group_id(&conn, m1);
    assert_eq!(group_id(&conn, m2), original, "前提：两张在一组");
    assert_eq!(group_id(&conn, m3), original, "前提：三张在一组");

    detach(&conn, m1).unwrap();
    assert_eq!(collection_of(&conn, m1), None, "移出就是没有归属");
    assert_eq!(collection_of(&conn, m2), Some(original), "同组别的成员不受影响");

    // 模拟"重启"：回填再跑一遍
    let again = backfill(&conn).unwrap();
    assert_eq!(again, 0, "移出去的那条不该被重算回来");
    assert_eq!(
        collection_of(&conn, m1),
        None,
        "★ 回填不许把用户移出的产物吸回原组（2026-10-18 的陷阱）"
    );
    crate::infra::testpaths::cleanup(&dir);
}

/// 移出之后原集合不留点不开的空格子：成员搬光了要删掉；只剩一张的 `auto` 组也要解散
/// （后者是 2026-10-18 补的，见 §13 —— `prune_singleton_auto` 一条语句把两件事都做了）。
#[test]
fn 移出集合_原集合空了就消失() {
    let (conn, dir) = temp_db("detach-prune");
    let p1 = insert_prompt(&conn, "IMAGE", "1girl", None);
    let p2 = insert_prompt(&conn, "IMAGE", "2girls", None);
    let a1 = insert_media(&conn, Some(p1), "IMAGE");
    let a2 = insert_media(&conn, Some(p1), "IMAGE");
    let b = insert_media(&conn, Some(p2), "IMAGE");
    let two = group_id(&conn, a1);
    assert_eq!(group_id(&conn, a2), two, "前提：两张在一组");
    let one = group_id(&conn, b);
    assert_ne!(two, one, "前提：b 自己一组");

    // ① 两张那组移出一张 → 只剩一张 auto → 整组解散
    detach(&conn, a1).unwrap();
    assert_eq!(
        collection_of(&conn, a2),
        None,
        "★ 只剩一张的 auto 组也要解散（那张回到散图）"
    );
    // ② 一张那组移出唯一成员 → 变空壳 → 删掉
    detach(&conn, b).unwrap();
    assert_eq!(collection_of(&conn, b), None);

    let left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM media_collections WHERE id IN (?1, ?2)",
            params![two, one],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "空壳与单成员 auto 集合都不许留（否则界面上是点不开的空格子）");
    crate::infra::testpaths::cleanup(&dir);
}

/// **手上不许混 kind**（用户 2026-10-18 拍板："手动集合不允许混图像和视频"）。
#[test]
fn 移进集合_kind不同要明确拒绝() {
    let (conn, dir) = temp_db("move-kind");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let image = insert_media(&conn, Some(p), "IMAGE");
    conn.execute(
        "INSERT INTO media_collections (group_key, kind, origin, rule_version)
         VALUES ('manual:v', 'VIDEO', 'manual', 1)",
        [],
    )
    .unwrap();
    let videos = conn.last_insert_rowid();

    let err = move_into(&conn, image, videos).unwrap_err();
    assert!(
        matches!(err, WriteError::Validation(_)),
        "混 kind 必须是**明确拒绝**（400 说人话），不是静默不动：{err:?}"
    );
    assert_eq!(collection_of(&conn, image), None, "拒绝之后归属一个字都不许动");
    crate::infra::testpaths::cleanup(&dir);
}

/// 移进来要**清掉**"已移出"标记：用户改主意了，回填该重新管它。
#[test]
fn 移进集合_会清掉移出标记() {
    let (conn, dir) = temp_db("move-clear-optout");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let m1 = insert_media(&conn, Some(p), "IMAGE");
    let m2 = insert_media(&conn, Some(p), "IMAGE");
    detach(&conn, m1).unwrap();

    move_into(&conn, m1, group_id(&conn, m2)).unwrap();
    assert_eq!(collection_of(&conn, m1), collection_of(&conn, m2));
    let opt_out: i64 = conn
        .query_row(
            "SELECT collection_opt_out FROM media_assets WHERE id = ?1",
            params![m1],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(opt_out, 0, "移进来之后就不再是「用户已移出」了");
    crate::infra::testpaths::cleanup(&dir);
}

/// 新建手动集合：成员都挂上去、老集合被搬空就消失、名字存得住。
#[test]
fn 新建手动集合_成员挂上去且老集合收拾干净() {
    let (conn, dir) = temp_db("create-manual");
    let p1 = insert_prompt(&conn, "IMAGE", "1girl", None);
    let p2 = insert_prompt(&conn, "IMAGE", "2girls", None);
    let a = insert_media(&conn, Some(p1), "IMAGE");
    let b = insert_media(&conn, Some(p2), "IMAGE");
    let old_a = group_id(&conn, a);
    let old_b = group_id(&conn, b);
    assert_ne!(old_a, old_b, "前提：两张原本在不同组");

    let created = create_manual(&conn, &[a, b], Some("  我的精选  ")).unwrap();
    assert_eq!(collection_of(&conn, a), Some(created));
    assert_eq!(collection_of(&conn, b), Some(created));

    // 两个老集合都被搬空 → 自动消失（不留点不开的空格子）
    let left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM media_collections WHERE id IN (?1, ?2)",
            params![old_a, old_b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "被搬空的老集合必须消失");

    let (origin, title): (String, Option<String>) = conn
        .query_row(
            "SELECT origin, title FROM media_collections WHERE id = ?1",
            params![created],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(origin, "manual");
    assert_eq!(title.as_deref(), Some("我的精选"), "名字首尾空白要去掉");
    crate::infra::testpaths::cleanup(&dir);
}

/// 手动集合的 key 必须以 `manual:` 开头 —— 否则它会被 [`attach`] 当成"算法那一组"，
/// 新图就会源源不断地吸进用户自己建的组里。
#[test]
fn 新建手动集合的key不会被新图吸走() {
    let (conn, dir) = temp_db("manual-key");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let a = insert_media(&conn, Some(p), "IMAGE");
    let created = create_manual(&conn, &[a], None).unwrap();

    let key: String = conn
        .query_row(
            "SELECT group_key FROM media_collections WHERE id = ?1",
            params![created],
            |r| r.get(0),
        )
        .unwrap();
    assert!(key.starts_with("manual:"), "手动集合的 key 必须自成一档：{key}");

    // 同款提示词的新图该进算法那一组，**不许**进用户建的这个
    let fresh = insert_media(&conn, Some(p), "IMAGE");
    let auto = group_id(&conn, fresh);
    assert_ne!(auto, created, "新图不许被吸进手动集合");
    assert_eq!(collection_of(&conn, fresh), Some(auto));
    crate::infra::testpaths::cleanup(&dir);
}

/// 混 kind 的新建要**明确拒绝**（用户 2026-10-18："手动集合不允许混图像和视频"）。
#[test]
fn 新建手动集合_混kind要拒绝() {
    let (conn, dir) = temp_db("create-mixed");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let image = insert_media(&conn, Some(p), "IMAGE");
    let video = insert_media(&conn, Some(p), "VIDEO");

    let err = create_manual(&conn, &[image, video], None).unwrap_err();
    assert!(matches!(err, WriteError::Validation(_)), "必须是 400 说人话：{err:?}");
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM media_collections", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "拒绝之后连空集合都不该留下");
    crate::infra::testpaths::cleanup(&dir);
}

/// ★ **把图移进一个算法分组 → 那一组就地转成手动**。
///
/// 不翻的话：`origin` 还是 `auto`，而它的 `group_key` 仍然等于"算法那一组"的指纹，
/// 于是 [`attach`] 会继续把同款提示词的新图往里吸 —— 用户会看到自己整理过的组
/// 越来越杂，而且**没有任何提示**。
#[test]
fn 移进算法分组_那一组就地转成手动() {
    let (conn, dir) = temp_db("promote-manual");
    let p1 = insert_prompt(&conn, "IMAGE", "1girl", None);
    let p2 = insert_prompt(&conn, "IMAGE", "2girls", None);
    let a = insert_media(&conn, Some(p1), "IMAGE");
    let b = insert_media(&conn, Some(p2), "IMAGE");
    let auto = group_id(&conn, a);
    assert_eq!(
        conn.query_row("SELECT origin FROM media_collections WHERE id = ?1", params![auto], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "auto",
        "前提：它是算法分出来的"
    );

    move_into(&conn, b, auto).unwrap();
    assert_eq!(collection_of(&conn, b), Some(auto));
    assert_eq!(
        conn.query_row("SELECT origin FROM media_collections WHERE id = ?1", params![auto], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "manual",
        "★ 用户碰过的组必须转成手动，否则它还会继续吸新图"
    );
    crate::infra::testpaths::cleanup(&dir);
}

/// 清单：**手动在前**，每条带 kind / origin / size / title。
#[test]
fn 列集合_手动排在算法前面且带成员数() {
    let (conn, dir) = temp_db("list-collections");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let a = insert_media(&conn, Some(p), "IMAGE");
    let b = insert_media(&conn, Some(p), "IMAGE");
    // 第三张同款：搬走 a 之后算法那一组还剩 b、c 两张，组才留得住
    // （只剩一张的 auto 组会被解散，见 §13）
    let c = insert_media(&conn, Some(p), "IMAGE");
    let auto = group_id(&conn, a);
    assert_eq!(group_id(&conn, b), auto);
    assert_eq!(group_id(&conn, c), auto);
    let manual = create_manual(&conn, &[a], Some("精选")).unwrap();

    let rows = list(&conn).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, manual, "手动集合排第一");
    assert_eq!(rows[0].title.as_deref(), Some("精选"));
    assert_eq!(rows[0].kind, "IMAGE");
    assert_eq!(rows[1].id, auto);
    assert_eq!(rows[1].title, None, "算法分出来的没有名字");
    assert_eq!(rows[1].size, 2, "b、c 还在算法那一组里");
    crate::infra::testpaths::cleanup(&dir);
}

#[test]
fn 落库_优先挂到算法那一组而不是用户拆出来的那组() {
    let (conn, dir) = temp_db("prefer-auto");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let m1 = insert_media(&conn, Some(p), "IMAGE");
    let auto_id = group_id(&conn, m1);

    // 用户"拆开"：手造一个 manual 集合，key 与 auto 的一样
    let key: String = conn
        .query_row(
            "SELECT group_key FROM media_collections WHERE id = ?1",
            params![auto_id],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO media_collections (group_key, kind, origin, rule_version)
         VALUES (?1, 'IMAGE', 'manual', ?2)",
        params![key, RULE_VERSION],
    )
    .unwrap();
    let manual_id = conn.last_insert_rowid();
    assert!(manual_id > auto_id);

    // 新产物该进 auto 那一组（不是后建的 manual）
    let m2 = insert_media(&conn, Some(p), "IMAGE");
    assert_eq!(
        group_id(&conn, m2),
        auto_id,
        "用户拆出来的组不该继续吸新图"
    );
    crate::infra::testpaths::cleanup(&dir);
}

#[test]
fn 回填_只补没分组的_手动拆开的不动() {
    let (conn, dir) = temp_db("backfill");
    let p1 = insert_prompt(&conn, "IMAGE", "1girl", None);
    let p2 = insert_prompt(&conn, "IMAGE", "2girls", None);
    // 三张都还没分组（模拟老库：列刚补上，全是 NULL）
    let m1 = insert_media(&conn, Some(p1), "IMAGE");
    let m2 = insert_media(&conn, Some(p1), "IMAGE");
    let m3 = insert_media(&conn, Some(p2), "IMAGE");
    // 再加一张孤儿：回填**不许**给它分组
    let orphan = insert_media(&conn, None, "IMAGE");

    let done = backfill(&conn).unwrap();
    assert_eq!(
        done, 2,
        "孤儿不算；m3 是独苗（同款只有它一张）也不成组，见 `独苗不建组`"
    );
    assert_eq!(collection_of(&conn, m1), collection_of(&conn, m2));
    assert_ne!(collection_of(&conn, m1), collection_of(&conn, m3));
    assert_eq!(
        collection_of(&conn, m3),
        None,
        "独苗留在散图：不为它建一个没意义的单成员组"
    );
    assert_eq!(collection_of(&conn, orphan), None, "孤儿不该被分组");

    // 用户把 m2 挪到另一个集合（模拟"拆开"）：再跑一次回填，它必须**不动**
    let detached = group_id(&conn, m3);
    conn.execute(
        "UPDATE media_assets SET collection_id = ?1 WHERE id = ?2",
        params![detached, m2],
    )
    .unwrap();
    let again = backfill(&conn).unwrap();
    assert_eq!(again, 0, "没有待分组的行了（孤儿不算）");
    assert_eq!(
        collection_of(&conn, m2),
        Some(detached),
        "回填不许覆盖用户已经定下来的分组"
    );
    crate::infra::testpaths::cleanup(&dir);
}

/// ★ **独苗不建组**（2026-10-18）：同款只有一张时**压根不建**集合行，
/// 而不是"先建一个再解散"（后者每次启动都要白跑一趟，日志还会跟着响）。
///
/// ⚠ 而且**不许**为了"别再建"去写 `collection_opt_out = 1` —— 那语义是
/// "用户明确移出"，会把将来同款再来一张时的归并永久挡住。
#[test]
fn 独苗不建组() {
    let (conn, dir) = temp_db("lone-no-group");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let lone = insert_media(&conn, Some(p), "IMAGE");

    assert_eq!(backfill(&conn).unwrap(), 0, "独苗不算进回填数");
    assert_eq!(collection_of(&conn, lone), None, "★ 独苗留在散图");
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM media_collections", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "★ 一张都不该建（连空壳都不留）");
    let opt_out: i64 = conn
        .query_row(
            "SELECT collection_opt_out FROM media_assets WHERE id = ?1",
            params![lone],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(opt_out, 0, "独苗不是「用户移出」，不许顺手写这个标记");

    // 幂等：再跑一次还是什么都不做
    assert_eq!(backfill(&conn).unwrap(), 0);
    crate::infra::testpaths::cleanup(&dir);
}

/// ★ **探针与回填同口径**：探针报的"几组 / 几张"就是回填马上要做的事。
///
/// 两边各写一遍分桶就会漂（探针说有活、回填却什么都没干），
/// 而启动日志正是拿探针的数字说话 —— 漂了就是天天报假活。
#[test]
fn 探针与回填同口径() {
    let (conn, dir) = temp_db("pending-grouped");
    let p1 = insert_prompt(&conn, "IMAGE", "1girl", None);
    let p2 = insert_prompt(&conn, "IMAGE", "2girls", None);
    let lone = insert_media(&conn, Some(p2), "IMAGE");
    let a = insert_media(&conn, Some(p1), "IMAGE");
    let b = insert_media(&conn, Some(p1), "IMAGE");
    // 孤儿 / 已移出的都不算活
    let orphan = insert_media(&conn, None, "IMAGE");
    let opted_out = insert_media(&conn, Some(p1), "IMAGE");
    conn.execute(
        "UPDATE media_assets SET collection_opt_out = 1 WHERE id = ?1",
        params![opted_out],
    )
    .unwrap();

    assert_eq!(
        pending_grouped(&conn).unwrap(),
        (1, 2),
        "只有『1girl』那两张算 1 组 / 2 张（独苗 / 孤儿 / 已移出都不算）"
    );

    assert_eq!(backfill(&conn).unwrap(), 2, "回填做的正是探针说的那 2 张");
    assert_eq!(
        pending_grouped(&conn).unwrap(),
        (0, 0),
        "归并完就没活了 —— 第二次启动因此一行日志都不该打"
    );
    assert_eq!(collection_of(&conn, lone), None);
    assert_eq!(collection_of(&conn, orphan), None);
    assert_eq!(collection_of(&conn, opted_out), None, "已移出的不许被吸回来");
    assert_eq!(collection_of(&conn, a), collection_of(&conn, b));
    crate::infra::testpaths::cleanup(&dir);
}

#[test]
fn 落库_重复调用是幂等的() {
    let (conn, dir) = temp_db("idempotent");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let m = insert_media(&conn, Some(p), "IMAGE");
    let first = group_id(&conn, m);
    for _ in 0..3 {
        assert_eq!(group_id(&conn, m), first);
    }
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM media_collections", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    crate::infra::testpaths::cleanup(&dir);
}

#[test]
fn 落库_产物不在时报未找到而不是瞎建集合() {
    let (conn, dir) = temp_db("missing");
    assert!(matches!(assign(&conn, 999), Err(WriteError::NotFound)));
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM media_collections", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "找不到产物就不该凭空建一个集合");
    crate::infra::testpaths::cleanup(&dir);
}

// ---------------------------------------------------------------------------
//  10. 集合的空与死：删产物之后的收尾（`media_write::delete` 走的就是这里）
// ---------------------------------------------------------------------------
//
// 删除这条入口与 `detach` / `move_into` 共用**同一个**收尾实现
// （`prune_singleton_auto`，见 §13），所以判据必须与 §13 完全一致：
//   * 还有 **>= 2** 个成员 → 集合行必须活着；
//   * `auto` 且**只剩 1 个**成员 → 解散（剩下那张回到散图）；
//   * 一个成员都不剩（含 `manual` 的空壳）→ 删掉，不留点不开的空格子。

/// 还有成员就得活着、空了才消失 —— **`>= 2` 那一档**。
///
/// ⚠ "只剩 1 个成员"那一档**不在这条用例里**（它由同一节的
/// `从两张里删掉一张_单成员自动集合也解散` 盯着）—— 这条只管"没搬空就得活着
/// / 空壳必须消失"。所以这里的算法那一组刻意放**三张**。
#[test]
fn 删产物_集合空了就消失_还有成员就留着() {
    let (conn, dir) = temp_db("prune");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let m1 = insert_media(&conn, Some(p), "IMAGE");
    let m2 = insert_media(&conn, Some(p), "IMAGE");
    // 第三张同款：删掉一张之后还剩两张，集合必须活着（只剩一张时该解散，见 §13）
    let m3 = insert_media(&conn, Some(p), "IMAGE");
    let cid = group_id(&conn, m1);
    assert_eq!(group_id(&conn, m2), cid);
    assert_eq!(group_id(&conn, m3), cid);

    // 删掉一张：还剩两张 → 集合还在。
    // （`media_write::delete` 在同一事务里做的两步：删行 + `prune_singleton_auto`）
    conn.execute("DELETE FROM media_assets WHERE id = ?1", params![m1])
        .unwrap();
    assert_eq!(prune_singleton_auto(&conn).unwrap(), 0, "还剩两张，不该解散");
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM media_collections WHERE id = ?1", params![cid], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 1, "还有成员就不该删集合");
    assert_eq!(collection_of(&conn, m2), Some(cid), "剩下两张的归属也不许动");
    assert_eq!(collection_of(&conn, m3), Some(cid));

    // 一个成员都不剩（这里是用户自己建的手动集合被删空）→ 空壳要消失。
    // 它走的是 `prune_all_empty` 那一步，不该被算进"解散了几个单成员 auto 组"。
    let (mine, lone) = single_manual(&conn, "IMAGE", "2girls", Some("精选"));
    conn.execute("DELETE FROM media_assets WHERE id = ?1", params![lone])
        .unwrap();
    assert_eq!(
        prune_singleton_auto(&conn).unwrap(),
        0,
        "manual 集合不算「单成员 auto 组」"
    );
    let mine_left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM media_collections WHERE id = ?1",
            params![mine],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(mine_left, 0, "删空了的集合必须被带走（否则『删到 0 张还留一个空组』）");
    crate::infra::testpaths::cleanup(&dir);
}

/// ★ **2 张里删掉 1 张 → 单成员 auto 组解散**（用户 2026-10-18 报的症状在**删除**
/// 这条入口上完全一样，只是他先试到的是"移到别的集合"）。
#[test]
fn 从两张里删掉一张_单成员自动集合也解散() {
    let (conn, dir) = temp_db("delete-singleton");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let m1 = insert_media(&conn, Some(p), "IMAGE");
    let m2 = insert_media(&conn, Some(p), "IMAGE");
    let cid = group_id(&conn, m1);
    assert_eq!(group_id(&conn, m2), cid, "前提：两张在一个 auto 集合里");

    // `media_write::delete` 那两步：删行（事务里）+ 收尾
    conn.execute("DELETE FROM media_assets WHERE id = ?1", params![m1])
        .unwrap();
    assert_eq!(prune_singleton_auto(&conn).unwrap(), 1, "解散的就是那一个");

    assert_eq!(
        collection_of(&conn, m2),
        None,
        "★ 剩下那张回到散图（不该再挂在一个没意义的组上）"
    );
    let left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM media_collections WHERE id = ?1",
            params![cid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "★ 只剩一张的 auto 集合必须整行消失");
    crate::infra::testpaths::cleanup(&dir);
}

/// ★ 与上一条**成对**：3 张里删掉 1 张还剩 2 张 → 集合必须好好活着。
///
/// 判据是"**只剩 1 个**成员"，不是"动过就散" —— 少了这条成对的用例，
/// 把收尾写成"只要删过就解散"也能骗过上面那一条。
#[test]
fn 从三张里删掉一张_还剩两张的自动集合不解散() {
    let (conn, dir) = temp_db("delete-two-left");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let m1 = insert_media(&conn, Some(p), "IMAGE");
    let m2 = insert_media(&conn, Some(p), "IMAGE");
    let m3 = insert_media(&conn, Some(p), "IMAGE");
    let cid = group_id(&conn, m1);
    assert_eq!(group_id(&conn, m2), cid, "前提：三张在一个 auto 集合里");
    assert_eq!(group_id(&conn, m3), cid);

    conn.execute("DELETE FROM media_assets WHERE id = ?1", params![m1])
        .unwrap();
    assert_eq!(prune_singleton_auto(&conn).unwrap(), 0, "还剩两张，不解散");

    let left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM media_collections WHERE id = ?1",
            params![cid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 1, "还有两个成员，集合行必须活着");
    assert_eq!(collection_of(&conn, m2), Some(cid), "剩下两张的归属一个字都不许动");
    assert_eq!(collection_of(&conn, m3), Some(cid));
    crate::infra::testpaths::cleanup(&dir);
}

/// ★ **幂等**：库已经收尾过 / 本来就没有该收的东西，再跑一次不报错、也不改任何东西。
///
/// （这条性质原先是 `prune_if_empty` 那条用例钉的 —— 那个函数已删，收尾只剩
/// [`prune_singleton_auto`] 一条路径，性质就跟着它走。）
#[test]
fn 单成员收尾_对空库与已收尾的库都不报错() {
    let (conn, dir) = temp_db("prune-idempotent");
    // 空库：没有集合、也没有产物
    assert_eq!(prune_singleton_auto(&conn).unwrap(), 0);
    assert_eq!(prune_all_empty(&conn).unwrap(), 0);

    // 造一条"入库那条路刚建出来的"单成员 auto 组（`assign` 不建 ≥2 的门槛）
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let m = insert_media(&conn, Some(p), "IMAGE");
    let cid = group_id(&conn, m);
    assert_eq!(prune_singleton_auto(&conn).unwrap(), 1, "第一次把它解散");
    assert_eq!(collection_of(&conn, m), None);
    assert_eq!(
        prune_singleton_auto(&conn).unwrap(),
        0,
        "第二次无事可做：幂等，而且集合行早就不在了也不许报错"
    );
    assert_eq!(prune_all_empty(&conn).unwrap(), 0);
    let left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM media_collections WHERE id = ?1",
            params![cid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "解散过的集合行不许复活");
    crate::infra::testpaths::cleanup(&dir);
}

// ---------------------------------------------------------------------------
//  11. 预设 ↔ 同款产物的自动关联（2026-10-11）
// ---------------------------------------------------------------------------

#[test]
fn 关联产物_按归一化后的提示词匹配() {
    let (conn, dir) = temp_db("preset-media");
    // 预设（人自己存的）
    let preset = insert_prompt(&conn, "IMAGE", "1girl, cyberpunk", Some("lowres"));
    // 自动捕获的同款（写法不同：带权重、顺序不同）
    let captured = insert_prompt(&conn, "IMAGE", "(1girl:1.2), cyberpunk", Some("(lowres:1.1)"));
    let m = insert_media(&conn, Some(captured), "IMAGE");
    // 一条**不同款**的（不该被关联）
    let other = insert_prompt(&conn, "IMAGE", "chinese ink painting", None);
    let m_other = insert_media(&conn, Some(other), "IMAGE");

    let ids = media_of_preset(&conn, preset).unwrap();
    assert!(ids.contains(&m), "写法不同也要认成同款：{ids:?}");
    assert!(!ids.contains(&m_other), "不同款的不许混进来：{ids:?}");

    // 反方向：从自动捕获那条能找出对应的预设
    let presets = presets_matching(&conn, captured).unwrap();
    assert_eq!(presets, vec![preset], "入库时靠它认出'这条有预设'");
    // 预设自己反查自己不算（它不在"别人"里）
    assert!(!presets_matching(&conn, preset).unwrap().contains(&preset));

    // 类型不同不算同款（图 vs 视频）
    let video = insert_prompt(&conn, "VIDEO", "1girl, cyberpunk", Some("lowres"));
    assert!(
        !media_of_preset(&conn, video).unwrap().contains(&m),
        "视频预设不该关联到图产物"
    );

    crate::infra::testpaths::cleanup(&dir);
}

/// ★ 用**真库原样的数据**钉住（2026-10-11 用户报的"从画廊建的预设显示未关联"）。
///
/// 现场是 `.db\comfyhub.db` 里的三条提示词 + 两条产物：
///   * 543 / 545 = 自动捕获（`source='ComfyUI'`），正文与 544 **逐字相同**；
///   * 544 = 用户在画廊里「存为预设」建的（`source='Manual'`）；
///   * 产物 363 → 543、364 → 545。
///
/// 所以 544 的关联产物必须是**两条** —— 用户就是从其中一张图存的预设。
#[test]
fn 关联产物_真库现场_从画廊建的预设要认到那两条产物() {
    let (conn, dir) = temp_db("preset-gallery-real");
    // 正文按真库抄（权重写法、末尾缺空格都照抄 —— 归一化必须能吃掉这些差别）
    const POSITIVE: &str = "(1girl:1.2), (solo:1.1), (black_hair:1.1), (long_hair:1.1), \
                            (blue_eyes:1.1), (black_dress:1.2), (bare_shoulders:1.1), \
                            (holding_flower:1.2), (flower:1.1), (from_side:1.1), (looking_back:1.2), \
                            (looking_at_viewer:1.1), (parted_lips:1.0), (dress:1.0),(holding:1.0),(ocean:1.0)";

    let captured_a = insert_prompt(&conn, "IMAGE", POSITIVE, None);
    let preset = insert_prompt(&conn, "IMAGE", POSITIVE, None);
    let captured_b = insert_prompt(&conn, "IMAGE", POSITIVE, None);
    // 把它们标成自动捕获 / 手动（`presets_matching` 只看 Manual 那一边）
    conn.execute("UPDATE prompts SET source = 'ComfyUI' WHERE id IN (?1, ?2)", params![captured_a, captured_b])
        .unwrap();
    conn.execute("UPDATE prompts SET source = 'Manual' WHERE id = ?1", params![preset]).unwrap();

    let m1 = insert_media(&conn, Some(captured_a), "IMAGE");
    let m2 = insert_media(&conn, Some(captured_b), "IMAGE");

    let ids = media_of_preset(&conn, preset).unwrap();
    assert_eq!(
        ids.len(),
        2,
        "从画廊存的预设必须认到那两条产物（否则卡片显示「未关联」）：{ids:?}"
    );
    assert!(ids.contains(&m1) && ids.contains(&m2), "{ids:?}");

    // 反方向：自动捕获那条能认出这个预设
    assert_eq!(presets_matching(&conn, captured_a).unwrap(), vec![preset]);

    // 两个判据必须**一致**：`prompt_ids_with_media` 认了它，计数就不能是 0
    // （2026-10-11 的现场就是这两个自己打架：`hasMedia=1` 筛得到、卡片却显示未关联）
    let with_media = prompt_ids_with_media(&conn).unwrap();
    assert!(with_media.contains(&preset), "有关联产物：{with_media:?}");
    assert!(with_media.contains(&captured_a));
    assert!(with_media.contains(&captured_b));

    crate::infra::testpaths::cleanup(&dir);
}

#[test]
fn 关联产物_预设不存在时报未找到() {
    let (conn, dir) = temp_db("preset-media-missing");
    assert!(matches!(media_of_preset(&conn, 999), Err(WriteError::NotFound)));
    assert!(matches!(presets_matching(&conn, 999), Err(WriteError::NotFound)));
    crate::infra::testpaths::cleanup(&dir);
}

#[test]
fn prune_all_empty_一次清干净() {    let (conn, dir) = temp_db("prune-all");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let m = insert_media(&conn, Some(p), "IMAGE");
    group_id(&conn, m);
    conn.execute("UPDATE media_assets SET collection_id = NULL", []).unwrap();
    assert_eq!(prune_all_empty(&conn).unwrap(), 1);
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM media_collections", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
    crate::infra::testpaths::cleanup(&dir);
}

// ---------------------------------------------------------------------------
//  12. 显式「重新自动整理」（2026-10-18）：只重算算法分组
// ---------------------------------------------------------------------------
//
// 这一节盯三条纪律，少一条都会**静默**出错：
//  ① `manual` 集合一个都不动（用户整理好的组被算法合回去是最气人的 bug）；
//  ② `collection_opt_out = 1` 的行不许被吸回来；
//  ③ 已经分好组的 auto 组必须**真的被重算**（`backfill` 自己永远不碰它们）。

/// ★ **手动集合一个都不动**：`regroup_auto` 只解散 `origin='auto'` 的集合。
///
/// 反例（这条用例要拦的）：一个"先把所有集合删掉再回填"的粗暴实现，会把用户刚整理好的
/// 手动集合连带拆散 —— 界面上看起来只是"组没了"，产物一张不少，所以特别难被发现。
#[test]
fn 重新整理_手动集合一个都不动() {
    let (conn, dir) = temp_db("regroup-manual");
    let p_auto = insert_prompt(&conn, "IMAGE", "1girl", None);
    let p_manual = insert_prompt(&conn, "IMAGE", "2girls", None);
    let a = insert_media(&conn, Some(p_auto), "IMAGE");
    // 第二张同款算法产物：重算之后那一组有两张，才不会因为"单成员 auto 组要解散"
    // （§13）而让下面"在某个 auto 集合里"的断言失去意义。
    let a2 = insert_media(&conn, Some(p_auto), "IMAGE");
    let b = insert_media(&conn, Some(p_manual), "IMAGE");

    // 前提：a、a2 在算法分组里；b 被用户收进自己建的手动集合
    let old_auto = group_id(&conn, a);
    assert_eq!(group_id(&conn, a2), old_auto, "前提：两张算法产物在一组");
    let manual = create_manual(&conn, &[b], Some("我的精选")).unwrap();
    assert_eq!(collection_of(&conn, b), Some(manual), "前提：b 属于手动集合");
    assert_ne!(old_auto, manual);

    let done = regroup_auto(&conn).unwrap();
    assert_eq!(done, 2, "只有算法那两张被重分（手动集合的成员不算）");

    assert_eq!(
        collection_of(&conn, b),
        Some(manual),
        "★ 手动集合的成员归属一个字都不许动（用户整理好的组不许被算法合回去）"
    );
    // 手动集合行本身还在，连同用户起的名字
    let (origin, title): (String, Option<String>) = conn
        .query_row(
            "SELECT origin, title FROM media_collections WHERE id = ?1",
            params![manual],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(origin, "manual");
    assert_eq!(title.as_deref(), Some("我的精选"), "手动集合的名字也不许丢");

    // 算法那张是"先解散、再按当前规则重建"：还该在某个 auto 集合里，但不是老那一条
    let regrouped = collection_of(&conn, a).expect("算法分组重算之后仍该在组里");
    assert_ne!(regrouped, old_auto, "auto 集合是解散后重建的，不该复用老行");
    let regrouped_origin: String = conn
        .query_row(
            "SELECT origin FROM media_collections WHERE id = ?1",
            params![regrouped],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(regrouped_origin, "auto");
    crate::infra::testpaths::cleanup(&dir);
}

/// ★ **已移出的不被吸回来**：解散只动 `collection_id`，`collection_opt_out` 原样保留，
/// 于是 `backfill` 的 `AND collection_opt_out = 0` 仍然挡着它。
///
/// 反例：解散时顺手把 `collection_opt_out` 清成 0（或者干脆重建一条 media 行），
/// 用户"我明明移出去了"的那张会在点一次「重新自动整理」之后悄悄回到原组。
#[test]
fn 重新整理_已移出的不被吸回来() {
    let (conn, dir) = temp_db("regroup-optout");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let m1 = insert_media(&conn, Some(p), "IMAGE");
    let m2 = insert_media(&conn, Some(p), "IMAGE");
    // 第三张：m1 移出之后原组还剩 m2、m3 两张，才留得住（单成员 auto 组会解散，见 §13）
    let m3 = insert_media(&conn, Some(p), "IMAGE");
    let original = group_id(&conn, m1);
    assert_eq!(group_id(&conn, m2), original, "前提：两张在一组");
    assert_eq!(group_id(&conn, m3), original, "前提：三张在一组");

    detach(&conn, m1).unwrap();
    assert_eq!(collection_of(&conn, m1), None, "前提：用户明确移出了 m1");

    let done = regroup_auto(&conn).unwrap();
    assert_eq!(done, 2, "已移出的那条不算进重分条数");
    assert_eq!(
        collection_of(&conn, m1),
        None,
        "★ 重新整理也不许把用户移出的产物吸回来"
    );
    let opt_out: i64 = conn
        .query_row(
            "SELECT collection_opt_out FROM media_assets WHERE id = ?1",
            params![m1],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(opt_out, 1, "解散 auto 不许顺手清掉「用户已移出」标记");
    assert!(
        collection_of(&conn, m2).is_some(),
        "同组剩下那张该按当前规则重新进组"
    );
    crate::infra::testpaths::cleanup(&dir);
}

/// ★ **已经分好组的也要按当前规则重算**（这是与 [`backfill`] 的唯一区别）。
///
/// 现场造法：同款提示词的两张产物被老规则分在**两个** auto 集合里。`backfill` 的判据是
/// `collection_id IS NULL`，所以它一条都不会碰（这正是用户说的"点了没反应"）；
/// `regroup_auto` 先把两条 auto 集合解散，再重分 —— 必须并回同一组，且老集合行不在了。
#[test]
fn 重新整理_把已分组的按当前规则重算() {
    let (conn, dir) = temp_db("regroup-recompute");
    // 同款提示词（写法不同，归一化后相同）→ 当前规则必须判成同一组
    let p1 = insert_prompt(&conn, "IMAGE", "(masterpiece:1.2), 1girl", None);
    let p2 = insert_prompt(&conn, "IMAGE", "1girl, masterpiece", None);
    let m1 = insert_media(&conn, Some(p1), "IMAGE");
    let m2 = insert_media(&conn, Some(p2), "IMAGE");

    // 手工造出"老规则分出来的两个组"：两条 auto 集合，各挂一张，rule_version 停在 0。
    let mut stale = Vec::new();
    for (key, media_id) in [("stale:a", m1), ("stale:b", m2)] {
        conn.execute(
            "INSERT INTO media_collections (group_key, kind, origin, rule_version)
             VALUES (?1, 'IMAGE', 'auto', 0)",
            params![key],
        )
        .unwrap();
        let cid = conn.last_insert_rowid();
        conn.execute(
            "UPDATE media_assets SET collection_id = ?1 WHERE id = ?2",
            params![cid, media_id],
        )
        .unwrap();
        stale.push(cid);
    }
    assert_ne!(
        collection_of(&conn, m1),
        collection_of(&conn, m2),
        "前提：老库里这两张被分在两个组"
    );
    assert_eq!(backfill(&conn).unwrap(), 0, "前提：都已分组，backfill 一条都不碰");

    let done = regroup_auto(&conn).unwrap();
    assert_eq!(done, 2, "两张都已分组，也该被重算进来");

    let merged = collection_of(&conn, m1).expect("重算之后该进组");
    assert_eq!(
        collection_of(&conn, m2),
        Some(merged),
        "★ 同款提示词必须回到同一组（老规则拆开的两组要被合并）"
    );
    // 老的两条 auto 集合行必须消失，全库只剩新建的那一条 —— 空集合一个都不留
    let stale_left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM media_collections WHERE id IN (?1, ?2)",
            params![stale[0], stale[1]],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale_left, 0, "解散的 auto 集合行必须被删掉");
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM media_collections", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "只剩重算后新成的那一组（不留点不开的空格子）");
    crate::infra::testpaths::cleanup(&dir);
}

// ---------------------------------------------------------------------------
//  13. 单成员集合：`auto` 只剩一张要解散，`manual` 只剩一张要留着（2026-10-18）
// ---------------------------------------------------------------------------
//
// 用户报的 bug 原话："将内容从一个集合调到另一个集合的时候，如果旧集合元素为 1，
// 旧集合没有自动解散。" 一个组只有一张图没有意义（界面上却仍然是叠放封面 + 数量角标，
// 点进去只有一张），该把那最后一张放回散图、删掉集合行。
//
// 两条纪律（少一条都会**静默**出错，而且错法相反）：
//  ① `auto` 只剩 1 个成员 → 必须解散。三个入口各自负责一段：
//     * **写路径**（移出 / 移进 / 新建手动集合 / 删产物）当场收尾（`prune_singleton_auto`）；
//     * **启动**收尾负责历史遗留与半截归并（`backfill` 自己不碰已分组的行）；
//     * **归并 / 重新整理**则**压根不建**单成员组（只给"同款 >= 2 张"的桶建组），
//       所以它们跑完也不会冒出来 —— 不是"建了再拆"。
//  ② `manual` 只剩 1 个成员 → **一个字都不许动**（那是用户自己建的，可能还改了标题，
//     解散它等于删用户的东西）；
//  ⚠ 而且**入库那条路（`assign`）不能收尾**：一批同款图是一条条进来的，中途每条都是
//     单成员组，当场解散会让它们永远并不到一起。

/// 造一个"用户自己建的"单成员集合（`manual`），返回 (集合 id, 成员 id)。
fn single_manual(conn: &Connection, kind: &str, pos: &str, title: Option<&str>) -> (i64, i64) {
    let p = insert_prompt(conn, kind, pos, None);
    let m = insert_media(conn, Some(p), kind);
    let c = create_manual(conn, &[m], title).unwrap();
    (c, m)
}

/// ★ **搬走一张之后，旧集合只剩一张就自动解散**（用户 2026-10-18 报的 bug）。
///
/// 现场：2 张同款图在一个 `auto` 集合里，用户把其中一张移到别的集合 —— 旧集合只剩 1 张，
/// 界面上却还是一个叠放封面 + 数量角标、点进去只有一张。该把那最后一张放回散图
/// （`collection_id = NULL`）并删掉集合行。
#[test]
fn 搬走一张之后_旧集合只剩一张就自动解散() {
    let (conn, dir) = temp_db("singleton-move");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let m1 = insert_media(&conn, Some(p), "IMAGE");
    let m2 = insert_media(&conn, Some(p), "IMAGE");
    let old = group_id(&conn, m1);
    assert_eq!(group_id(&conn, m2), old, "前提：两张在同一个 auto 集合里");

    // 目标：另一个集合（模拟界面上的"移到集合…"）
    let (target, _t) = single_manual(&conn, "IMAGE", "2girls", None);
    assert_ne!(target, old);

    move_into(&conn, m1, target).unwrap();

    assert_eq!(collection_of(&conn, m1), Some(target), "搬走的那张进了新集合");
    assert_eq!(
        collection_of(&conn, m2),
        None,
        "★ 旧集合只剩一张 → 那张回到散图（不该再挂在一个没意义的组上）"
    );
    let left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM media_collections WHERE id = ?1",
            params![old],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "★ 只剩一张的 auto 集合必须整行消失");
    crate::infra::testpaths::cleanup(&dir);
}

/// ★ **只剩一张的 `manual` 集合不解散**（硬纪律）。
///
/// `manual` 是用户自己建的（甚至改了标题）—— 解散它等于删用户的东西。
/// 反例（这条用例要拦的）：收尾时只判"成员数 == 1"不看 `origin`，
/// 用户刚整理好的单人组会无声无息地散掉，产物一张不少，所以特别难被发现。
#[test]
fn 只剩一张的_手动集合不解散() {
    let (conn, dir) = temp_db("singleton-manual");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let m1 = insert_media(&conn, Some(p), "IMAGE");
    let m2 = insert_media(&conn, Some(p), "IMAGE");
    // 用户自己建的集合，两张都在里面
    let mine = create_manual(&conn, &[m1, m2], Some("我的精选")).unwrap();

    // 把 m2 挪到别处 → `mine` 只剩 1 个成员（但它是 manual）
    let (other, _x) = single_manual(&conn, "IMAGE", "2girls", None);
    move_into(&conn, m2, other).unwrap();
    // 写路径里 `move_into` / `detach` 已经调过收尾，这里再显式跑一遍（幂等）
    assert_eq!(
        prune_singleton_auto(&conn).unwrap(),
        0,
        "manual 集合不该被算进解散数"
    );

    assert_eq!(
        collection_of(&conn, m1),
        Some(mine),
        "★ 手动集合只剩一张也要原样留着（解散它等于删用户的东西）"
    );
    let (origin, title): (String, Option<String>) = conn
        .query_row(
            "SELECT origin, title FROM media_collections WHERE id = ?1",
            params![mine],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(origin, "manual");
    assert_eq!(title.as_deref(), Some("我的精选"), "用户起的名字也不许丢");
    crate::infra::testpaths::cleanup(&dir);
}

/// ★ **重新整理之后，单成员 `auto` 集合一个都不留**（历史数据靠这一条收干净）。
///
/// 现场造法：老库里已经存在一个"只有 1 个成员"的 auto 集合（旧版本留下的）。
/// `backfill` 一条都不会碰它（成员的 `collection_id` 不是 NULL），
/// 收干净它的是 `regroup_auto` 的第 ① 步（**DELETE 整个 `origin='auto'`**）；
/// 而重算那一遍**只会给"同款 >= 2 张"的桶建组**（[`backfill`] 纪律 3），
/// 孤零零那张因此直接留在散图里 —— 不是"建了再解散"。
///
/// ⚠ 这条用例是那个**不变式**的正面防线：`regroup_auto` 在 2026-10-18 之后
/// **没有**收尾调用了（推演见它的纪律 4）。谁要是把"只建 >= 2 的桶"这条件改掉，
/// 这里会立刻红 —— 而不是被一次防御性收尾悄悄兜住。
#[test]
fn 重新整理之后_单成员自动集合也不留() {
    let (conn, dir) = temp_db("regroup-singleton");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let m = insert_media(&conn, Some(p), "IMAGE");
    // 手造历史数据：一条 auto 集合，只有 1 个成员
    conn.execute(
        "INSERT INTO media_collections (group_key, kind, origin, rule_version)
         VALUES ('stale:single', 'IMAGE', 'auto', 0)",
        [],
    )
    .unwrap();
    let stale = conn.last_insert_rowid();
    conn.execute(
        "UPDATE media_assets SET collection_id = ?1 WHERE id = ?2",
        params![stale, m],
    )
    .unwrap();
    assert_eq!(backfill(&conn).unwrap(), 0, "前提：已分组，backfill 不碰它");

    regroup_auto(&conn).unwrap();

    // 判据：全库里不存在"origin='auto' 且只有 1 个成员"的集合
    let singles: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM media_collections WHERE origin = 'auto'
               AND id IN (SELECT collection_id FROM media_assets
                           GROUP BY collection_id HAVING COUNT(*) = 1)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(singles, 0, "★ 单成员自动集合一个都不许留");
    assert_eq!(collection_of(&conn, m), None, "那张回到散图");
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM media_collections", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0,
        "连集合行都不该剩（不留点不开的空格子）"
    );
    crate::infra::testpaths::cleanup(&dir);
}

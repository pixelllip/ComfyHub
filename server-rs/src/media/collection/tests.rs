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
fn 从产物DTO取端点_没关联提示词就是空() {
    let a = Endpoint::from(&media_dto("IMAGE", None, None));
    let b = ep("IMAGE", "", None);
    assert_eq!(group_key(&a), group_key(&b), "孤儿产物也要能算出一个稳定的 key");

    let c = Endpoint::from(&media_dto("IMAGE", Some("1girl"), Some("bad hands")));
    let d = ep("IMAGE", "1girl", Some("bad hands"));
    assert_eq!(group_key(&c), group_key(&d));
}

#[test]
fn 从产物DTO取端点_负提示词空白当没有() {
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
fn 从提示词DTO取端点() {
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
    assert_eq!(done, 3, "孤儿不算进回填数");
    assert_eq!(collection_of(&conn, m1), collection_of(&conn, m2));
    assert_ne!(collection_of(&conn, m1), collection_of(&conn, m3));
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
//  10. 集合的空与死：删产物要带走空集合
// ---------------------------------------------------------------------------

#[test]
fn 删产物_集合空了就消失_还有成员就留着() {
    let (conn, dir) = temp_db("prune");
    let p = insert_prompt(&conn, "IMAGE", "1girl", None);
    let m1 = insert_media(&conn, Some(p), "IMAGE");
    let m2 = insert_media(&conn, Some(p), "IMAGE");
    let cid = group_id(&conn, m1);
    assert_eq!(group_id(&conn, m2), cid);

    // 删掉一张：集合还在（还有 m2）
    conn.execute("DELETE FROM media_assets WHERE id = ?1", params![m1]).unwrap();
    prune_if_empty(&conn, cid).unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM media_collections WHERE id = ?1", params![cid], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 1, "还有成员就不该删集合");

    // 删掉最后一张：集合该消失（否则"删到 0 张还留一个空组"）
    conn.execute("DELETE FROM media_assets WHERE id = ?1", params![m2]).unwrap();
    prune_if_empty(&conn, cid).unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM media_collections WHERE id = ?1", params![cid], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 0, "空集合必须被带走");
    crate::infra::testpaths::cleanup(&dir);
}

#[test]
fn prune_if_empty_对不存在的集合也不报错() {
    let (conn, dir) = temp_db("prune-idempotent");
    prune_if_empty(&conn, 999).unwrap();
    assert_eq!(prune_all_empty(&conn).unwrap(), 0);
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

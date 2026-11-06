//! 同步 API 与跨端：manifest / changes / push / bootstrap、可达地址、凭据上传。
//!
//! 夹具在 [`super::common`]；`use super::common::*;` 之后照旧直接写用例。

use super::common::*;

/// `GET /api/sync/manifest` —— 手机连上来的第一件事（🔒 契约 §1）。
///
/// 最要紧的一条：**带凭据/令牌的表绝不能出现在 `synced: true` 里**。
/// `_sync_log` 的触发器其实覆盖了 `app_settings`，所以"从触发器派生同步范围"这个
/// 看起来很自然的写法会把**配对令牌明文**推给手机。这条用例盯着那份显式白名单。
#[tokio::test]
async fn sync_manifest_lists_scope_and_never_offers_credentials() {
    let state = state();
    let (status, body) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    assert_eq!(status, StatusCode::OK);

    // 形状：契约 §1 的五个键一个都不能少/改名
    assert_eq!(
        keys(&body),
        vec![
            "cursor",
            "kitVersion",
            "schemaVersion",
            "serverTime",
            "tables"
        ],
        "manifest 的键集合就是契约",
    );
    assert_eq!(
        body["schemaVersion"].as_i64(),
        Some(comfyhub_server_rs::infra::db::SCHEMA_VERSION),
        "= PRAGMA user_version（真源是 db.rs 的 SCHEMA_VERSION，别写死数字）"
    );
    // 夹具是**直接 SQL 插的行** —— 触发器照常点亮 `_sync_log`，所以游标已经往前走了。
    // （这正是"游标 = MAX(seq)、不是自己数的行数"的一个活证据）
    let fixture_cursor = body["cursor"].as_i64().unwrap();
    assert!(fixture_cursor > 0, "夹具插过行，_sync_log 就不该是空的");
    let max_seq: i64 = state
        .with(|conn| conn.query_row("SELECT MAX(seq) FROM _sync_log", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(fixture_cursor, max_seq, "游标 = MAX(_sync_log.seq)");
    assert!(
        body["serverTime"].as_str().unwrap().ends_with('Z'),
        "时间一律 ISO-8601 UTC：{}",
        body["serverTime"],
    );

    // 同步表：带 rowCount
    assert_eq!(body["tables"]["prompts"]["synced"], true);
    assert_eq!(body["tables"]["prompts"]["rowCount"].as_i64(), Some(2));
    assert_eq!(body["tables"]["tags"]["rowCount"].as_i64(), Some(2));

    // ★ 安全：令牌/凭据/模型目录一律 synced:false，而且**连行数都不给**
    for dangerous in ["app_settings", "ai_providers", "ai_models", "ai_run_events"] {
        let entry = &body["tables"][dangerous];
        assert_eq!(entry["synced"], false, "{dangerous} 绝不能被当成可同步的表");
        assert!(
            entry["reason"].as_str().is_some_and(|r| !r.is_empty()),
            "{dangerous} 要给理由"
        );
        assert!(
            entry["rowCount"].is_null(),
            "{dangerous} 连行数都不该外发（行数本身也是信息）"
        );
    }

    // kitVersion：Kotlin 侧没写过就是 null —— **不编一个**
    assert!(body["kitVersion"].is_null(), "没写过版本号就得如实回 null");

    // Kotlin 侧写过之后就取得到（键名 `ai.runkit.version` 与 RunKit.KEY_VERSION 同值）
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO app_settings (k, v, updated_at) VALUES ('ai.runkit.version','2026-09-24.1','x')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    let (_, body) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    assert_eq!(body["kitVersion"].as_str(), Some("2026-09-24.1"));
}

/// `cursor` 必须跟着 `_sync_log` 走（手机靠它判断"我的游标还有效吗"）。
#[tokio::test]
async fn sync_manifest_cursor_follows_the_sync_log() {
    let state = state();
    let (_, before_body) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    let before = before_body["cursor"].as_i64().unwrap();

    // 写一行被同步的表 → 触发器点亮 `_sync_log` → 游标必须跟着走
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO tags (name, normalized, category, use_count, created_at)
                 VALUES ('游标用例','游标用例','风格',0,'2026-09-24T00:00:00.000Z')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    let (_, body) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    let cursor = body["cursor"].as_i64().unwrap();
    assert!(
        cursor > before,
        "写过之后游标必须往前走：{before} -> {cursor}"
    );

    // 与账本里的最大值**同源**（不能是"自己数的行数"）
    let max_seq: i64 = state
        .with(|conn| conn.query_row("SELECT MAX(seq) FROM _sync_log", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(cursor, max_seq, "游标 = MAX(_sync_log.seq)");
}

/// `GET /api/sync/changes` —— 增量。契约 §2 的硬约束逐条验。
#[tokio::test]
async fn sync_changes_returns_full_rows_and_scalar_or_array_keys() {
    let state = state();
    let (_, before) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    let since = before["cursor"].as_i64().unwrap();

    // 一张整数主键表 + 一张**TEXT 主键**表 + 一张**复合主键**表
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO tags (name, normalized, category, use_count, created_at)
                 VALUES ('增量A','增量a','风格',0,'2026-09-24T00:00:00.000Z')",
                [],
            )
            .map(|_| ())?;
            conn.execute(
                "INSERT INTO ai_conversations (id,title,archived,created_at,updated_at)
                 VALUES ('conv-uuid-1','增量会话',0,'2026-09-24T00:00:00.000Z','2026-09-24T00:00:00.000Z')",
                [],
            )
            .map(|_| ())?;
            conn.execute("INSERT INTO prompt_tags (prompt_id, tag_id) VALUES (10, 2)", [])
                .map(|_| ())
        })
        .unwrap();

    let (status, body) = get_json(
        router_for(state.clone()),
        &format!("/api/sync/changes?since={since}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        keys(&body),
        vec!["cursor", "deletes", "hasMore", "upserts"],
        "changes 的键集合就是契约",
    );
    assert_eq!(body["hasMore"], false);
    assert!(body["cursor"].as_i64().unwrap() >= since);

    // upserts 给的是**完整行**（不是 diff）：标签那行连 use_count / created_at 都在
    let tag = &body["upserts"]["tags"][0];
    assert_eq!(tag["name"], "增量A");
    assert!(tag["use_count"].is_number(), "完整行要带齐所有列：{tag}");
    assert!(tag["created_at"].is_string(), "完整行要带齐所有列：{tag}");
    // TEXT 主键（UUID）原样是字符串，没有被数字化
    assert_eq!(body["upserts"]["ai_conversations"][0]["id"], "conv-uuid-1");

    // 复合主键的 insert 也会进 upserts，且取得到整行
    assert_eq!(body["upserts"]["prompt_tags"][0]["prompt_id"], 10);
    assert_eq!(body["upserts"]["prompt_tags"][0]["tag_id"], 2);
}

/// **`deletes` 必须独立成组**，且复合主键要发成数组（契约 §2「主键表示」）。
#[tokio::test]
async fn sync_changes_groups_deletes_separately_with_array_keys() {
    let state = state();
    let (_, before) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    let since = before["cursor"].as_i64().unwrap();

    // 删一条单列主键的 + 一条复合主键的
    state
        .with(|conn| {
            conn.execute("DELETE FROM tags WHERE id = 2", [])
                .map(|_| ())?;
            conn.execute(
                "DELETE FROM prompt_tags WHERE prompt_id = 10 AND tag_id = 1",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    let (_, body) = get_json(
        router_for(state.clone()),
        &format!("/api/sync/changes?since={since}"),
    )
    .await;

    // 单列 → 标量数字；绝不能混进 upserts
    assert_eq!(body["deletes"]["tags"][0], 2, "单列主键发标量");
    assert!(
        body["upserts"]["tags"].is_null(),
        "删掉的行不该出现在 upserts 里"
    );
    // 复合 → 数组（不是 '10:1' 字符串）
    //
    // ⚠ 这里**不断言顺序**：删 tag 2 会经外键级联掉 prompt_tags(11,2)，
    // 那条级联删除的账本条目排在显式删 (10,1) 的前面。两条都要在，顺序由 seq 决定。
    let deleted = body["deletes"]["prompt_tags"].as_array().unwrap();
    assert!(
        deleted.contains(&serde_json::json!([10, 1])),
        "显式删的复合主键要发数组：{deleted:?}",
    );
    assert!(
        deleted.contains(&serde_json::json!([11, 2])),
        "★ 外键级联删掉的行也要进 deletes（否则手机永远留着幽灵行）：{deleted:?}",
    );
}

/// ★ 凭据/令牌表**一条都不许出现在同步响应里**。
///
/// 分两层验，两层都是真的：
///
/// ① **schema 现状**：`app_settings` / `ai_providers` 只有 `updated_at` 的 `touch` 触发器，
///    **没有** `_sync_log` 触发器 —— 所以写它们根本不会进账本。
///    （⚠ 别把 `trg_x_touch` 看成 `trg_x_sync_*`：当初就是这么误判过一次。）
/// ② **实现在被"将来扩宽触发器"时的表现**：手工往账本里插一条 `tbl='app_settings'` 的条目
///    （模拟"哪天有人给设置表补了同步触发器"），`changes` **必须**把它挡在门外，
///    而且游标要照常跨过去（否则手机永远推不动）。
#[tokio::test]
async fn sync_changes_never_leaks_settings_or_providers() {
    let state = state();
    let (_, before) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    let since = before["cursor"].as_i64().unwrap();

    // ① 写一行令牌表 + 一行凭据表，再看账本 —— **不该有任何条目**
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO app_settings (k, v, updated_at) VALUES ('auth.pairToken','SECRET-TOKEN','x')",
                [],
            )
            .map(|_| ())?;
            conn.execute(
                "INSERT INTO ai_providers (id,display_name,api,base_url,created_at,updated_at)
                 VALUES ('p-secret','秘密供应商','openai','https://x','2026-09-24T00:00:00.000Z','2026-09-24T00:00:00.000Z')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    let touched: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM _sync_log WHERE seq > ?1 AND tbl IN ('app_settings','ai_providers')",
                [since],
                |r| r.get(0),
            )})
        .unwrap();
    assert_eq!(
        touched, 0,
        "这两张表**不该有** _sync_log 触发器（touch 触发器不是 sync 触发器）",
    );

    // ② 手工注入两条账本条目（模拟将来触发器被扩宽）
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO _sync_log (tbl, pk, op, at) VALUES ('app_settings','auth.pairToken','upsert','x')",
                [],
            )
            .map(|_| ())?;
            conn.execute(
                "INSERT INTO _sync_log (tbl, pk, op, at) VALUES ('ai_providers','p-secret','upsert','x')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    let (status, body) = get_json(
        router_for(state.clone()),
        &format!("/api/sync/changes?since={since}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // ★ 一条都不能漏出去（响应体里连那个令牌字符串都不该出现）
    let raw = body.to_string();
    assert!(
        !raw.contains("SECRET-TOKEN"),
        "配对令牌明文绝不能出现在同步响应里：{raw}"
    );
    assert!(
        body["upserts"]["app_settings"].is_null(),
        "app_settings 不同步"
    );
    assert!(
        body["upserts"]["ai_providers"].is_null(),
        "ai_providers 不同步"
    );
    assert!(body["deletes"]["app_settings"].is_null());
    assert!(body["deletes"]["ai_providers"].is_null());

    // 但游标要能跨过这些**不属于同步范围**的条目（否则手机永远推不动）
    let cursor = body["cursor"].as_i64().unwrap();
    let max_seq: i64 = state
        .with(|conn| conn.query_row("SELECT MAX(seq) FROM _sync_log", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(cursor, max_seq, "扫完了就跳到账本末尾，跨过不同步的条目");
}

/// ★ **游标落在保留窗口之外 → `410 Gone`**（契约 §2：这个分支不是可选项）。
///
/// 缺了它的表现是"手机少了几百条，而且**永远补不回来**" ——
/// 增量接口照着旧游标往后扫，永远扫不到被裁掉的那些洞。
#[tokio::test]
async fn sync_changes_returns_410_when_the_cursor_fell_out_of_the_window() {
    let state = state();

    // 模拟日志裁剪：把账本清空，再写一行 → 此时 MIN(seq) 已经是个很大的数
    state
        .with(|conn| {
            conn.execute("DELETE FROM _sync_log", []).map(|_| ())?;
            conn.execute(
                "INSERT INTO tags (name, normalized, category, use_count, created_at)
                 VALUES ('裁剪后','裁剪后','风格',0,'2026-09-24T00:00:00.000Z')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    // 老游标（1）早于窗口 → 410
    let (status, body) = get_json(router_for(state.clone()), "/api/sync/changes?since=1").await;
    assert_eq!(
        status,
        StatusCode::GONE,
        "窗口外的游标必须 410，实际 {status} {body}"
    );
    // 410 的信封与别处一致：`error` 是**人话**（手机判 410 认的是状态码，不是 `error` 的值）
    assert_eq!(keys(&body), vec!["error"]);
    assert!(
        body["error"].as_str().unwrap().contains("bootstrap"),
        "错误信息要告诉手机改走 bootstrap：{body}",
    );

    // 贴着窗口左沿（min - 1）是**安全**的，不该 410
    let min: i64 = state
        .with(|conn| conn.query_row("SELECT MIN(seq) FROM _sync_log", [], |r| r.get(0)))
        .unwrap();
    let (status, _) = get_json(
        router_for(state.clone()),
        &format!("/api/sync/changes?since={}", min - 1),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "刚好贴着左沿应当正常返回（不多报一次全量）"
    );

    // 空账本不该 410（新手机本来就没有游标）
    state
        .with(|conn| conn.execute("DELETE FROM _sync_log", []).map(|_| ()))
        .unwrap();
    let (status, body) = get_json(router_for(state.clone()), "/api/sync/changes?since=99999").await;
    assert_eq!(status, StatusCode::OK, "空账本不 410：{body}");
    assert_eq!(body["hasMore"], false);
}

/// 分页：`hasMore: true` 时游标只能停在**扫到的最后一条**（手机必须接着拉）。
#[tokio::test]
async fn sync_changes_paginates_and_advances_the_cursor() {
    let state = state();
    let (_, before) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    let mut cursor = before["cursor"].as_i64().unwrap();

    for i in 0..5 {
        state
            .with(|conn| {
                conn.execute(
                    "INSERT INTO tags (name, normalized, category, use_count, created_at)
                     VALUES (?1, ?1, '风格', 0, '2026-09-24T00:00:00.000Z')",
                    [format!("分页{i}")],
                )
                .map(|_| ())
            })
            .unwrap();
    }

    // limit=2：第一页应当 hasMore=true，且游标**没有**跳到账本末尾
    let (_, page1) = get_json(
        router_for(state.clone()),
        &format!("/api/sync/changes?since={cursor}&limit=2"),
    )
    .await;
    assert_eq!(page1["hasMore"], true, "还有下一页");
    let c1 = page1["cursor"].as_i64().unwrap();
    assert!(c1 > cursor, "游标要往前走");
    let max_seq: i64 = state
        .with(|conn| conn.query_row("SELECT MAX(seq) FROM _sync_log", [], |r| r.get(0)))
        .unwrap();
    assert!(
        c1 < max_seq,
        "没扫完就不能跳到末尾（否则中间那几条永远丢）：c1={c1} max={max_seq}"
    );
    assert_eq!(
        page1["upserts"]["tags"].as_array().unwrap().len(),
        2,
        "一页就是 limit 条"
    );

    // 接着拉，最后把所有标签都收全（一页都不许漏）
    cursor = c1;
    let mut seen = page1["upserts"]["tags"].as_array().unwrap().len();
    for _ in 0..10 {
        let (_, page) = get_json(
            router_for(state.clone()),
            &format!("/api/sync/changes?since={cursor}&limit=2"),
        )
        .await;
        seen += page["upserts"]["tags"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);
        cursor = page["cursor"].as_i64().unwrap();
        if page["hasMore"] == false {
            break;
        }
    }
    assert_eq!(
        seen, 5,
        "翻完所有页要正好收到 5 条（多一条/少一条都是 bug）"
    );
}

/// ★ **M4 的 DoD：手机产生的会话在桌面可见；重复推送不产生重复行。**
#[tokio::test]
async fn sync_push_applies_and_is_idempotent() {
    let state = state();
    let item = push_item(
        "ai_conversation",
        "conv-phone-1",
        conv_payload("conv-phone-1", "手机上起的会话", "2026-09-24T10:00:00.000Z"),
    );

    // 第一次：applied
    let (status, body) = push(&state, vec![item.clone()]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["applied"], serde_json::json!(["conv-phone-1"]));
    assert_eq!(body["rejected"], serde_json::json!([]));

    // 桌面看得见（这是 M4 DoD 的前半句）
    let (status, row) = get_json(router_for(state.clone()), "/api/sync/changes?since=0").await;
    assert_eq!(status, StatusCode::OK);
    let titles: Vec<&str> = row["upserts"]["ai_conversations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["title"].as_str())
        .collect();
    assert!(
        titles.contains(&"手机上起的会话"),
        "上行之后的会话应当在同步流里：{titles:?}"
    );

    // ★ 第二次推**同一条**：不许产生重复行
    let (status, body) = push(&state, vec![item.clone()]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["applied"],
        serde_json::json!([]),
        "第二次不该再 applied"
    );
    assert_eq!(
        body["rejected"][0]["code"], "STALE_IGNORED",
        "LWW：同一条再推→服务端赢"
    );
    assert_eq!(body["rejected"][0]["refId"], "conv-phone-1");

    let n: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM ai_conversations WHERE id = 'conv-phone-1'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(n, 1, "重复推送**只能有一行**（就地更新，绝不新增）");
    let title: String = state
        .with(|conn| {
            conn.query_row(
                "SELECT title FROM ai_conversations WHERE id='conv-phone-1'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(title, "手机上起的会话", "被拒的那次不该改动内容");
}

/// **服务端赢（LWW）**：`updated_at` 不比桌面新就不许覆盖。
#[tokio::test]
async fn sync_push_keeps_the_desktop_value_when_the_push_is_older() {
    let state = state();
    // 桌面先有一行（更新的时间）
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_conversations (id,title,archived,created_at,updated_at)
                 VALUES ('conv-lww','桌面改过的名字',0,'2026-09-24T00:00:00.000Z','2026-09-24T12:00:00.000Z')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    // 手机推一份**更旧**的 → 被拒，桌面值原封不动
    let (status, body) = push(
        &state,
        vec![push_item(
            "ai_conversation",
            "conv-lww",
            conv_payload("conv-lww", "手机上的旧名字", "2026-09-24T09:00:00.000Z"),
        )],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rejected"][0]["code"], "STALE_IGNORED");
    let title: String = state
        .with(|conn| {
            conn.query_row(
                "SELECT title FROM ai_conversations WHERE id='conv-lww'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(title, "桌面改过的名字", "旧的推送不许把桌面的行回退");

    // 手机推一份**更新**的 → 覆盖（桌面已经在别处改过，这是正常接力）
    let (_, body) = push(
        &state,
        vec![push_item(
            "ai_conversation",
            "conv-lww",
            conv_payload("conv-lww", "手机上的新名字", "2026-09-24T13:00:00.000Z"),
        )],
    )
    .await;
    assert_eq!(body["applied"], serde_json::json!(["conv-lww"]));
    let title: String = state
        .with(|conn| {
            conn.query_row(
                "SELECT title FROM ai_conversations WHERE id='conv-lww'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(title, "手机上的新名字");
}

/// `rejected` 必须**逐条给 code**，而且**永久性失败与暂时失败要分清**。
#[tokio::test]
async fn sync_push_rejects_with_per_item_codes() {
    let state = state();
    let (status, body) = push(
        &state,
        vec![
            // 手机不许创建的表（总纲 §3.3）
            push_item("prompts", "p1", serde_json::json!({"id":"p1","updated_at":"2026-09-24T10:00:00.000Z"})),
            push_item("media_assets", "m1", serde_json::json!({"id":"m1"})),
            push_item("tags", "t1", serde_json::json!({"id":"t1"})),
            // 未知类型
            push_item("whatever", "x1", serde_json::json!({"id":"x1"})),
            // 列对不上（永久性：手机死循环重试也没用）
            push_item(
                "ai_conversation",
                "conv-badcol",
                serde_json::json!({"id":"conv-badcol","不存在的列": 1, "updated_at":"2026-09-24T10:00:00.000Z"}),
            ),
            // refId 与 payload.id 不一致
            push_item("ai_conversation", "conv-ref", conv_payload("别的id", "x", "2026-09-24T10:00:00.000Z")),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let codes: Vec<&str> = body["rejected"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["code"].as_str())
        .collect();
    assert_eq!(
        codes,
        vec![
            "FORBIDDEN_KIND",
            "FORBIDDEN_KIND",
            "FORBIDDEN_KIND",
            "UNKNOWN_KIND",
            "SCHEMA_MISMATCH",
            "REF_MISMATCH",
        ],
        "每个被拒项都要有自己的 code（不能只回一个总数）：{body}",
    );
    assert_eq!(body["applied"], serde_json::json!([]), "这批一条都不该成功");

    // 不支持的操作（delete 是"手机删桌面数据"，策略还没定）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/sync/push",
        serde_json::json!({"batch":[{"kind":"ai_conversation","refId":"c-del","op":"delete","payload":{"id":"c-del"}}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rejected"][0]["code"], "UNSUPPORTED_OP");

    // 被拒的那几条**一条都不许落库**
    // （夹具里本来就有一条 conv-0，所以按 id 查而不是数总数）
    let bad: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM ai_conversations WHERE id IN ('p1','m1','t1','x1','conv-badcol','conv-ref')",
                [],
                |r| r.get(0),
            )})
        .unwrap();
    assert_eq!(bad, 0, "被拒的行一条都不许落库");
}

/// ★ **`changes` 要能扫过每一张被同步的表**（真库上曾经在这里 500）。
///
/// 现场：真实库副本上 `/api/sync/changes?since=0&limit=500` 回
/// `INTERNAL: no such column: id in SELECT * FROM ai_message_parts ...` ——
/// `ai_message_parts` 的主键是 `(message_id, ordinal)`，当初按"单列 id"实现。
/// 夹具里没有这张表的行，所以当时**全绿也没抓住**。
/// 这条用例的做法是：夹具里给每张同步表都塞一行，然后要求 `changes` 一次扫完都不出错，
/// 且 **10 张表一张不少地出现在 upserts 里**（少一张就是那条链路没被走到）。
#[tokio::test]
async fn sync_changes_covers_every_synced_table_without_error() {
    let state = state();
    // `media_tags` 不放公共夹具里：`patch_media_...` 那条用例依赖"产物 100 没有自己的标签"，
    // 共享夹具里塞一行会把那条用例弄红。所以**在这条用例里**补，`since=0` 照样能扫到它。
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO media_tags (media_id, tag_id) VALUES (100, 1)",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    let (status, body) = get_json(
        router_for(state.clone()),
        "/api/sync/changes?since=0&limit=500",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "扫过所有同步表不该出错：{body}");

    let all: Vec<&str> = [
        "prompts",
        "tags",
        "prompt_tags",
        "media_assets",
        "media_tags",
        "ai_conversations",
        "ai_messages",
        "ai_message_parts",
        "ai_attachments",
        "capture_runs",
    ]
    .to_vec();
    for table in &all {
        let rows = body["upserts"][table].as_array();
        assert!(
            rows.is_some_and(|r| !r.is_empty()),
            "夹具里每张同步表都有一行，所以 {table} 必须出现在 upserts 里：{}",
            body["upserts"],
        );
    }

    // 复合主键那张表要给出**完整行**（含 message_id / ordinal / type）
    let part = &body["upserts"]["ai_message_parts"][0];
    assert_eq!(part["message_id"], "msg-0");
    assert_eq!(part["ordinal"], 0);
    assert_eq!(part["type"], "text");
}

/// 复合主键的 `deletes` 要按**各自的编码**发数组 ——
/// `ai_message_parts` 用的是 `message_id#ordinal`（`#`，且第一段是 UUID 文本），
/// 与 `prompt_tags` 的 `prompt_id:tag_id`（`:`，两段都是数字）**不是一回事**。
#[tokio::test]
async fn sync_changes_deletes_use_each_tables_own_pk_encoding() {
    let state = state();
    let (_, before) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    let since = before["cursor"].as_i64().unwrap();

    state
        .with(|conn| {
            // 删一条消息块（复合主键，`#` 分隔，第一段是 UUID）
            conn.execute(
                "DELETE FROM ai_message_parts WHERE message_id='msg-0' AND ordinal=0",
                [],
            )
            .map(|_| ())?;
            // 删一条附件（单列 UUID 主键）
            conn.execute("DELETE FROM ai_attachments WHERE id='att-0'", [])
                .map(|_| ())
        })
        .unwrap();

    let (status, body) = get_json(
        router_for(state.clone()),
        &format!("/api/sync/changes?since={since}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["deletes"]["ai_message_parts"][0],
        serde_json::json!(["msg-0", 0]),
        "复合主键发数组，第一段是**字符串**（UUID）不是数字",
    );
    assert_eq!(
        body["deletes"]["ai_attachments"][0],
        serde_json::json!("att-0"),
        "单列文本主键发字符串",
    );
}

/// ★ 手机推**消息块**上来（复合主键、没有 `id` 列）必须能成功。
///
/// 真库上曾经因为"假定表有 `id` 列"而把这类上行**全判 SCHEMA_MISMATCH** ——
/// 表现是"手机上能聊、桌面看不到内容"，而且错误码还写着"永远不可能成功"，
/// 手机照做就会把 outbox 那行丢掉（**永久丢数据**）。
#[tokio::test]
async fn sync_push_accepts_composite_key_tables_without_an_id_column() {
    let state = state();
    let (status, body) = push(
        &state,
        vec![push_item(
            "ai_message_part",
            "msg-0#1",
            serde_json::json!({
                "message_id": "msg-0",
                "ordinal": 1,
                "type": "text",
                "text": "手机推上来的块",
                "created_at": "2026-09-24T10:00:00.000Z",
            }),
        )],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["applied"],
        serde_json::json!(["msg-0#1"]),
        "被拒了：{body}"
    );

    let text: String = state
        .with(|conn| {
            conn.query_row(
                "SELECT text FROM ai_message_parts WHERE message_id='msg-0' AND ordinal=1",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(text, "手机推上来的块");

    // 再推同一条（内容一样）→ **幂等**：不管 applied 里有没有它，结果必须还是同一行。
    //
    // ⚠ 注意 `ai_message_parts` **没有 `updated_at` 列**，所以走不到 LWW 分支 ——
    //   它会老老实实再做一次 `ON CONFLICT DO UPDATE`（于是 `applied` 里又有它）。
    //   这不是 bug：契约 §4 要的是"同一条推两次**结果相同**"，而结果确实相同（一行、内容不变）。
    //   有 `updated_at` 的表（会话 / 消息）才会在第二次落到 `STALE_IGNORED`。
    let (_, body) = push(
        &state,
        vec![push_item(
            "ai_message_part",
            "msg-0#1",
            serde_json::json!({
                "message_id": "msg-0",
                "ordinal": 1,
                "type": "text",
                "text": "手机推上来的块",
                "created_at": "2026-09-24T10:00:00.000Z",
            }),
        )],
    )
    .await;
    assert!(
        body["rejected"].as_array().unwrap().is_empty(),
        "重复推不该被拒（它只是幂等地再写一次）：{body}",
    );
    let n: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM ai_message_parts WHERE message_id='msg-0' AND ordinal=1",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(n, 1, "幂等：同一块推两次**只有一行**");
}

/// 单批上限 500（契约 §4：别让一次请求扛几万行）。
#[tokio::test]
async fn sync_push_rejects_oversized_batches() {
    let state = state();
    let batch: Vec<serde_json::Value> = (0..501)
        .map(|i| {
            push_item(
                "ai_conversation",
                &format!("c{i}"),
                serde_json::json!({"id": format!("c{i}")}),
            )
        })
        .collect();
    let (status, body) = push(&state, batch).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("500"), "{body}");
}

/// 上行点亮 `_sync_log`（否则手机上产生的会话**永远同步不回其它设备**）。
#[tokio::test]
async fn sync_push_lights_up_the_sync_log_without_fake_deletes() {
    let state = state();
    let (_, before) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    let since = before["cursor"].as_i64().unwrap();

    push(
        &state,
        vec![push_item(
            "ai_conversation",
            "conv-log",
            conv_payload("conv-log", "会点亮账本", "2026-09-24T10:00:00.000Z"),
        )],
    )
    .await;
    // 再推一次更新的（走 UPDATE 分支）
    push(
        &state,
        vec![push_item(
            "ai_conversation",
            "conv-log",
            conv_payload("conv-log", "改名了", "2026-09-24T11:00:00.000Z"),
        )],
    )
    .await;

    // ★ R3：upsert 绝不能产生 **delete** 条目
    //   （`INSERT OR REPLACE` 内部是 DELETE+INSERT，会点亮 DELETE 触发器写假 delete）
    let fake_deletes: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM _sync_log WHERE seq > ?1 AND tbl='ai_conversations' AND op='delete'",
                [since],
                |r| r.get(0),
            )})
        .unwrap();
    assert_eq!(fake_deletes, 0, "upsert 不许写假 delete（R3）");

    let upserts: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM _sync_log WHERE seq > ?1 AND tbl='ai_conversations' AND op='upsert'",
                [since],
                |r| r.get(0),
            )})
        .unwrap();
    assert!(upserts >= 2, "插入与更新都要进账本，实际 {upserts}");
}

// ---------------------------------------------------------------------------
//  GET /api/sync/bootstrap
// ---------------------------------------------------------------------------

/// 全量分页：形状自洽 + **翻完所有页不重不漏** + 只发同步范围内的表。
#[tokio::test]
async fn sync_bootstrap_pages_without_gaps_or_duplicates() {
    let state = state();
    // 夹具里 tags 有 2 行；再补 3 行凑成 5 行，pageSize=2 → 3 页
    for i in 0..3 {
        state
            .with(|conn| {
                conn.execute(
                    "INSERT INTO tags (name, normalized, category, use_count, created_at)
                     VALUES (?1, ?1, '风格', 0, '2026-09-24T00:00:00.000Z')",
                    [format!("全量{i}")],
                )
                .map(|_| ())
            })
            .unwrap();
    }

    let (status, page1) = get_json(
        router_for(state.clone()),
        "/api/sync/bootstrap?table=tags&page=1&pageSize=2",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        keys(&page1),
        vec!["page", "pageSize", "rows", "table", "totalPages"],
        "bootstrap 的键集合就是契约 §3",
    );
    assert_eq!(page1["table"], "tags");
    assert_eq!(page1["page"], 1);
    assert_eq!(page1["pageSize"], 2);
    assert_eq!(page1["totalPages"], 3, "5 行 / 每页 2 = 3 页");
    assert_eq!(page1["rows"].as_array().unwrap().len(), 2);

    // ★ 翻完所有页：id 一个不重、一个不漏
    let mut ids: Vec<i64> = Vec::new();
    for page in 1..=3 {
        let (_, body) = get_json(
            router_for(state.clone()),
            &format!("/api/sync/bootstrap?table=tags&page={page}&pageSize=2"),
        )
        .await;
        for row in body["rows"].as_array().unwrap() {
            ids.push(row["id"].as_i64().unwrap());
        }
    }
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        5,
        "翻完所有页要正好 5 条（重复/漏掉都是 bug）：{ids:?}"
    );
}

/// **绝不发 `.db` 文件**：`app_settings` 里有配对令牌明文，bootstrap 一张表都不许多发。
#[tokio::test]
async fn sync_bootstrap_refuses_tables_outside_the_scope() {
    let state = state();
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO app_settings (k, v, updated_at) VALUES ('auth.pairToken','SECRET-TOKEN','x')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    // 凭据/令牌表一律 400，而且错误信息里**列出合法表名**（自解释）
    for forbidden in ["app_settings", "ai_providers", "ai_models", "ai_run_events"] {
        let (status, body) = get_json(
            router_for(state.clone()),
            &format!("/api/sync/bootstrap?table={forbidden}&page=1"),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{forbidden} 不该能全量导出"
        );
        let msg = body["error"].as_str().unwrap();
        assert!(msg.contains("prompts"), "错误信息要列出合法表名：{msg}");
        assert!(
            !body.to_string().contains("SECRET-TOKEN"),
            "令牌绝不能出现：{body}"
        );
    }

    // 缺 table 也是 400（必填，理由见 sync::bootstrap 的注释）
    let (status, body) = get_json(router_for(state.clone()), "/api/sync/bootstrap?page=1").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("table"));
}

// ---------------------------------------------------------------------------
//  GET /api/net/urls（P4 第 73 步）：本机可达地址（「同步与连接」页的候选）
// ---------------------------------------------------------------------------

/// 测试里的 AppState **没有监听任何端口**（`port == 0`）→ 如实回空数组。
///
/// 这是刻意的"宁可什么都不说"：报一个我们其实没在听的地址，用户会以为是网络问题。
#[tokio::test]
async fn net_urls_is_empty_when_we_are_not_listening() {
    let app = router_for(state());
    let (status, body) = get_json(app, "/api/net/urls").await;
    assert_eq!(status, 200);
    assert_eq!(body["port"], 0, "没绑端口就报 0：{body}");
    assert_eq!(
        body["urls"].as_array().unwrap().len(),
        0,
        "宁可空，也不许编：{body}"
    );
}

/// 真的在听的时候：回环那条一定在（不依赖这台机器有几张网卡），
/// 每条都得是 `http://…:<我们实际听的端口>`，且 `kind` 只能是那四个值之一。
#[tokio::test]
async fn net_urls_lists_the_port_we_actually_listen_on() {
    let st = state();
    st.set_port(18081);
    let app = router_for(st);
    let (status, body) = get_json(app, "/api/net/urls").await;
    assert_eq!(status, 200);
    assert_eq!(body["port"], 18081);

    let urls = body["urls"].as_array().expect("urls 数组");
    assert!(!urls.is_empty(), "至少要有一条回环：{body}");
    let mut saw_loopback = false;
    for item in urls {
        let kind = item["kind"].as_str().unwrap();
        let url = item["url"].as_str().unwrap();
        assert!(
            ["loopback", "lan", "tailscale", "other"].contains(&kind),
            "kind 只有这四种：{kind}"
        );
        assert!(
            url.starts_with("http://"),
            "候选地址必须是 http:// 开头：{url}"
        );
        assert!(url.ends_with(":18081"), "端口必须是我们实际听的那个：{url}");
        if kind == "loopback" {
            saw_loopback = true;
            assert_eq!(url, "http://127.0.0.1:18081");
        }
    }
    assert!(saw_loopback, "同机/模拟器那条路必须有：{body}");
}

/// **只被允许读的设备不许写**：这是"两个开关分开"在 HTTP 层的样子（§14.1）。
#[tokio::test]
async fn credential_upload_is_forbidden_without_the_upload_switch() {
    let st = state();
    seed_auth_device(&st, true, false);
    let app = router_for(st);

    let (status, body) = send_json_token(
        app,
        "PUT",
        "/api/ai/credentials",
        serde_json::json!({ "credentials": { "SOME_KEY": "sk-1" } }),
        DEVICE_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "得到 {body}");
    assert!(
        body["error"].as_str().unwrap_or("").contains("修改密钥"),
        "措辞要指对方向（读/写是两件事）：{body}"
    );
}

/// 版本号落后 → `STALE_IGNORED`，而且**一个字节都不写**（§14.3）。
#[tokio::test]
async fn credential_upload_refuses_a_stale_rev_without_writing() {
    let st = state();
    seed_auth_device(&st, false, true);
    put_setting(&st, comfyhub_server_rs::provider::credentials::REV_KEY, "7");
    let store_before =
        std::fs::read_to_string(comfyhub_server_rs::provider::credentials::store_path(&st)).ok();
    let app = router_for(st.clone());

    let (status, body) = send_json_token(
        app,
        "PUT",
        "/api/ai/credentials",
        serde_json::json!({ "credentials": { "SOME_KEY": "sk-1" }, "rev": 3 }),
        DEVICE_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "版本冲突不是错误状态码：{body}");
    assert_eq!(body["applied"].as_array().unwrap().len(), 0);
    let rejected = body["rejected"].as_array().unwrap();
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0]["code"], "STALE_IGNORED");
    assert_eq!(rejected[0]["ref"], "SOME_KEY");
    assert_eq!(body["rev"], 7, "回执要带上桌面**当前**的版本号：{body}");

    // 库里那条版本号没动，凭据文件也没被碰过
    assert_eq!(
        read_setting(&st, comfyhub_server_rs::provider::credentials::REV_KEY).unwrap(),
        serde_json::json!(7)
    );
    let store_after =
        std::fs::read_to_string(comfyhub_server_rs::provider::credentials::store_path(&st)).ok();
    assert_eq!(store_before, store_after, "STALE 分支不许落任何东西");
}

/// 逐条独立：坏名字 / 空值 → `BAD_PAYLOAD`；环境变量提供的 ref → `FORBIDDEN`（只读）。
/// **一条坏的不连坐**（其余照写）。
#[tokio::test]
async fn credential_upload_rejects_per_ref_and_never_conflates_them() {
    let st = state();
    seed_auth_device(&st, false, true);
    // 环境变量提供的 ref：写入一律拒绝（与桌面手动填同一条纪律）
    std::env::set_var("COMFYHUB_TEST_ENV_KEY", "sk-from-env");
    let app = router_for(st.clone());

    let (status, body) = send_json_token(
        app,
        "PUT",
        "/api/ai/credentials",
        serde_json::json!({ "credentials": {
            "坏名字 with space": "sk-1",
            "EMPTY_ONE": "   ",
            "COMFYHUB_TEST_ENV_KEY": "sk-2"
        } }),
        DEVICE_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "逐条拒不是整批失败：{body}");
    std::env::remove_var("COMFYHUB_TEST_ENV_KEY");

    let rejected = body["rejected"].as_array().unwrap();
    let by_ref = |name: &str| {
        rejected
            .iter()
            .find(|r| r["ref"] == name)
            .unwrap_or_else(|| panic!("{name} 必须有一条拒收：{body}"))
    };
    assert_eq!(by_ref("坏名字 with space")["code"], "BAD_PAYLOAD");
    assert_eq!(by_ref("EMPTY_ONE")["code"], "BAD_PAYLOAD");
    let env_one = by_ref("COMFYHUB_TEST_ENV_KEY");
    assert_eq!(env_one["code"], "FORBIDDEN");
    assert!(
        env_one["message"].as_str().unwrap().contains("环境变量"),
        "要说清为什么拒：{env_one}"
    );
    assert_eq!(
        body["applied"].as_array().unwrap().len(),
        0,
        "三条都该被拒：{body}"
    );
}

/// 真的写进去：应用成功、版本号加一、值能从托管库里解出来（Windows 才有 DPAPI）。
#[cfg(windows)]
#[tokio::test]
async fn credential_upload_applies_bumps_rev_and_the_key_really_works() {
    let st = state();
    seed_auth_device(&st, false, true);
    let app = router_for(st.clone());

    let (status, body) = send_json_token(
        app,
        "PUT",
        "/api/ai/credentials",
        serde_json::json!({ "credentials": { "PHONE_PUSHED_KEY": "sk-from-phone" } }),
        DEVICE_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["applied"], serde_json::json!(["PHONE_PUSHED_KEY"]));
    assert_eq!(body["rev"], 1, "写入成功要 bump：{body}");

    // 值**真的能用**（不是只回了个 200）：走与上游调用同一条解析路径
    let resolved =
        comfyhub_server_rs::mobile::auth::resolve_credential(&st, "PHONE_PUSHED_KEY").unwrap();
    assert_eq!(resolved, "sk-from-phone");
    assert_eq!(
        read_setting(&st, comfyhub_server_rs::provider::credentials::REV_KEY).unwrap(),
        serde_json::json!(1)
    );

    // 值**不回显**：状态接口只给 configured/source/writable
    let described =
        comfyhub_server_rs::provider::credentials::describe_now(&st, "PHONE_PUSHED_KEY");
    assert!(described.configured);
    assert!(!serde_json::to_string(&described)
        .unwrap()
        .contains("sk-from-phone"));
}

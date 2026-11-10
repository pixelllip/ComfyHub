//! Provider 与模型：CRUD、模型目录、内置目录、能力三层、凭据、联网搜索凭据。
//!
//! 夹具在 [`super::common`]；`use super::common::*;` 之后照旧直接写用例。

use super::common::*;

// ===========================================================================
//  AI 端点第一批（P4 第 16 步）：GET /api/ai/providers
// ===========================================================================

/// provider 清单：**形状要逐字对齐**，而且**一个字节的密钥都不许出去**（AIH-012）。
#[tokio::test]
async fn ai_providers_shape_and_never_leaks_a_secret_value() {
    let state = state();
    // 托管凭据文件里放一把"密钥"：它**只该被读键名**，值绝不能出现在响应里
    let store = state.media_dir().parent().unwrap().join("ai");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(
        store.join("credentials.dpapi.json"),
        r#"{"MANAGED_KEY":"DPAPI-SEALED-SECRET-VALUE"}"#,
    )
    .unwrap();
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_providers
                 (id, display_name, api, base_url, credential_ref, endpoint_trust, enabled, revision)
                 VALUES ('p-managed', '本机网关', 'openai-completions',
                         'http://127.0.0.1:11434/v1', 'MANAGED_KEY', 'loopback', 1, 3),
                        ('p-none', '没配过的', 'anthropic-messages',
                         'https://api.anthropic.com/v1', 'MISSING_KEY', 'public', 0, 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let (status, body) = get_json(router_for(state), "/api/ai/providers").await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let list = body.as_array().unwrap();
    assert_eq!(list.len(), 2, "按 display_name, id 排序");
    assert_eq!(list[0]["id"], "p-managed");

    let mut shape = keys(&list[0]);
    shape.sort();
    assert_eq!(
        shape,
        vec![
            "api",
            "baseURL",
            "createdAt",
            "credential",
            "credentialRef",
            "displayName",
            "enabled",
            "endpointTrust",
            "id",
            "revision",
            "updatedAt",
        ],
        "`credential` 与两个时间戳最容易漏（Kotlin 那边 encodeDefaults = true，一定会序列化）"
    );
    assert_eq!(list[0]["baseURL"], "http://127.0.0.1:11434/v1");
    assert_eq!(list[0]["enabled"], true);
    assert_eq!(list[0]["revision"], 3);

    // 凭据状态：三个键、**没有值**；托管文件里有 → managed 且可写
    assert_eq!(
        keys(&list[0]["credential"]),
        vec!["configured", "source", "writable"]
    );
    assert_eq!(list[0]["credential"]["configured"], true);
    assert_eq!(list[0]["credential"]["source"], "managed");
    assert_eq!(list[0]["credential"]["writable"], true);
    // 没配过的那条：none，但**仍可写**（界面要能给它写一把新的）
    assert_eq!(list[1]["credential"]["configured"], false);
    assert_eq!(list[1]["credential"]["source"], "none");
    assert_eq!(list[1]["credential"]["writable"], true);

    // ★ 红线（AIH-012）：响应里**不许出现那把密钥的值**
    let raw = serde_json::to_string(&body).unwrap();
    assert!(
        !raw.contains("DPAPI-SEALED-SECRET-VALUE"),
        "凭据值绝不能出库/出网：{raw}"
    );
}

// ===========================================================================
//  模型目录（P4 第 20 步 = Task 2）：GET /api/ai/providers/{id}/models
// ===========================================================================

/// 模型目录：**12 个键永远在、6 个可空键一个都不许自己冒出来**，且坏 JSON 不能让整页打不开。
///
/// 这条用例是照着**真库副本上 Kotlin（8090）的实测**写的，不是照着 DTO 猜的：
/// 81 条 `command-code-goat` 模型里 `thinkingFormat` 有 10 条不出现、
/// `maxOutputTokens` / `maxAttachmentBytes` / `maxAttachmentCount` / `capabilityVerifiedAt`
/// 81/81 全都不出现（NULL → `explicitNulls = false`）。
/// 写成"所有键一定都在"就等于**用测试把错误的形状钉住**（第 18 步刚栽过这一次）。
#[tokio::test]
async fn ai_models_distinguish_missing_provider_from_an_empty_catalog() {
    let state = state();
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_providers
                 (id, display_name, api, base_url, endpoint_trust, enabled, revision)
                 VALUES ('p-models', '模型目录', 'openai-completions', 'http://127.0.0.1:1234/v1',
                         'loopback', 1, 1),
                        ('p-empty', '空目录', 'openai-completions', 'http://127.0.0.1:1235/v1',
                         'loopback', 1, 1)",
                [],
            )?;
            conn.execute(
                r#"INSERT INTO ai_models
                   (provider_id, model_id, display_name, input_modalities, attachment_transports,
                    mime_allowlist, tools, parallel_tools, reasoning, thinking_efforts, thinking_format,
                    context_window, max_output_tokens, max_attachment_bytes, max_attachment_count,
                    capability_source, capability_verified_at, enabled)
                   VALUES
                   -- ① 样样都有：18 个键一个不少
                   ('p-models', 'm-a', 'A 模型', '["text","image"]', '{"image":["inline_base64"]}',
                    '["image/png"]', 1, 0, 1, '{"high":"high","max":4096}', 'openai',
                    200000, 8192, 8388608, 4, 'builtin', '2026-09-23T05:00:00.000Z', 1),
                   -- ② 可空列全 NULL：6 个可空键**一个都不许出现**
                   --    （display_name / capability_source 是 NOT NULL，给真值）
                   ('p-models', 'm-b', 'm-b', NULL, NULL, NULL, 0, 0, 0, NULL, NULL,
                    NULL, NULL, NULL, NULL, 'manual', NULL, 0),
                   -- ③ 坏 JSON：解析不出来就按空处理，**不能让整个端点 500**
                   ('p-models', 'm-c', 'B 模型', 'not json', '[1,2]', '{oops}',
                    1, 0, 0, '{"off":null,"low":"low"}', '   ',
                    NULL, NULL, 0, 0, 'manual', NULL, 1)"#,
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let (status, body) = get_json(
        router_for(state.clone()),
        "/api/ai/providers/p-models/models",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let list = body.as_array().unwrap();
    // 顺序照抄 Kotlin 的 `ORDER BY display_name, model_id`（**默认 BINARY 排序**）：
    // 大写 ASCII 在前、小写在后 → A 模型 / B 模型 / m-b
    assert_eq!(
        list.iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["m-a", "m-c", "m-b"],
        "顺序换了在界面上就是「列表位置变了」：{body}"
    );

    // ① 全字段那行：精确键集合（18 个）
    assert_eq!(
        keys(&list[0]),
        vec![
            "attachmentTransports",
            "capabilitySource",
            "capabilityVerifiedAt",
            "contextWindow",
            "displayName",
            "enabled",
            "id",
            "inputModalities",
            "maxAttachmentBytes",
            "maxAttachmentCount",
            "maxOutputTokens",
            "mimeAllowlist",
            "parallelTools",
            "providerId",
            "reasoning",
            "thinkingEfforts",
            "thinkingFormat",
            "tools",
        ],
        "少一个键 = 界面上少一块内容（附件准入与思考强度选择器全靠它们）"
    );
    assert_eq!(list[0]["providerId"], "p-models");
    assert_eq!(list[0]["id"], "m-a");
    assert_eq!(list[0]["displayName"], "A 模型");
    assert_eq!(
        list[0]["inputModalities"],
        serde_json::json!(["text", "image"])
    );
    assert_eq!(
        list[0]["attachmentTransports"],
        serde_json::json!({"image": ["inline_base64"]}),
        "附件传输方式的唯一真源（AGENTS §10）"
    );
    assert_eq!(list[0]["mimeAllowlist"], serde_json::json!(["image/png"]));
    assert_eq!(list[0]["tools"], true);
    assert_eq!(list[0]["reasoning"], true);
    // 数字必须还是数字（写成字符串前端 `as num?` 就取不到）
    assert_eq!(list[0]["contextWindow"], 200000);
    assert_eq!(list[0]["maxOutputTokens"], 8192);
    assert_eq!(list[0]["maxAttachmentBytes"], 8388608);
    assert_eq!(list[0]["maxAttachmentCount"], 4);
    // thinking_efforts 的值**可能是数字**（Anthropic 的思考预算，AIH-056）→ 统一按字面文本
    assert_eq!(list[0]["thinkingEfforts"]["max"], "4096");
    // 时间列**不是透传**：Kotlin 先解析、再按 Java `Instant.toString()` 重排 ——
    // 库里那个 `.000` 在这里被吃掉（`…:00.000Z` → `…:00Z`）。
    // 这条断言就是 `iso_time` 存在的理由：透传版本在真库上"恰好"是对的，
    // 直到某一行的时间戳毫秒位真的是 000（1/1000）。
    assert_eq!(list[0]["capabilityVerifiedAt"], "2026-09-23T05:00:00Z");

    // ② 全 NULL 那行：**只有 12 个键**
    assert_eq!(
        keys(&list[2]),
        vec![
            "attachmentTransports",
            "capabilitySource",
            "displayName",
            "enabled",
            "id",
            "inputModalities",
            "mimeAllowlist",
            "parallelTools",
            "providerId",
            "reasoning",
            "thinkingEfforts",
            "tools",
        ],
        "可空列为 NULL 时 Kotlin 连键都不发（explicitNulls = false）"
    );
    assert_eq!(list[2]["displayName"], "m-b");
    assert_eq!(list[2]["capabilitySource"], "manual");
    assert_eq!(list[2]["inputModalities"], serde_json::json!([]));
    assert_eq!(
        list[2]["attachmentTransports"],
        serde_json::json!({}),
        "空表不是 null"
    );
    assert_eq!(list[2]["thinkingEfforts"], serde_json::json!({}));
    assert_eq!(list[2]["enabled"], false);

    // ③ 损坏 JSON 那行：不报错，按空处理；空白 `thinking_format` 也算"未声明"
    assert_eq!(list[1]["inputModalities"], serde_json::json!([]));
    assert_eq!(list[1]["attachmentTransports"], serde_json::json!({}));
    assert_eq!(list[1]["mimeAllowlist"], serde_json::json!([]));
    assert_eq!(
        list[1]["thinkingEfforts"],
        serde_json::json!({"low": "low"}),
        "JSON null 的值要丢掉（Kotlin contentOrNull 也是这个口径）"
    );
    let m_c = list[1].as_object().unwrap();
    assert!(
        !m_c.contains_key("thinkingFormat"),
        "空白串 = 没声明：{body}"
    );
    assert!(!m_c.contains_key("contextWindow"));

    // ★ 三种"没有模型"必须分清：provider 存在但目录为空 → 200 `[]`
    let (status, empty) = get_json(
        router_for(state.clone()),
        "/api/ai/providers/p-empty/models",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty, serde_json::json!([]));

    // ★ provider 不存在 → **400 `{error:"CONFIG_ERROR", detail: 人话}`**（不是 404、不是 500）
    let (status, missing) = get_json(router_for(state), "/api/ai/providers/nope/models").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(keys(&missing), vec!["detail", "error"]);
    assert_eq!(missing["error"], "CONFIG_ERROR");
    assert_eq!(missing["detail"], "Provider 不存在: nope");
}

// ---------------------------------------------------------------------------
//  Provider 写侧 + 模型目录替换（P4 第 37 步）
// ---------------------------------------------------------------------------

/// provider 的新建 / 更新 / 删除完整一轮，以及**每一条校验的稳定码与人话**。
///
/// 这里检查"设置能否被正确保存"：校验顺序颠倒（例如先判 URL 再判重名）会让
/// 在一个本无错误的字段上显示报错；`revision` 少判一次，两个窗口会互相覆盖设置
/// （而"被覆盖了"这件事现场完全看不出来）。
#[tokio::test]
async fn provider_crud_round_trip_with_revision_and_validation() {
    let state = state();

    // 新建：201 + **11 个键** + revision 由库给 1
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/providers",
        serde_json::json!({
            "id": "my-gateway",
            "displayName": "我的网关",
            "api": "openai-completions",
            "baseURL": "http://127.0.0.1:1234/v1/",
            "credentialRef": "MY_GATEWAY_KEY"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        keys(&body),
        vec![
            "api",
            "baseURL",
            "createdAt",
            "credential",
            "credentialRef",
            "displayName",
            "enabled",
            "endpointTrust",
            "id",
            "revision",
            "updatedAt"
        ]
    );
    assert_eq!(body["revision"], 1);
    assert_eq!(body["enabled"], true);
    assert_eq!(
        body["baseURL"], "http://127.0.0.1:1234/v1",
        "只去掉末尾的 `/`，路径一个字都不许改"
    );
    assert_eq!(
        body["endpointTrust"], "loopback",
        "没传 endpointTrust → 按主机**推断**（否则回环地址会撞上 public 档）"
    );
    assert_eq!(
        body["credential"],
        serde_json::json!({"configured": false, "source": "none", "writable": true})
    );

    // 重名：CONFIG_ERROR + 明确说"ID 创建后不可修改"
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/providers",
        serde_json::json!({ "id": "my-gateway", "displayName": "x", "api": "openai-completions", "baseURL": "https://api.example.com" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert_eq!(
        body["detail"],
        "Provider ID 已存在: my-gateway（ID 创建后不可修改）"
    );

    // 三条校验：ID 形态 / 协议 / 凭据引用名 —— 文案都是给人看的
    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/providers",
        serde_json::json!({ "id": "Bad_ID", "displayName": "x", "api": "openai-completions", "baseURL": "https://api.example.com" }),
    )
    .await;
    assert_eq!(
        body["detail"],
        "Provider ID 只能是小写 kebab-case（如 my-gateway），且创建后不可修改"
    );
    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/providers",
        serde_json::json!({ "id": "p1", "displayName": "x", "api": "openai", "baseURL": "https://api.example.com" }),
    )
    .await;
    assert_eq!(
        body["detail"],
        "不支持的协议 openai；首期仅支持 openai-completions / openai-responses / anthropic-messages"
    );
    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/providers",
        serde_json::json!({ "id": "p1", "displayName": "x", "api": "openai-completions", "baseURL": "https://api.example.com", "credentialRef": "lower_case" }),
    )
    .await;
    assert_eq!(
        body["detail"],
        "凭据引用名只能是大写字母、数字和下划线，且以字母开头"
    );

    // URL 与信任档：公网必须 https；public 档不许指向回环
    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/providers",
        serde_json::json!({ "id": "p1", "displayName": "x", "api": "openai-completions", "baseURL": "http://api.example.com", "endpointTrust": "public" }),
    )
    .await;
    assert_eq!(body["detail"], "公网端点必须使用 https");
    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/providers",
        serde_json::json!({ "id": "p1", "displayName": "x", "api": "openai-completions", "baseURL": "https://127.0.0.1:9", "endpointTrust": "public" }),
    )
    .await;
    assert_eq!(
        body["detail"],
        "端点信任级别 public 不允许访问主机 127.0.0.1（当前判定：LOOPBACK）"
    );

    // 更新：缺 revision / revision 过期 —— 两条都要 400 且说清怎么办
    let (_, body) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/providers/my-gateway",
        serde_json::json!({ "displayName": "改了", "api": "openai-completions", "baseURL": "http://127.0.0.1:1234" }),
    )
    .await;
    assert_eq!(body["detail"], "缺少 revision，请刷新后重试");
    let (_, body) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/providers/my-gateway",
        serde_json::json!({ "displayName": "改了", "api": "openai-completions", "baseURL": "http://127.0.0.1:1234", "revision": 9 }),
    )
    .await;
    assert_eq!(
        body["detail"],
        "配置已被其他窗口修改（当前 revision=1，提交的=9），请刷新后重试"
    );

    // 更新成功：revision +1，返回值是**回读的那份**
    let (status, body) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/providers/my-gateway",
        serde_json::json!({
            "displayName": "我的网关（改）",
            "api": "openai-responses",
            "baseURL": "http://127.0.0.1:1234",
            "revision": 1,
            "enabled": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["revision"], 2);
    assert_eq!(body["displayName"], "我的网关（改）");
    assert_eq!(body["api"], "openai-responses");
    assert_eq!(body["enabled"], false);
    assert_eq!(
        body["credentialRef"],
        serde_json::Value::Null,
        "没传就是清掉（Kotlin 同）"
    );

    // ID 不许改（body 里带一个不一样的）
    let (_, body) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/providers/my-gateway",
        serde_json::json!({ "id": "other", "displayName": "x", "api": "openai-completions", "baseURL": "http://127.0.0.1:1234", "revision": 2 }),
    )
    .await;
    assert_eq!(body["detail"], "Provider ID 创建后不可修改");

    // 单个读 / 读不到
    let (status, body) = get_json(router_for(state.clone()), "/api/ai/providers/my-gateway").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], "my-gateway");
    let (status, body) = get_json(router_for(state.clone()), "/api/ai/providers/nope").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], "Provider 不存在: nope");

    // 删除：连锁把模型目录一起带走（R1 的 ON DELETE CASCADE）
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_models (provider_id, model_id, display_name) VALUES ('my-gateway', 'm', 'M')",
                [],
            )?;
            Ok(())
        })
        .expect("插模型");
    let (status, body) = send_empty(
        router_for(state.clone()),
        "DELETE",
        "/api/ai/providers/my-gateway",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        serde_json::json!({ "deleted": true, "id": "my-gateway" })
    );
    let left: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM ai_models WHERE provider_id='my-gateway'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(
        left, 0,
        "删 provider 要连模型一起删（外键 ON DELETE CASCADE）"
    );
    let (status, body) = send_empty(
        router_for(state.clone()),
        "DELETE",
        "/api/ai/providers/my-gateway",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], "Provider 不存在: my-gateway");
}

/// `PUT /providers/{id}/models`：全量替换 + 逐条校验（AIH-011）。
///
/// 模型目录是**附件准入与思考强度的唯一真源** —— 少校验一条，就会保存一个
/// "看着配好了、一用就 400"的模型。
#[tokio::test]
async fn provider_models_put_replaces_the_catalog_and_validates_every_field() {
    let state = state();
    add_provider(&state, "p-models", Some("MODELS_KEY"));

    let model = serde_json::json!({
        "providerId": "p-models",
        "id": "vendor/model-1",
        "displayName": "模型一号",
        "inputModalities": ["text", "image"],
        "attachmentTransports": { "image": ["inline_base64"] },
        "mimeAllowlist": ["image/png"],
        "tools": true,
        "parallelTools": true,
        "reasoning": true,
        "thinkingEfforts": { "low": "low", "max": "32768" },
        "thinkingFormat": "deepseek",
        "contextWindow": 128000,
        "enabled": true
    });
    let (status, body) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/providers/p-models/models",
        serde_json::json!({ "models": [model] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);
    let saved = &body[0];
    assert_eq!(
        saved["inputModalities"],
        serde_json::json!(["text", "image"])
    );
    assert_eq!(
        saved["attachmentTransports"],
        serde_json::json!({ "image": ["inline_base64"] })
    );
    assert_eq!(
        saved["thinkingEfforts"],
        serde_json::json!({ "low": "low", "max": "32768" })
    );
    assert_eq!(saved["thinkingFormat"], "deepseek");
    assert_eq!(saved["contextWindow"], 128000);
    assert!(
        !saved.as_object().unwrap().contains_key("maxOutputTokens"),
        "没给的可空键不许冒出来"
    );

    // 全量替换：旧的没了、新的在（同一个事务里）
    let (status, body) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/providers/p-models/models",
        serde_json::json!({ "models": [{
            "id": "vendor/model-2", "displayName": "模型二号", "capabilitySource": "discovered", "enabled": false
        }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["id"], "vendor/model-2");
    assert_eq!(body[0]["enabled"], false);
    assert_eq!(body[0]["capabilitySource"], "discovered");
    // body 里没写 providerId → 以**路径**为准（信 body 会出现"给 A 存了一堆写着 B 的模型"）
    assert_eq!(body[0]["providerId"], "p-models");
    // 缺省值走 Kotlin 的那套
    assert_eq!(body[0]["tools"], false);
    assert_eq!(body[0]["inputModalities"], serde_json::json!([]));

    // 六条校验各一发
    let cases: Vec<(serde_json::Value, &str)> = vec![
        (
            serde_json::json!({ "id": "m", "displayName": "M", "inputModalities": ["hologram"] }),
            "未知的输入模态: hologram",
        ),
        (
            serde_json::json!({ "id": "m", "displayName": "M", "attachmentTransports": { "smell": ["inline_base64"] } }),
            "未知的传输方式键: smell",
        ),
        (
            serde_json::json!({ "id": "m", "displayName": "M", "attachmentTransports": { "image": ["telepathy"] } }),
            "未知的传输方式: telepathy",
        ),
        (
            serde_json::json!({ "id": "m", "displayName": "M", "capabilitySource": "guessed" }),
            "非法的能力来源: guessed",
        ),
        (
            serde_json::json!({ "id": "m", "displayName": "M", "reasoning": true, "thinkingEfforts": { "ultra": "x" } }),
            "未知的思考等级: ultra",
        ),
        (
            serde_json::json!({ "id": "m", "displayName": "M", "reasoning": true, "thinkingEfforts": { "high": "0" } }),
            "思考等级 high 的表达不能为空",
        ),
        (
            serde_json::json!({ "id": "m", "displayName": "M", "thinkingEfforts": { "high": "high" } }),
            "声明了思考等级却没勾选「支持推理」，两者必须一致",
        ),
        (
            serde_json::json!({ "id": "m", "displayName": "M", "thinkingFormat": "telepathy" }),
            "未知的思考方言: telepathy（可选 openai / deepseek / qwen / openrouter / zai）",
        ),
    ];
    for (bad, expected) in cases {
        let (status, body) = send_json(
            router_for(state.clone()),
            "PUT",
            "/api/ai/providers/p-models/models",
            serde_json::json!({ "models": [bad] }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{expected}");
        assert_eq!(body["error"], "CONFIG_ERROR", "{expected}");
        assert_eq!(body["detail"], expected);
        // 校验失败**不许动**已有目录（先校验、再删旧插新）
        let (_, body) = get_json(
            router_for(state.clone()),
            "/api/ai/providers/p-models/models",
        )
        .await;
        assert_eq!(
            body[0]["id"], "vendor/model-2",
            "校验失败时旧目录必须原地不动"
        );
    }

    // provider 不存在 → 400 CONFIG_ERROR（不是 404）
    let (status, body) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/providers/nope/models",
        serde_json::json!({ "models": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], "Provider 不存在: nope");
}

// ---------------------------------------------------------------------------
//  内置模型目录的状态与同步（P4 第 38 步）
// ---------------------------------------------------------------------------

/// 预览 → 同步 → 再预览：**幂等**，且**已修改的行不得改动**。
///
/// 这条链的失败模式均为静默：覆盖已修改的能力声明、删除自定义添加的模型、
/// 或者"第二次同步又新增了 69 个"。判据只有"库里那几行还是不是原样"。
#[tokio::test]
async fn builtin_status_previews_and_sync_is_idempotent_and_never_touches_user_edits() {
    let state = state();

    // ① 预览：provider 不存在 → 整份目录都要插，`providerCreated=true`
    let (status, preview) = get_json(router_for(state.clone()), "/api/ai/builtin/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        keys(&preview),
        vec![
            "added",
            "divergent",
            "kept",
            "mode",
            "modelCount",
            "providerCreated",
            "providerId",
            "updated",
            "version"
        ],
        "`providerId` **在**（它是目录里那个 provider 的 id），`error` 不在（成功时是 null）"
    );
    assert!(!preview["providerId"].as_str().unwrap().is_empty());
    assert_eq!(preview["mode"], "add-missing");
    assert_eq!(preview["providerCreated"], true);
    assert!(preview["added"].as_i64().unwrap() > 0);
    assert_eq!(preview["updated"], 0, "预览从不刷新");
    assert_eq!(preview["divergent"], serde_json::json!([]));
    let total = preview["added"].as_i64().unwrap();
    assert_eq!(preview["modelCount"], total);
    let version = preview["version"].as_str().unwrap().to_string();

    // ② 预览**不写库**：一个 provider / 模型都不该多出来
    let providers = state
        .with(|conn| {
            conn.query_row("SELECT COUNT(*) FROM ai_providers", [], |r| {
                r.get::<_, i64>(0)
            })
        })
        .unwrap();
    assert_eq!(providers, 0, "只读预览绝不许写库");
    let seeded_version: Option<String> = state
        .with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT v FROM app_settings WHERE k = 'ai.builtin.seed.version'",
                    [],
                    |r| r.get(0),
                )
                .ok())
        })
        .unwrap();
    assert!(seeded_version.is_none(), "预览也不许写版本号");

    // ③ 同步（默认 add-missing）：整份插进去
    let (status, sync) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/builtin/sync",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sync["error"], serde_json::Value::Null, "同步不该失败");
    assert_eq!(sync["providerCreated"], true);
    assert_eq!(sync["added"], total);
    assert_eq!(sync["modelCount"], total);
    // 版本号记进 app_settings（下次启动/同步据此判断目录有没有变）
    let stored: String = state
        .with(|conn| {
            conn.query_row(
                "SELECT v FROM app_settings WHERE k='ai.builtin.seed.version'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(stored, version);

    // ④ 再预览：现在一条都不缺
    let (_, preview) = get_json(router_for(state.clone()), "/api/ai/builtin/status").await;
    assert_eq!(preview["added"], 0);
    assert_eq!(preview["providerCreated"], false);
    assert_eq!(preview["modelCount"], total);
    assert_eq!(
        preview["kept"].as_i64().unwrap() + preview["divergent"].as_array().unwrap().len() as i64,
        total
    );

    // ⑤ 再同步一次：**幂等**（added=0、模型总数不变）
    let (_, again) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/builtin/sync",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(again["added"], 0, "第二次同步不该再新增");
    assert_eq!(again["modelCount"], total);

    // ⑥ 已修改的行：显示名 / 启用 / 附件上限 —— 同步**不得改动**
    let (provider_id, model_id) = state
        .with(|conn| {
            let p: String =
                conn.query_row("SELECT id FROM ai_providers LIMIT 1", [], |r| r.get(0))?;
            let m: String = conn.query_row(
                "SELECT model_id FROM ai_models WHERE provider_id=?1 ORDER BY model_id LIMIT 1",
                [&p],
                |r| r.get(0),
            )?;
            Ok((p, m))
        })
        .unwrap();
    state
        .with(|conn| {
            conn.execute(
                "UPDATE ai_models SET display_name='用户改的名字', enabled=0, max_attachment_bytes=123
                  WHERE provider_id=?1 AND model_id=?2",
                rusqlite::params![provider_id, model_id],
            )?;
            // 自定义添加的模型（不在目录中）——同步**永不删除**
            conn.execute(
                "INSERT INTO ai_models (provider_id, model_id, display_name, capability_source)
                 VALUES (?1, 'user-added-model', '用户自己加的', 'manual')",
                rusqlite::params![provider_id],
            )?;
            Ok(())
        })
        .unwrap();
    let (_, sync) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/builtin/sync",
        serde_json::json!({ "mode": "add-missing" }),
    )
    .await;
    assert_eq!(sync["added"], 0);
    assert_eq!(sync["modelCount"], total + 1, "用户自己加的那个还在");
    let (name, enabled, max_bytes): (String, i64, Option<i64>) = state
        .with(|conn| {
            conn.query_row(
                "SELECT display_name, enabled, max_attachment_bytes FROM ai_models
                  WHERE provider_id=?1 AND model_id=?2",
                rusqlite::params![provider_id, model_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
        })
        .unwrap();
    assert_eq!(name, "用户改的名字", "补齐模式绝不碰用户改过的显示名");
    assert_eq!(enabled, 0, "用户关掉的模型不许被打开");
    assert_eq!(max_bytes, Some(123));

    // ⑦ 对齐模式（点击按钮触发）：把**目录中已有的那些**能力对齐回来，
    //    但显示名 / enabled / 附件上限仍然不碰
    let (_, refreshed) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/builtin/sync",
        serde_json::json!({ "mode": "refresh-capabilities" }),
    )
    .await;
    assert_eq!(refreshed["added"], 0, "对齐模式绝不新增");
    assert!(refreshed["updated"].as_i64().unwrap() >= 0);
    let (name, enabled, max_bytes): (String, i64, Option<i64>) = state
        .with(|conn| {
            conn.query_row(
                "SELECT display_name, enabled, max_attachment_bytes FROM ai_models
                  WHERE provider_id=?1 AND model_id=?2",
                rusqlite::params![provider_id, model_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
        })
        .unwrap();
    assert_eq!(name, "用户改的名字");
    assert_eq!(enabled, 0);
    assert_eq!(max_bytes, Some(123));

    // ⑧ 未知 mode → 400（**不静默使用默认值**：否则「对齐能力」只补齐了缺失模型，
    //    而界面会显示"成功"）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/builtin/sync",
        serde_json::json!({ "mode": "whatever" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert_eq!(
        body["detail"],
        "未知的同步模式：whatever（可选 add-missing / refresh-capabilities）"
    );
}

/// `GET /api/ai/builtin/status` 在"provider 已在"时**要带 `providerId`**（键出现与否是形状的一部分）。
#[tokio::test]
async fn builtin_status_shape_changes_when_the_provider_exists() {
    let state = state();
    add_provider(&state, "p-shape", None);
    let (status, body) = get_json(router_for(state.clone()), "/api/ai/builtin/status").await;
    assert_eq!(status, StatusCode::OK);
    // 库里没有内置目录那个 provider → 仍然按"整份插入"算，但 `providerCreated=true` 且带 id
    assert_eq!(body["providerCreated"], true);
    assert!(!body["providerId"].as_str().unwrap().is_empty());
    assert!(keys(&body).contains(&"providerId".to_string()));
}

// ---------------------------------------------------------------------------
//  能力目录的状态与实测校准开关（P4 第 39 步）
// ---------------------------------------------------------------------------

/// `GET /api/ai/capabilities/status` 的形状 + `PUT .../probe-toggle` 的开关语义。
///
/// 两个承重事实：**`builtinRules` 必须与生成表一致**（119 条 = 118 + 手工补的 `deepseek-flash`；生成器漏规则就从这儿露馅），
/// 以及**开关只认 "1"**（认不出来的值一律按关：那是"会花钱发真请求"的开关）。
#[tokio::test]
async fn capabilities_status_reports_the_three_layers_and_toggles_probe() {
    let state = state();

    // ① 一层都没有时：只出现必有的键（三层概况与 remote 年龄**都不出现**）
    let (status, body) = get_json(router_for(state.clone()), "/api/ai/capabilities/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        keys(&body),
        vec![
            "builtinRules",
            "probeEnabled",
            "refreshIntervalDays",
            "remoteStale",
            "version"
        ],
        "没拉过在线目录时 remote 那几项**整个键不出现**（写成 0 小时会让界面显示『刚刚更新过』）"
    );
    assert_eq!(body["version"], "2026-09b");
    assert_eq!(
        body["builtinRules"], 119,
        "生成表 118 条 + 手工补的 deepseek-flash"
    );
    assert_eq!(body["refreshIntervalDays"], 7);
    assert_eq!(body["remoteStale"], true, "没拉过就算过期");
    assert_eq!(body["probeEnabled"], false, "实测校准默认关（会花钱）");

    // ② 开关：打开 / 关闭 —— 回的都是**刚回读**的那份状态 + 一句人话
    let (status, body) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/capabilities/probe-toggle",
        serde_json::json!({ "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["probeEnabled"], true);
    assert_eq!(body["lastMessage"], "已允许实测校准");
    let stored: String = state
        .with(|conn| {
            conn.query_row(
                "SELECT v FROM app_settings WHERE k='ai.capabilities.probe'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(
        stored, "1",
        "线上值就是 \"1\" / \"0\"（ThinkingProbe 认的就是这个）"
    );

    let (_, off) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/capabilities/probe-toggle",
        serde_json::json!({ "enabled": false }),
    )
    .await;
    assert_eq!(off["probeEnabled"], false);
    assert_eq!(off["lastMessage"], "已关闭实测校准");
    let stored: String = state
        .with(|conn| {
            conn.query_row(
                "SELECT v FROM app_settings WHERE k='ai.capabilities.probe'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(stored, "0");

    // 认不出来的值一律按**关**（手改库成 "yes" 也不许让花钱的开关被打开）
    state
        .with(|conn| {
            conn.execute(
                "UPDATE app_settings SET v='yes' WHERE k='ai.capabilities.probe'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let (_, body) = get_json(router_for(state.clone()), "/api/ai/capabilities/status").await;
    assert_eq!(body["probeEnabled"], false, "只有 \"1\" 算开");

    // ③ 两层覆盖层都在时：三层概况 + remote 的年龄与新鲜度
    let dir = state
        .media_dir()
        .parent()
        .unwrap()
        .join("ai")
        .join("capabilities");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("custom.json"),
        r#"{"source":"custom","entries":{"a":{},"b":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("remote.json"),
        r#"{"source":"models.dev","fetchedAt":"2026-09-25T00:00:00.000Z","entries":{"a":{}}}"#,
    )
    .unwrap();
    let (_, body) = get_json(router_for(state.clone()), "/api/ai/capabilities/status").await;
    assert_eq!(body["customSource"], "custom");
    assert_eq!(body["customEntries"], 2);
    assert_eq!(body["remoteSource"], "models.dev");
    assert_eq!(body["remoteEntries"], 1);
    assert_eq!(
        body["remoteFetchedAt"], "2026-09-25T00:00:00Z",
        "时间列要走 `Instant.toString()` 重排（`.000Z` 的小数点整个不出现）"
    );
    let age = body["remoteAgeHours"].as_i64().unwrap();
    assert!(age > 0, "拉取时间在过去 → 年龄是正数，实际 {age}");
    assert_eq!(body["remoteStale"], age > 7 * 24, "未过期时不许说它过期");

    // ④ 坏 JSON 当"该层不存在"，**不许 500**（目录坏了不能让设置页也打不开）
    std::fs::write(dir.join("custom.json"), "{ 这不是 JSON").unwrap();
    let (status, body) = get_json(router_for(state.clone()), "/api/ai/capabilities/status").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("customSource").is_none(), "坏的那层整个键都不出现");
    assert_eq!(body["remoteSource"], "models.dev", "另一层不受影响");
}

/// `refresh` **已经实现**（2026-09-27，`86d7e71`）：带 `COMFYHUB_NO_DOWNLOAD=1` 时
/// 它必须**如实跳过**（200 + `lastMessage` 说清原因），而不是去出网。
///
/// `probe`（实测校准，会对每个模型发真请求、花钱）**2026-09-27 已落地**（`capability_probe.rs`）
/// —— 这里改成正面的断言，盯住那条最要紧的纪律：**开关关着时一个上游请求都不发**。
/// "到底发了几次"由 `capability_probe.rs` 的注入式假 transport 用例钉死
/// （本文件只能通过 HTTP 打端点，看不到 transport 的调用次数）。
#[tokio::test]
async fn capability_refresh_is_implemented_and_probe_is_honest_when_disabled() {
    // ⚠ 只在这个用例里设一下、用完立刻撤（`std::env` 是进程级的，别的用例并行跑着）
    std::env::set_var("COMFYHUB_NO_DOWNLOAD", "1");
    let state = state();
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/capabilities/refresh",
        serde_json::json!({}),
    )
    .await;
    std::env::remove_var("COMFYHUB_NO_DOWNLOAD");
    assert_eq!(status, StatusCode::OK, "refresh 已经是实现：{body}");
    let message = body["lastMessage"].as_str().unwrap_or("");
    assert!(
        message.contains("COMFYHUB_NO_DOWNLOAD=1"),
        "关闭在线更新时要如实说清原因：{body}"
    );
    assert!(
        body["remoteSource"].is_null() || !body.to_string().contains("\"entries\""),
        "只是跳过，不该有数据集被写进来：{body}"
    );

    // ① 开关**默认关**：如实拒绝（200 + ok=false + 一句说清"会花钱"的话），不是 501
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/capabilities/probe",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "这是「一次正常执行、结果是没做」，不是服务器出错（501 会让界面的错误分支先跳出来）：{body}"
    );
    assert_eq!(
        keys(&body),
        vec!["message", "ok", "outcomes"],
        "键集合就是三个：{body}"
    );
    assert_eq!(body["ok"], false, "没校准成功就不许说成功：{body}");
    assert_eq!(
        body["outcomes"].as_array().unwrap().len(),
        0,
        "一个模型都没试：{body}"
    );
    let message = body["message"].as_str().unwrap();
    assert!(
        message.contains("真实请求"),
        "要说清它会对每个模型发真请求：{message}"
    );
    assert!(message.contains("费用"), "还必须说清会产生费用：{message}");

    // ② 库里有推理模型、开关也打开了 —— 只要**没有凭据又没有本机网关**，
    //    这条端点也只该如实回报（失败/跳过），**绝不许 500**、绝不许发明档位。
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_providers
                 (id, display_name, api, base_url, endpoint_trust, enabled, revision)
                 VALUES ('p-probe', '本机假网关', 'openai-completions', 'http://127.0.0.1:1/v1',
                         'loopback', 1, 1)",
                [],
            )?;
            conn.execute(
                r#"INSERT INTO ai_models (provider_id, model_id, display_name, input_modalities,
                                          reasoning, thinking_efforts, capability_source, enabled)
                   VALUES ('p-probe', 'dead-model', '连不上的模型', '["text"]', 1,
                           '{"off":"none","high":"high"}', 'builtin', 1)"#,
                [],
            )?;
            conn.execute(
                "INSERT INTO app_settings (k, v) VALUES ('ai.capabilities.probe', '1')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/capabilities/probe",
        serde_json::json!({ "providerId": "p-probe", "modelId": "dead-model" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "连不上是**如实回报**，不是 500：{body}"
    );
    let outcomes = body["outcomes"].as_array().unwrap();
    assert_eq!(outcomes.len(), 1, "{body}");
    assert_eq!(outcomes[0]["modelId"], "dead-model");
    assert_eq!(outcomes[0]["ok"], false);
    assert_eq!(outcomes[0]["written"], false);
    assert!(
        outcomes[0].get("acceptedTop").is_none(),
        "一个档位都没试成 → `acceptedTop` 连键都不许出现（更不许发明一个）：{body}"
    );
    assert!(
        outcomes[0].get("ladder").is_none(),
        "没试成就不许写回：{body}"
    );
    assert!(
        !outcomes[0]["message"].as_str().unwrap_or("").is_empty(),
        "失败必须带一句为什么：{body}"
    );
    // 端口 1 上没有东西 → 这条校准不该改动 `custom.json`（它本来也不存在）
    let custom = state
        .media_dir()
        .parent()
        .unwrap()
        .join("ai")
        .join("capabilities")
        .join("custom.json");
    assert!(
        !custom.exists(),
        "写不成 / 没试成都不许凭空造一份手工目录：{}",
        custom.display()
    );
}

/// 凭据端点（`/api/ai/providers/{id}/credentials`）的完整一轮：状态 → 校验 → 写入 → 删除。
///
/// 这一组盯的是**三件会静默做错的事**：
/// ① 响应里混进了明文（AIH-012：任何 DTO 都不含密钥值）—— 用哨兵串在响应与文件里各搜一次；
/// ② 空值被当成"清空"而不是"不修改"（AIH-013）—— 空值那一步之后状态必须**不变**；
/// ③ 写进去的密文对方解不开（字母表/作用域错了照样能写出一个文件）——
///    这里用**自己的** DPAPI 反解一次（真机交叉验证在真库副本那一轮做）。
#[tokio::test]
async fn provider_credentials_round_trip_without_ever_returning_the_value() {
    let state = state();
    // ref 里带 pid：万一哪台机器上真设了同名环境变量，也只影响这一条用例
    let reference = format!("FIXTURE_KEY_{}", std::process::id());
    add_provider(&state, "p-cred", Some(&reference));

    // ① 初始：没有值 + 可写（`none` 档，界面能写一把新的）
    let (status, body) = get_json(
        router_for(state.clone()),
        "/api/ai/providers/p-cred/credentials",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys(&body), vec!["configured", "source", "writable"]);
    assert_eq!(body["configured"], false);
    assert_eq!(body["source"], "none");
    assert_eq!(body["writable"], true);

    // ② 输入校验：粘贴整行 NAME=value 要**报错**，而不是把它当密钥存下来
    let (status, body) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/providers/p-cred/credentials",
        serde_json::json!({ "value": "OPENAI_API_KEY=sk-sentinel-1234567890" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR", "AI 错误：error 是稳定码");
    assert_eq!(
        body["detail"],
        "请只粘贴密钥的值，不要粘贴 NAME=value 这种整行"
    );
    assert_eq!(keys(&body), vec!["detail", "error"], "detail 不许为空值");

    // ③ 空值 = 不修改（不是清空）：回状态、且库里仍然没有这个 ref
    let (status, body) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/providers/p-cred/credentials",
        serde_json::json!({ "value": "   " }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["configured"], false);
    let store_file = state
        .media_dir()
        .parent()
        .unwrap()
        .join("ai")
        .join("credentials.dpapi.json");
    assert!(
        comfyhub_server_rs::provider::credentials::read_store(&store_file).is_empty(),
        "留空必须**一个字节都不写**（AIH-012）"
    );

    // ④ 真写一把：状态翻转成 managed，且**明文不出现在响应里、也不出现在文件里**
    let sentinel = "sk-sentinel-1234567890";
    let (status, body) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/providers/p-cred/credentials",
        serde_json::json!({ "value": sentinel }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["configured"], true);
    assert_eq!(body["source"], "managed");
    assert_eq!(body["writable"], true);
    assert!(!body.to_string().contains(sentinel), "响应里不许出现明文");

    let raw_file = std::fs::read_to_string(&store_file).expect("凭据文件应当已经落盘");
    assert!(
        !raw_file.contains(sentinel),
        "文件里不许出现明文（AIH-015）"
    );
    let store = comfyhub_server_rs::provider::credentials::read_store(&store_file);
    let sealed_text = store.get(&reference).expect("这个 ref 应当已入库").clone();
    // 字母表绊线：Kotlin 用的是 `Base64.getEncoder()`（标准 `+/`），写成 url-safe 的话
    // Kotlin 侧解不开 —— 随机密文里出现 `-`/`_` 是必然事件，所以这条能真的挡住改错的那天。
    assert!(
        !sealed_text.contains('-') && !sealed_text.contains('_'),
        "必须是**标准** base64（`+/`），不能是 url-safe：{sealed_text}"
    );
    assert_eq!(
        sealed_text.len() % 4,
        0,
        "标准 base64 的长度一定是 4 的倍数（带填充）"
    );
    // 编码路径只有一处：`base64_std` 出来的东西必须就是库里那份（改字母表/去掉填充都会红）
    let decoded = comfyhub_server_rs::mobile::auth::base64_decode(&sealed_text).expect("base64");
    assert_eq!(
        comfyhub_server_rs::mobile::auth::base64_std(&decoded),
        sealed_text
    );

    // 自己反解一次：证明写出去的确实是"本机 DPAPI 能解开的密文"
    #[cfg(windows)]
    {
        let bytes = comfyhub_server_rs::mobile::auth::base64_decode(&sealed_text).expect("base64");
        let plain = comfyhub_server_rs::mobile::auth::unprotect_secret(&bytes).expect("DPAPI 解密");
        assert_eq!(String::from_utf8(plain).unwrap(), sentinel);
    }

    // ⑤ 再读一次：状态是"现场算的"（不是把上一次的响应缓存下来）
    let (status, body) = get_json(
        router_for(state.clone()),
        "/api/ai/providers/p-cred/credentials",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["configured"], true);
    assert_eq!(body["source"], "managed");

    // ⑥ 删除：状态回到 none，文件里那把 key 真的没了（不留空壳）
    let (status, body) = send_empty(
        router_for(state.clone()),
        "DELETE",
        "/api/ai/providers/p-cred/credentials",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["configured"], false);
    assert_eq!(body["source"], "none");
    assert!(
        !comfyhub_server_rs::provider::credentials::read_store(&store_file)
            .contains_key(&reference)
    );
}

/// 删除 provider 时**托管密钥随之处理**（现象：「删除 provider 时，密钥被设置为
/// 不自动删除」）：以前 provider 删了，`credentials.dpapi.json` 里那把 key 成了孤儿 ——
/// 界面上再也摸不到，却还留在盘上。两条会静默做错的边界各有一条断言：
/// ① 无人引用 → key 一并清掉；② ref 被别的 provider 共用 → 留着（删的是引用不是 key）。
/// （env 来源那条路在 `unset_orphan_credential` 里被 `env_source` 挡住 —— 往测试进程里
/// 塞环境变量会泄漏到别的用例，不值得，那里只有一行。）
#[tokio::test]
async fn deleting_a_provider_takes_its_orphan_credential_with_it() {
    let state = state();
    let reference = format!("FIXTURE_ORPHAN_KEY_{}", std::process::id());
    let store_file = state
        .media_dir()
        .parent()
        .unwrap()
        .join("ai")
        .join("credentials.dpapi.json");

    // ① 无人引用：删 provider → key 一起没了
    add_provider(&state, "p-orphan", Some(&reference));
    let (status, _) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/providers/p-orphan/credentials",
        serde_json::json!({ "value": "sk-orphan-1234567890" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        comfyhub_server_rs::provider::credentials::read_store(&store_file).contains_key(&reference)
    );

    let (status, _) = send_empty(
        router_for(state.clone()),
        "DELETE",
        "/api/ai/providers/p-orphan",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !comfyhub_server_rs::provider::credentials::read_store(&store_file)
            .contains_key(&reference),
        "provider 删掉后它那把无人引用的 key 应当一并清掉"
    );

    // ② ref 被别的 provider 共用：删一个，key 留着（删的是引用，不是 key 本身）
    add_provider(&state, "p-share-a", Some(&reference));
    add_provider(&state, "p-share-b", Some(&reference));
    let (status, _) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/providers/p-share-a/credentials",
        serde_json::json!({ "value": "sk-shared-1234567890" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send_empty(
        router_for(state.clone()),
        "DELETE",
        "/api/ai/providers/p-share-a",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        comfyhub_server_rs::provider::credentials::read_store(&store_file).contains_key(&reference),
        "ref 还有别的 provider 在用，key 必须留着"
    );
    let (status, _) = send_empty(
        router_for(state.clone()),
        "DELETE",
        "/api/ai/providers/p-share-b",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !comfyhub_server_rs::provider::credentials::read_store(&store_file)
            .contains_key(&reference),
        "最后一个引用也删掉后，key 才跟着走"
    );
}

/// 凭据端点的**三段前置判定**：provider 不存在 / 没配 credentialRef —— 都必须是
/// `400 CONFIG_ERROR + detail`（人话），**不是 404**（写成 404 就没有 detail，
/// 界面又一次显示不出人话，P4 第 17 步那条对齐）。
#[tokio::test]
async fn credential_endpoints_report_config_errors_as_ai_errors() {
    let state = state();
    add_provider(&state, "p-no-ref", None);

    let (status, body) = get_json(
        router_for(state.clone()),
        "/api/ai/providers/nope/credentials",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert_eq!(body["detail"], "Provider 不存在: nope");

    for method in ["PUT", "DELETE"] {
        let (status, body) = if method == "PUT" {
            send_json(
                router_for(state.clone()),
                "PUT",
                "/api/ai/providers/p-no-ref/credentials",
                serde_json::json!({ "value": "sk-x" }),
            )
            .await
        } else {
            send_empty(
                router_for(state.clone()),
                "DELETE",
                "/api/ai/providers/p-no-ref/credentials",
            )
            .await
        };
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method}");
        assert_eq!(body["error"], "CONFIG_ERROR", "{method}");
        assert_eq!(
            body["detail"], "该 Provider 未设置凭据引用名（credentialRef）",
            "{method}"
        );
    }
}

/// 联网搜索那把 key 的三条端点（免密钥出厂可用，这里只是个可选入口）。
///
/// ⚠ 这台机器上若设了 `keenable` / `COMFYHUB_CRED_keenable`，那"这一档只读"就是唯一正确的行为 ——
/// 用例按环境**两条路都钉住**，不然它在有环境变量的机器上必然红（而那不是 bug）。
#[tokio::test]
async fn web_search_credential_is_write_only_and_read_only_when_env_provides_it() {
    let state = state();
    let from_env = comfyhub_server_rs::provider::credentials::env_source(
        comfyhub_server_rs::provider::credentials::KEENABLE_REF,
    )
    .is_some();

    let (status, body) = get_json(router_for(state.clone()), "/api/ai/web-search/credential").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys(&body), vec!["configured", "source", "writable"]);
    assert_eq!(body["source"], if from_env { "env" } else { "none" });
    assert_eq!(body["writable"], !from_env, "env 来源只读（AIH-013）");

    let put = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/ai/web-search/credential",
        serde_json::json!({ "value": "k-sentinel-abc" }),
    )
    .await;
    if from_env {
        assert_eq!(put.0, StatusCode::BAD_REQUEST);
        assert_eq!(put.1["error"], "CONFIG_ERROR");
        assert_eq!(
            put.1["detail"],
            "密钥 keenable 由环境变量（COMFYHUB_CRED_keenable）提供，是只读的；请先移除环境变量再在应用内填写"
        );
        let del = send_empty(
            router_for(state.clone()),
            "DELETE",
            "/api/ai/web-search/credential",
        )
        .await;
        assert_eq!(del.0, StatusCode::BAD_REQUEST);
        assert_eq!(
            del.1["detail"],
            "密钥 keenable 由环境变量提供，无法在应用内移除"
        );
        return;
    }

    assert_eq!(put.0, StatusCode::OK);
    assert_eq!(put.1["configured"], true);
    assert_eq!(put.1["source"], "managed");
    assert!(!put.1.to_string().contains("k-sentinel-abc"));

    let del = send_empty(
        router_for(state.clone()),
        "DELETE",
        "/api/ai/web-search/credential",
    )
    .await;
    assert_eq!(del.0, StatusCode::OK);
    assert_eq!(del.1["configured"], false);
    assert_eq!(del.1["source"], "none");
}

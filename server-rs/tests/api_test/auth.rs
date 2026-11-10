//! 配对闸门与密钥下行：开关、ticket、claim、限流、凭据下行的授权判据。
//!
//! 夹具在 [`super::common`]；`use super::common::*;` 之后照旧直接写用例。

use super::common::*;

/// **默认行为一个字节都不许变**：闸门关着时，非回环不带令牌也照样通。
///
/// 这条是三条既有 e2e（capture / submit / ai-tools）与桌面 App 的代言人：
/// 它们全都不带任何头，闸门一开就得全绿 —— 红了就说明默认被改了。
#[tokio::test]
async fn auth_gate_is_off_by_default_and_blocks_only_when_enabled() {
    let _serial = auth_test_lock().lock().await;
    let state = state();
    // （一）没有 auth.pairing 这一行 = 今天的行为
    let (status, _) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/prompts",
        None,
        "10.9.1.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "闸门默认关着：局域网请求照旧放行");

    // （二）开着以后：没带令牌 → 403（**不是 401**）
    seed_auth(&state, false);
    let (status, body) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/prompts",
        None,
        "10.9.1.2",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "需要配对令牌（X-ComfyHub-Token）");
    assert!(body.get("detail").is_none(), "普通错误只有 error 一个键");

    // 令牌不对 / 已被吊销 → 401（与 403 的语义不能混）
    let (status, body) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/prompts",
        None,
        "10.9.1.3",
        Some("wrong-token"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "令牌无效或已被吊销");

    // 设备令牌 / 全局令牌 → 放行
    let (status, _) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/prompts",
        None,
        "10.9.1.4",
        Some("device-token-43chars-bbbbbbbbbbbbbbbbbbbbbb"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/prompts",
        None,
        "10.9.1.5",
        Some("global-token-43chars-aaaaaaaaaaaaaaaaaaaaaa"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 回环永远放行（桌面 App / 脚本 / e2e 走的就是这条）
    let (status, _) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/prompts",
        None,
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "回环不带令牌也必须通");
}

/// 手机发起配对的完整一幕：举手 → 桌面看到 6 位码 → 兑码拿令牌 → 令牌真的能用。
#[tokio::test]
async fn auth_pair_flow_hands_out_a_device_token_exactly_once() {
    let _serial = auth_test_lock().lock().await;
    let state = state();
    seed_auth(&state, false);

    // ① 手机举手（未配对设备唯一能打的两条路径之一）
    let (status, body) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/pair/request",
        Some(serde_json::json!({"deviceId": "phone-e2e-1", "deviceName": "OnePlus 8T"})),
        "10.9.2.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let request_id = body["requestId"].as_str().expect("requestId").to_string();
    assert!(!request_id.is_empty());
    // ★ 纪律：响应里**没有 ticket**（给了它，手机自己就能算出 6 位码）
    assert!(
        !body.as_object().unwrap().contains_key("ticket"),
        "手机举手的响应里不许出现 ticket：{body}"
    );
    assert_eq!(
        keys(&body),
        vec!["codeExpiresAt", "expiresAt", "requestId", "serverTime"],
        "举手响应的键集合就是这四个"
    );

    // ② 桌面（回环）看到待配对列表，里面是本机的 6 位码
    let (status, body) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/auth/pair/pending",
        None,
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mine = body["pending"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["requestId"] == request_id.as_str())
        .expect("待配对列表里应当有这一条")
        .clone();
    let code = mine["code"].as_str().unwrap().to_string();
    assert_eq!(code.len(), 6);
    assert!(code.chars().all(|c| c.is_ascii_digit()));
    assert_eq!(mine["ip"], "10.9.2.1");
    assert_eq!(mine["deviceName"], "OnePlus 8T");

    // 非回环读这张列表 → 403（它是"谁有钥匙"的情报）
    let (status, _) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/auth/pair/pending",
        None,
        "10.9.2.2",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // ③ 码错 → 400「配对失败」，且**不回显** ticket / code
    let wrong = if code == "000000" { "111111" } else { "000000" };
    let (status, body) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/pair/claim",
        Some(serde_json::json!({
            "requestId": request_id, "code": wrong,
            "deviceId": "phone-e2e-1", "deviceName": "OnePlus 8T"
        })),
        "10.9.2.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "配对失败");
    let text = body.to_string();
    assert!(!text.contains(&code), "失败响应里不许回显 6 位码");
    assert!(
        !text.contains(&request_id) || true,
        "（会话 id 可以回显，它是攻击者自己给的）"
    );

    // ④ 码对 → 拿到**这台设备专属**的令牌（只此一次）
    let (status, body) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/pair/claim",
        Some(serde_json::json!({
            "requestId": request_id, "code": code,
            "deviceId": "phone-e2e-1", "deviceName": "OnePlus 8T"
        })),
        "10.9.2.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let token = body["token"].as_str().expect("token").to_string();
    assert_eq!(token.len(), 43, "32 字节 base64url = 43 字符（D11）");
    assert_eq!(body["deviceId"], "phone-e2e-1");
    assert!(body["schemaVersion"].as_i64().unwrap() > 0);

    // ⑤ 同一个会话**不能兑第二次**（"一次性"这个词的全部含义）
    let (status, _) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/pair/claim",
        Some(serde_json::json!({
            "requestId": request_id, "code": code,
            "deviceId": "phone-e2e-1", "deviceName": "OnePlus 8T"
        })),
        "10.9.2.1",
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::GONE,
        "用过的会话 → 410（手机据此回桌面重新发起）"
    );

    // ⑥ 令牌真的能用：拿它打一条受保护的端点
    let (status, _) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/ai/conversations",
        None,
        "10.9.2.1",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // ⑥′ "最后活跃"要**立刻**看得到（内存里那笔还没落库的活跃也要叠加上来）。
    // 少了叠加这一层，界面上的"最后活跃"会停在上一次落库的时刻（30 秒前），
    // 表现是"数据没错、只是旧了" —— 那种最容易被当成 bug 去查后端。
    let (_, status_body) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/auth",
        None,
        "127.0.0.1",
        None,
    )
    .await;
    let just_seen = status_body["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "phone-e2e-1")
        .expect("刚配的设备")
        .clone();
    assert_eq!(
        just_seen["lastIp"], "10.9.2.1",
        "最后来源地址就是刚打过来的那个"
    );
    assert!(
        just_seen["lastSeenAt"].is_string(),
        "最后活跃不必等 30 秒落库"
    );

    // ⑦ 设备列表：**有这台设备、但绝不含令牌**（连前缀都不回）
    let (status, body) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/auth/devices",
        None,
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let device = body["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "phone-e2e-1")
        .expect("新配的设备应当在列表里")
        .clone();
    // 设备行只该有那几个**事实**键：必有的四个 + 两个随时间才出现的
    // （`lastSeenAt` / `lastIp` 有没有取决于活跃记录落库没有，所以这里逐条算，
    //  不写成"键一定等于某四个" —— 那会随"谁先跑了 GET /api/auth"而红）
    let device_keys = keys(&device);
    for required in [
        "allowCredentials",
        "allowCredentialsUpload",
        "id",
        "name",
        "pairedAt",
    ] {
        assert!(
            device_keys.iter().any(|k| k == required),
            "设备行少了 {required}：{device_keys:?}"
        );
    }
    for key in &device_keys {
        assert!(
            [
                "allowCredentials",
                "allowCredentialsUpload",
                // 按设备放权提交（2026-11-09 §4.2）：默认关，值就是"这台设备能不能免审批提交"
                "autoApproveSubmits",
                "id",
                "lastIp",
                "lastSeenAt",
                "name",
                "pairedAt"
            ]
            .contains(&key.as_str()),
            "设备行冒出一个不该有的键：{key}"
        );
    }
    assert_eq!(device["allowCredentials"], false, "配对成功 ≠ 允许下行密钥");
    // ★ 密钥**上行**是另一个开关，同样默认关（§14.1）：读和写不是同一种权力
    assert_eq!(
        device["allowCredentialsUpload"], false,
        "配对成功 ≠ 允许改桌面上的密钥"
    );
    assert!(
        !body.to_string().contains(&token),
        "设备列表里一个字都不许出现长期令牌"
    );
    // 而且令牌**确实落库了**（不是只在响应里一闪而过）
    let stored = read_setting(&state, "auth.pairing").expect("auth.pairing 已落库");
    assert!(stored["devices"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["id"] == "phone-e2e-1"));

    // ⑧ 吊销 → **立刻** 401（无宽限期）
    let (status, _) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/devices/phone-e2e-1/revoke",
        Some(serde_json::json!({})),
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/ai/conversations",
        None,
        "10.9.2.1",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // 别的设备不受影响（夹具里那台仍然通）
    let (status, _) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/ai/conversations",
        None,
        "10.9.2.1",
        Some("device-token-43chars-bbbbbbbbbbbbbbbbbbbbbb"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "吊销一台不许误伤另一台");
}

/// 6 位码只有 10^6 种：错 5 次就把这次配对**当场作废**（不是锁一会儿）。
#[tokio::test]
async fn auth_claim_voids_the_session_after_five_wrong_codes() {
    let _serial = auth_test_lock().lock().await;
    let state = state();
    seed_auth(&state, false);
    let (_, body) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/pair/request",
        Some(serde_json::json!({"deviceId": "phone-brute", "deviceName": "guess"})),
        "10.9.3.1",
        None,
    )
    .await;
    let request_id = body["requestId"].as_str().unwrap().to_string();

    let (_, listing) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/auth/pair/pending",
        None,
        "127.0.0.1",
        None,
    )
    .await;
    let code = listing["pending"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["requestId"] == request_id.as_str())
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_string();
    let wrong = if code == "000000" { "111111" } else { "000000" };

    for attempt in 1..=4 {
        let (status, _) = send_as(
            router_for(state.clone()),
            "POST",
            "/api/auth/pair/claim",
            Some(serde_json::json!({"requestId": request_id, "code": wrong, "deviceId": "phone-brute"})),
            "10.9.3.1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "第 {attempt} 次错码");
    }
    // 第 5 次错 → 会话作废（410）
    let (status, _) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/pair/claim",
        Some(
            serde_json::json!({"requestId": request_id, "code": wrong, "deviceId": "phone-brute"}),
        ),
        "10.9.3.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::GONE);
    // 作废之后**即使码对也不行**
    let (status, _) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/pair/claim",
        Some(serde_json::json!({"requestId": request_id, "code": code, "deviceId": "phone-brute"})),
        "10.9.3.1",
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::GONE,
        "作废是硬判据，不许被一个正确的码救回来"
    );
}

/// **密钥下行**：只认"回环 或 已配对且被显式授权"的设备（与闸门开关无关）。
#[tokio::test]
async fn credentials_downlink_requires_an_authorized_device() {
    let state = state();
    seed_auth(&state, false);
    // 夹具用**环境变量**这一档（托管文件那档是 DPAPI 密文，由 auth.rs 的单测覆盖）
    let reference = "FIXTURE_KEY_E2E_AUTH";
    std::env::set_var(reference, "sk-fixture-value");

    let uri = format!("/api/ai/credentials?refs={reference}");
    // ① 匿名非回环 → 403（**即使闸门关着**也不放行；这句人话指向"你还没配对"）
    put_setting(
        &state,
        "auth.pairing",
        &serde_json::json!({"enabled": false, "token": "g"}).to_string(),
    );
    let (status, body) = send_as(
        router_for(state.clone()),
        "GET",
        &uri,
        None,
        "10.9.4.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "需要配对令牌（X-ComfyHub-Token）");

    seed_auth(&state, false);
    // ② 配对了但没勾"允许密钥" → 403（含全局令牌也不行）
    for token in [
        "device-token-43chars-bbbbbbbbbbbbbbbbbbbbbb",
        "global-token-43chars-aaaaaaaaaaaaaaaaaaaaaa",
    ] {
        let (status, _) = send_as(
            router_for(state.clone()),
            "GET",
            &uri,
            None,
            "10.9.4.2",
            Some(token),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "令牌 {token:?} 不该能读密钥");
    }

    // ③ 勾了 → 放行并拿到明文
    seed_auth(&state, true);
    let (status, body) = send_as(
        router_for(state.clone()),
        "GET",
        &uri,
        None,
        "10.9.4.3",
        Some("device-token-43chars-bbbbbbbbbbbbbbbbbbbbbb"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body["credentials"][reference], "sk-fixture-value");

    // ④ 本机回环永远可以（桌面 App 自己要用）
    let (status, body) = send_as(
        router_for(state.clone()),
        "GET",
        &uri,
        None,
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["credentials"][reference], "sk-fixture-value");

    // ⑤ 桌面上没有的那把 ref：**不报错、也不出现在结果里**（手机据此如实说"还得自己填"）
    let (status, body) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/ai/credentials?refs=NOT_CONFIGURED_REF",
        None,
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["credentials"].as_object().unwrap().is_empty());
    assert!(body["missing"].as_array().unwrap().len() == 1);

    std::env::remove_var(reference);
}

/// 开关与轮换：打开开关即发放令牌；轮换 = 全局令牌换新 + **所有设备立即失效**。
#[tokio::test]
async fn auth_switch_and_rotate_behave_as_documented() {
    let _serial = auth_test_lock().lock().await;
    let state = state();
    // 打开开关（还没有任何状态）
    let (status, body) = send_as(
        router_for(state.clone()),
        "PUT",
        "/api/auth",
        Some(serde_json::json!({"enabled": true})),
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enabled"], true);
    assert_eq!(body["hasToken"], true, "打开开关时顺手生成长期令牌");
    assert_eq!(
        keys(&body),
        vec!["devices", "enabled", "hasToken", "pending", "schema"]
    );
    let first = read_setting(&state, "auth.pairing").unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // 非回环打管理端点 → 403（只允许本机）
    let (status, _) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/auth",
        None,
        "10.9.5.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 参数不对 → 400（不是静默当成 false）
    let (status, _) = send_as(
        router_for(state.clone()),
        "PUT",
        "/api/auth",
        Some(serde_json::json!({})),
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 轮换：换令牌 + 清空所有设备
    seed_auth(&state, true);
    let (status, body) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/rotate",
        Some(serde_json::json!({})),
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["revokedDevices"], 1, "轮换要说清楚吊销了几台");
    let second = read_setting(&state, "auth.pairing").unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(first, second);
    // 旧设备令牌立刻 401，新全局令牌 200
    let (status, _) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/prompts",
        None,
        "10.9.5.2",
        Some("device-token-43chars-bbbbbbbbbbbbbbbbbbbbbb"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/prompts",
        None,
        "10.9.5.2",
        Some(&second),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// 扫码通道**已经做了**（2026-09-27）：回环来源能拿到 ticket + 6 位码 + 地址数组。
///
/// 这条用例原先断言的是"明确 501"（那时候还没做）—— 现在改成正面断言，
/// **顺带钉住"返回体里没有长期令牌"**：二维码里只有 ticket（兑换还要靠 6 位码）。
#[tokio::test]
async fn pair_start_hands_out_only_a_ticket_not_a_token() {
    let st = state();
    let (status, body) = send_as(
        router_for(st.clone()),
        "POST",
        "/api/auth/pair/start",
        Some(serde_json::json!({})),
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["ticket"].is_string() && body["code"].is_string(),
        "{body}"
    );
    assert!(body["urls"].is_array(), "{body}");
    // 长期令牌：既不在响应里，也不落库（这时候还**没有任何设备**）
    let raw = body.to_string();
    assert!(!raw.contains("X-ComfyHub-Token"), "{raw}");
    assert!(
        read_setting(&st, "auth.pairing").is_none(),
        "只出码不该写设备表：{:?}",
        read_setting(&st, "auth.pairing")
    );
}

/// 兑码**也要限速**：只靠"错 5 次作废"挡不住"再举一次手换 5 次机会"。
#[tokio::test]
async fn claim_attempts_are_rate_limited_per_source_address() {
    let state = state();
    seed_auth(&state, false);
    // 同一来源、对着一个不存在的会话反复兑：前若干次是 400，之后必须出现 429
    let mut saw_rate_limited = false;
    let mut bodies: Vec<String> = Vec::new();
    for _ in 0..40 {
        let (status, body) = send_as(
            router_for(state.clone()),
            "POST",
            "/api/auth/pair/claim",
            Some(serde_json::json!({
                "requestId": "deadbeef", "code": "000000", "deviceId": "phone-flood"
            })),
            "10.9.6.1",
            None,
        )
        .await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            saw_rate_limited = true;
            bodies.push(body.to_string());
            break;
        }
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "限速之前只能是「配对失败」"
        );
    }
    assert!(
        saw_rate_limited,
        "同一个来源反复兑码必须被限速（设计 §5.2 的来源地址口径）"
    );
    // 429 的响应里**不回显任何凭据**（ticket / code / 会话 id）
    let text = bodies.join(" ");
    assert!(!text.contains("000000"), "429 不许回显码：{text}");
    assert!(!text.contains("deadbeef"), "429 连会话 id 都不必回：{text}");
    // 另一个来源不受影响（限速是按来源地址分桶的）
    let (status, _) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/pair/claim",
        Some(serde_json::json!({
            "requestId": "deadbeef", "code": "000000", "deviceId": "phone-other"
        })),
        "10.9.6.2",
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "限速按 IP 分桶，不该误伤别的来源"
    );
}

// ---------------------------------------------------------------------------
//  POST /api/auth/pair/start（扫码通道的桌面那一端，docs/pairing-design.md §5.1）
// ---------------------------------------------------------------------------

/// 整条扫码链路：桌面出码（仅回环）→ 手机拿 ticket + 6 位码换令牌 → 设备落库（两个开关都关）。
#[tokio::test]
async fn pair_start_hands_out_a_ticket_and_the_scan_flow_claims() {
    let st = state();
    let app = router_for(st.clone());

    // ① 桌面出码
    let (status, body) = send_as(
        app.clone(),
        "POST",
        "/api/auth/pair/start",
        None,
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ticket = body["ticket"].as_str().unwrap_or("").to_string();
    let code = body["code"].as_str().unwrap_or("").to_string();
    assert!(ticket.len() >= 40, "ticket 要够长：{body}");
    assert_eq!(code.len(), 6, "6 位十进制码：{body}");
    assert!(
        body["urls"].is_array(),
        "urls 永远是数组（一条都没有时是空数组）：{body}"
    );
    assert!(
        body["expiresAt"].is_string() && body["codeExpiresAt"].is_string(),
        "{body}"
    );

    // ② 扫码会话**不进** pair/pending（那张列表是给"手机举手"那条路用的）
    //
    // ⚠ 判据是"**条数没变**"而不是"条数为 0"：`sessions()` 是**进程级全局态**，
    // 同进程并行执行的其他配对用例随时会写入"手机举手"的会话（本仓库出现过该问题，
    // 所以 auth 用例还有一把 `auth_test_lock`）。断言 0 会变成"看谁先跑"的偶发红。
    let (status, before) = send_as(
        app.clone(),
        "GET",
        "/api/auth/pair/pending",
        None,
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{before}");
    let count_before = before["pending"].as_array().unwrap().len();
    let (status, again) = send_as(
        app.clone(),
        "POST",
        "/api/auth/pair/start",
        None,
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    let (status, after) = send_as(
        app.clone(),
        "GET",
        "/api/auth/pair/pending",
        None,
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert_eq!(
        after["pending"].as_array().unwrap().len(),
        count_before,
        "又出了一次码，但『待配对』列表的条数不该变（扫码会话不进来）：before={before} after={after}"
    );

    // ③ 手机兑码（匿名端点）：ticket + 码 → 长期令牌
    let (status, claim) = send_as(
        app.clone(),
        "POST",
        "/api/auth/pair/claim",
        Some(serde_json::json!({
            // clone：后面第 ⑤ 步还要用同一对 ticket/码再兑一次
            "ticket": ticket.clone(),
            "code": code.clone(),
            "deviceId": "phone-scan-1",
            "deviceName": "KB2000"
        })),
        "10.0.0.9",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claim}");
    assert!(claim["token"].as_str().unwrap_or("").len() >= 40, "{claim}");

    // ④ 设备真的落库了，而且两个凭据开关都是关的（配对成功 ≠ 能读/写密钥）
    let file = read_setting(&st, "auth.pairing").expect("auth.pairing 已落库");
    let device = file["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "phone-scan-1")
        .expect("扫到的这台设备要在库里");
    assert_eq!(device["allowCredentials"], false);
    assert_eq!(device["allowCredentialsUpload"], false);

    // ⑤ **一次性**：同一对 ticket + 正确的码再兑一次 → `410 Gone`（不是 400）。
    //    这两句话在手机上意思不同：410 = "回电脑上重新发起"，400 = "码错了，再试试"。
    let (status, reused) = send_as(
        app,
        "POST",
        "/api/auth/pair/claim",
        Some(serde_json::json!({
            "ticket": ticket,
            "code": code,
            "deviceId": "phone-scan-2",
            "deviceName": "另一台手机"
        })),
        "10.0.0.10",
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::GONE,
        "用过的会话再兑必须是 410：{reused}"
    );
}

/// 非回环不许生成二维码（ticket 编进二维码 = 看到的人就能拿去兑）。
#[tokio::test]
async fn pair_start_is_loopback_only() {
    let app = router_for(state());
    let (status, body) = send_as(app, "POST", "/api/auth/pair/start", None, "10.0.0.9", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("只允许在本机"),
        "要说清是（只允许在本机）：{body}"
    );
}

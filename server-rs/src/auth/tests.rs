//! `auth` 的单元测试（主体代码在 [`auth`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn file_enabled() -> AuthFile {
    AuthFile {
        enabled: true,
        token: Some("global-token".into()),
        devices: vec![DeviceRecord {
            id: "phone-1".into(),
            name: "OnePlus".into(),
            token: "device-token".into(),
            allow_credentials: true,
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn hmac_matches_rfc4231_vector() {
    // RFC 4231 Test Case 2
    let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
    assert_eq!(
        mac.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
}

#[test]
fn base64url_and_decode_round_trip() {
    // 32 字节 → 43 字符、无填充（D11 的令牌形状）
    let token = base64url(&[0u8; 32]);
    assert_eq!(token.len(), 43);
    assert!(!token.contains('='));
    assert_eq!(base64_decode(&token).unwrap(), vec![0u8; 32]);
    // Kotlin 写的是**标准** base64（带 + / 与填充），解码器两种都要认
    assert_eq!(base64_decode("+/8=").unwrap(), vec![0xfb, 0xff]);
}

#[test]
fn rolling_code_is_six_digits_and_window_scoped() {
    let code = code_for("ticket", 1_700_000_000);
    assert_eq!(code.len(), 6);
    assert!(code.chars().all(|c| c.is_ascii_digit()));
    // 同一个 60 秒窗口内恒定、跨窗口必变（滚动的唯一含义）。
    // ⚠ 窗口是按 `unix / 60` 切的，不是"从第一次请求起算 60 秒"：
    //    1_700_000_000 落在窗口的第 20 秒，所以 +39 还在同一格、+40 就换格了。
    assert_eq!(code_for("ticket", 1_700_000_000), code_for("ticket", 1_700_000_039));
    assert_ne!(code_for("ticket", 1_700_000_000), code_for("ticket", 1_700_000_040));
    assert_ne!(code_for("ticket", 1_700_000_000), code_for("ticket", 1_700_000_060));
    // 换 ticket 就换码（"作废 ticket 自动作废码"）
    assert_ne!(code_for("ticket", 1_700_000_000), code_for("other", 1_700_000_000));
}

#[test]
fn gate_lets_anonymous_pairing_paths_through() {
    // 闸门开着、没有令牌：这两条路径必须放行（它们就是给未配对设备用的）
    for path in [PAIR_REQUEST_PATH, PAIR_CLAIM_PATH] {
        assert!(matches!(
            decide(path, "POST", Some("10.0.0.9".parse().unwrap()), None, &file_enabled()),
            Verdict::Allow(_)
        ));
    }
}

#[test]
fn gate_forbids_and_unauthorizes_differently() {
    let remote = Some("10.0.0.9".parse().unwrap());
    // 没带令牌 → 403（不是 401）
    match decide("/api/prompts", "GET", remote, None, &file_enabled()) {
        Verdict::Forbidden(_) => {}
        other => panic!("期望 403，得到 {other:?}"),
    }
    // 带了错的 → 401
    match decide("/api/prompts", "GET", remote, Some("nope"), &file_enabled()) {
        Verdict::Unauthorized(_) => {}
        other => panic!("期望 401，得到 {other:?}"),
    }
    // 对的 → 放行
    assert_eq!(
        decide("/api/prompts", "GET", remote, Some("device-token"), &file_enabled()),
        Verdict::Allow(Grant::Device { id: "phone-1".into(), allow_credentials: true, allow_credentials_upload: false })
    );
    // 全局令牌也算
    assert_eq!(
        decide("/api/prompts", "GET", remote, Some("global-token"), &file_enabled()),
        Verdict::Allow(Grant::Global)
    );
}

#[test]
fn gate_off_keeps_todays_behaviour() {
    // **不许把默认行为改掉**：关着的时候，非回环不带令牌也照样通
    let file = AuthFile::default();
    assert!(!file.enabled);
    assert!(matches!(
        decide("/api/prompts", "GET", Some("10.0.0.9".parse().unwrap()), None, &file),
        Verdict::Allow(_)
    ));
    // 三条既有 e2e / 桌面 App 走的是回环那条
    assert!(matches!(
        decide("/api/prompts", "GET", Some("127.0.0.1".parse().unwrap()), None, &file),
        Verdict::Allow(Grant::Loopback)
    ));
}

#[test]
fn loopback_is_recognised_including_ipv4_mapped() {
    assert!(is_loopback(Some("127.0.0.1".parse().unwrap())));
    assert!(is_loopback(Some("::1".parse().unwrap())));
    assert!(is_loopback(Some("::ffff:127.0.0.1".parse().unwrap())));
    assert!(!is_loopback(Some("10.44.33.71".parse().unwrap())));
    // ⚠ 拿不到来源地址 = **不是**本机（保守的那一侧）
    assert!(!is_loopback(None));
}

#[test]
fn credentials_endpoint_is_never_anonymous() {
    // 纪律 ④：与 enabled 无关 —— 关着闸门也照样要"已配对 + 已授权"
    let off = AuthFile {
        enabled: false,
        token: Some("global-token".into()),
        devices: vec![
            DeviceRecord {
                id: "phone-1".into(),
                token: "granted".into(),
                allow_credentials: true,
                ..Default::default()
            },
            DeviceRecord {
                id: "phone-2".into(),
                token: "not-granted".into(),
                allow_credentials: false,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let remote = Some("10.0.0.9".parse().unwrap());
    // 匿名 → 403（**不是**放行）
    match decide(CREDENTIALS_PATH, "GET", remote, None, &off) {
        Verdict::Forbidden(_) => {}
        other => panic!("匿名请求必须被拒，得到 {other:?}"),
    }
    // 全局令牌也不行：那是"配对管理"的钥匙，不是"读密钥"的授权
    match decide(CREDENTIALS_PATH, "GET", remote, Some("global-token"), &off) {
        Verdict::Forbidden(_) => {}
        other => panic!("全局令牌不该能读密钥，得到 {other:?}"),
    }
    // 配对了但没勾"允许密钥" → 403
    match decide(CREDENTIALS_PATH, "GET", remote, Some("not-granted"), &off) {
        Verdict::Forbidden(_) => {}
        other => panic!("未授权设备不该能读密钥，得到 {other:?}"),
    }
    // 勾了 → 放行
    assert!(matches!(
        decide(CREDENTIALS_PATH, "GET", remote, Some("granted"), &off),
        Verdict::Allow(Grant::Device { allow_credentials: true, .. })
    ));
    // 本机回环 → 放行（桌面 App 自己要用）
    assert!(matches!(
        decide(CREDENTIALS_PATH, "GET", Some("127.0.0.1".parse().unwrap()), None, &off),
        Verdict::Allow(Grant::Loopback)
    ));

    // ★ 读与写是**两个开关**（§14.1）：phone-1 只被允许**读**，
    //   于是它的 PUT 必须被拒 —— 合成一个开关就会在这里放行，
    //   而放行的后果是"这台手机能改桌面上的 key"。
    match decide(CREDENTIALS_PATH, "PUT", remote, Some("granted"), &off) {
        Verdict::Forbidden(msg) => assert!(msg.contains("修改密钥"), "措辞要指对方向：{msg}"),
        other => panic!("只被允许读的设备不许写，得到 {other:?}"),
    }
}

/// 反过来：只勾了"允许改"的设备 **不许读**（两个开关谁都不许顺手带上另一个）。
#[test]
fn the_two_credential_switches_are_independent() {
    let file = AuthFile {
        enabled: false,
        devices: vec![DeviceRecord {
            id: "phone-1".into(),
            token: "upload-only".into(),
            allow_credentials: false,
            allow_credentials_upload: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    let remote = Some("10.0.0.9".parse().unwrap());
    assert!(matches!(
        decide(CREDENTIALS_PATH, "PUT", remote, Some("upload-only"), &file),
        Verdict::Allow(Grant::Device { allow_credentials_upload: true, .. })
    ));
    match decide(CREDENTIALS_PATH, "GET", remote, Some("upload-only"), &file) {
        Verdict::Forbidden(msg) => assert!(msg.contains("读取密钥"), "措辞要指对方向：{msg}"),
        other => panic!("只被允许写的设备不许读，得到 {other:?}"),
    }
    // 旧库里的设备没有这个字段 → 反序列化后必须是 false（默认关），不是"跟着读那个开关走"
    let legacy: AuthFile = serde_json::from_str(
        r#"{"schema":"/2","enabled":true,"devices":[{"id":"d","token":"t","allowCredentials":true}]}"#,
    )
    .unwrap();
    assert!(legacy.devices[0].allow_credentials);
    assert!(!legacy.devices[0].allow_credentials_upload);
}

#[test]
fn auth_file_tolerates_missing_keys() {
    // 旧的 `/1` 形状（没有 devices）与手工改过的 JSON 都要能读
    let old: AuthFile = serde_json::from_str(r#"{"schema":"comfyhub-inbound-auth/1","enabled":true}"#).unwrap();
    assert!(old.devices.is_empty());
    assert_eq!(old.token, None);
    let partial: AuthFile = serde_json::from_str(r#"{"enabled":true,"devices":[{"id":"p","token":"t"}]}"#).unwrap();
    assert_eq!(partial.devices.len(), 1);
    assert!(!partial.devices[0].allow_credentials);
    assert_eq!(partial.schema, SCHEMA);
}

#[cfg(windows)]
#[test]
fn dpapi_round_trip_works() {
    // 证明我们这套 FFI（空熵 + CurrentUser）真的能加解密 ——
    // 它是"钥匙下行"能不能用的唯一技术前提
    let plain = b"sk-test-not-a-real-key";
    let sealed = dpapi::protect(plain).expect("DPAPI 加密失败");
    assert_ne!(sealed, plain);
    assert_eq!(dpapi::unprotect(&sealed).expect("DPAPI 解密失败"), plain);
    // 密文按 Kotlin 的形状落盘：base64 文本
    let text = base64_with(B64STD, &sealed, true);
    assert_eq!(dpapi::unprotect(&base64_decode(&text).unwrap()).unwrap(), plain);
}

#[cfg(windows)]
#[test]
fn resolve_credential_reads_the_managed_store_env_first() {
    // 托管文件那份是 **Kotlin 写的密文**（标准 base64 + DPAPI/CurrentUser/空熵）。
    // 这条用例把"Rust 能不能解开 Kotlin 写的东西"钉死 —— 自己写的 FFI
    // 参数（熵 / scope）错一位，表现就是"下行的密钥全是乱码或直接失败"。
    let tmp = crate::testpaths::scratch("auth-cred");
    let media = tmp.join("media");
    let ai = tmp.join("ai");
    std::fs::create_dir_all(&media).unwrap();
    std::fs::create_dir_all(&ai).unwrap();
    let plain = "sk-from-managed-store";
    let sealed = dpapi::protect(plain.as_bytes()).unwrap();
    let stored = serde_json::json!({ "fixture.ref": base64_with(B64STD, &sealed, true) });
    std::fs::write(
        ai.join("credentials.dpapi.json"),
        serde_json::to_string_pretty(&stored).unwrap(),
    )
    .unwrap();

    let state = AppState::new(crate::db::open_memory().unwrap(), ":memory:", media.clone());
    assert_eq!(resolve_credential(&state, "fixture.ref").unwrap(), plain);

    // 环境变量优先（与 Kotlin `CredentialService.resolve` 同顺序）
    std::env::set_var("fixture.ref", "from-env");
    assert_eq!(resolve_credential(&state, "fixture.ref").unwrap(), "from-env");
    std::env::remove_var("fixture.ref");

    // 桌面上没有的 ref → 404 形状的人话（不是 500、也不是空串）
    let error = resolve_credential(&state, "nope").unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);
    assert!(error.human().contains("nope"));

    let _ = std::fs::remove_dir_all(&tmp);
}

//! `endpoint_guard` 的单元测试（主体代码在 [`endpoint_guard`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

/// 红线：**任何**信任档都不放行（这条错了就是 SSRF）。
#[test]
fn red_lines_are_blocked_in_every_trust_level() {
    for host in ["169.254.169.254", "metadata.google.internal", "metadata.goog", "0.0.0.0", "::"] {
        assert_eq!(scope_of(host), Scope::Blocked, "{host} 必须是红线");
        for trust in ["public", "loopback", "private-network", "unsafe-any"] {
            let err = assert_allowed(host, trust).unwrap_err();
            assert_eq!(err, "该地址属于云元数据/保留地址，已拒绝", "{host} / {trust}");
        }
    }
}

/// 四档 × 各类地址的**完整真值表**（少一格就是"某一档悄悄放宽了"）。
#[test]
fn the_trust_table_is_exactly_kotlins() {
    let cases = [
        ("127.0.0.1", Scope::Loopback),
        ("localhost", Scope::Loopback),
        ("api.localhost", Scope::Loopback),
        ("[::1]", Scope::Loopback),
        ("10.1.2.3", Scope::Private),
        ("172.16.0.1", Scope::Private),
        ("172.32.0.1", Scope::Public), // 172.32 不在 172.16/12 里
        ("192.168.1.5", Scope::Private),
        ("fe80::1", Scope::Blocked),
        ("fec0::1", Scope::Private),
        ("fc00::1", Scope::Public), // ULA**不是** Java 的 site-local（照抄，别"现代化"）
        ("8.8.8.8", Scope::Public),
        ("api.openai.com", Scope::Public),
    ];
    for (host, expected) in cases {
        assert_eq!(scope_of(host), expected, "{host}");
    }

    let allowed = |host: &str, trust: &str| assert_allowed(host, trust).is_ok();
    // public：只放行公网（https 那条在 normalizeBaseUrl 里管，这里只看地址归属）
    assert!(allowed("8.8.8.8", "public"));
    assert!(!allowed("127.0.0.1", "public"));
    assert!(!allowed("192.168.1.5", "public"));
    assert!(!allowed("fe80::1", "public"));
    // loopback：只放行回环
    assert!(allowed("127.0.0.1", "loopback"));
    assert!(allowed("localhost", "loopback"));
    assert!(!allowed("192.168.1.5", "loopback"));
    assert!(!allowed("8.8.8.8", "loopback"));
    // private-network：内网 + 回环（链路本地那一档被红线吃掉了）
    assert!(allowed("192.168.1.5", "private-network"));
    assert!(allowed("10.0.0.1", "private-network"));
    assert!(allowed("127.0.0.1", "private-network"));
    assert!(!allowed("8.8.8.8", "private-network"));
    assert!(!allowed("fe80::1", "private-network"));
    // unsafe-any：普通地址全放行，红线仍然拦住（上面那条用例）
    assert!(allowed("8.8.8.8", "unsafe-any"));
    assert!(allowed("192.168.1.5", "unsafe-any"));
    // 认不出来的档位：一个字都不放行
    assert!(!allowed("8.8.8.8", "whatever"));
}

/// 推断（界面留空 `endpointTrust` 时用它）：与上面那张表同源。
#[test]
fn inferred_trust_matches_the_scope() {
    assert_eq!(infer_trust("127.0.0.1"), "loopback");
    assert_eq!(infer_trust("192.168.1.5"), "private-network");
    assert_eq!(infer_trust("8.8.8.8"), "public");
    assert_eq!(infer_trust("api.openai.com"), "public");
    // ⚠ 链路本地地址**推不出 private-network**：它在上一步就被判成 BLOCKED（红线），
    // 于是落到 `else -> public`。Kotlin 的 infer 里那一支 `Scope.LINK_LOCAL` 是**死代码**
    // （`AddressScope.of` 永远不会产出它）—— 这里照抄，不"顺手让它有意义"。
    // 结果一样是拒绝：`assert_allowed(fe80::1, "public")` 撞红线。
    assert_eq!(infer_trust("fe80::1"), "public");
    assert!(assert_allowed("fe80::1", infer_trust("fe80::1")).is_err());
}

/// 失败文案要**带当前判定**（用户排错全靠那半句）。
#[test]
fn the_error_message_names_the_scope() {
    let err = assert_allowed("127.0.0.1", "public").unwrap_err();
    assert_eq!(err, "端点信任级别 public 不允许访问主机 127.0.0.1（当前判定：LOOPBACK）");
}

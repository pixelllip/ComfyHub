//! **端点信任闸门**（SSRF 防护，AIH-017）—— `AddressScope` + `EndpointGuard` 的逐条对照。
//!
//! 它是"用户填了一个 Base URL"与"我们真的往那儿发请求"之间唯一的一道判定：
//! 保存时就拦（不等请求发出），这样"填了一个内网地址"当场看到人话，
//! 而不是几分钟后一条来历不明的 `PROVIDER_UNREACHABLE`。
//!
//! # 四个信任档（AIH-017：默认最严，放宽必须由用户显式选择）
//!
//! | 档 | 允许的地址 |
//! | --- | --- |
//! | `public` | 只有公网（并且必须是 **https**） |
//! | `loopback` | 只有本机回环 |
//! | `private-network` | 内网 + 回环 + **链路本地** |
//! | `unsafe-any` | 除"红线地址"外都放行 |
//!
//! ⚠ **红线一条都不能松**（`BLOCKED`）：云元数据地址（`169.254.169.254`、
//! `metadata.google.internal`）与 `0.0.0.0` 这种"任意地址"**在任何档位下都拒绝**——
//! `unsafe-any` 也只放行"能解析出来的普通地址"。这是这个模块存在的全部理由：
//! 一个用户可控的 URL 直通出网客户端，等于把云元数据端点交给调用方。
//!
//! # 与 Java 判定的一致性（为什么不用 crate）
//!
//! Kotlin 侧用的是 `InetAddress.isLoopbackAddress / isSiteLocalAddress / isLinkLocalAddress`，
//! 语义很细（尤其 IPv6 的 `fec0::/10` vs `fc00::/7`）。这里**逐条照它写**，
//! 但只对**字面量地址**判定：域名一律当 `PUBLIC`（Kotlin 同样留给请求时做 DNS 复核，
//! 见 `EndpointGuard.resolveAndCheck`）。判定纯函数、无 I/O，所以每一档都能单测。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// 地址归属（`enum Scope`，名字逐字对齐 —— 它会出现在给用户看的那句话里）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Loopback,
    Private,
    LinkLocal,
    Public,
    Blocked,
}

impl Scope {
    /// 给用户看的名字（Kotlin 是把枚举名直接插进文案的，所以是**大写**）。
    pub fn wire(self) -> &'static str {
        match self {
            Scope::Loopback => "LOOPBACK",
            Scope::Private => "PRIVATE",
            Scope::LinkLocal => "LINK_LOCAL",
            Scope::Public => "PUBLIC",
            Scope::Blocked => "BLOCKED",
        }
    }
}

/// 云元数据地址：**无论信任级别都不允许**。
const BLOCKED_HOSTS: [&str; 2] = ["metadata.google.internal", "metadata.goog"];

/// 主机 → 归属（`AddressScope.of`）。
///
/// 域名（解析不出字面量地址）一律 `PUBLIC` —— 与 Kotlin 同：真正的 DNS 复核发生在请求时。
pub fn scope_of(host: &str) -> Scope {
    let mut text = host.trim().to_lowercase();
    // Java 的 `removeSurrounding("[", "]")`：`[::1]` 这种带方括号的 IPv6
    if text.starts_with('[') && text.ends_with(']') && text.len() > 1 {
        text = text[1..text.len() - 1].to_string();
    }
    if text.is_empty() {
        return Scope::Blocked;
    }
    if BLOCKED_HOSTS.contains(&text.as_str()) {
        return Scope::Blocked;
    }
    if text == "localhost" || text.ends_with(".localhost") {
        return Scope::Loopback;
    }
    let Ok(addr) = text.parse::<IpAddr>() else {
        // 不是字面量地址（域名）→ 公网，留给请求时做 DNS 复核
        return Scope::Public;
    };
    match addr {
        IpAddr::V4(v4) => scope_of_v4(v4),
        IpAddr::V6(v6) => scope_of_v6(v6),
    }
}

fn scope_of_v4(addr: Ipv4Addr) -> Scope {
    if addr.is_loopback() {
        return Scope::Loopback; // 127.0.0.0/8
    }
    if addr.is_unspecified() {
        return Scope::Blocked; // 0.0.0.0 = Java isAnyLocalAddress
    }
    if addr.is_link_local() {
        return Scope::Blocked; // 169.254.0.0/16（含云元数据）
    }
    if addr.is_private() {
        return Scope::Private; // 10/8、172.16/12、192.168/16
    }
    Scope::Public
}

fn scope_of_v6(addr: Ipv6Addr) -> Scope {
    if addr.is_loopback() {
        return Scope::Loopback; // ::1
    }
    if addr.is_unspecified() {
        return Scope::Blocked; // ::
    }
    let segments = addr.segments();
    // 链路本地 fe80::/10（含 fe80::..febf:ffff）
    if segments[0] & 0xffc0 == 0xfe80 {
        return Scope::Blocked;
    }
    // 站点本地 fec0::/10（Java `Inet6Address.isSiteLocalAddress` 认的就是这一段，
    // **不是** fc00::/7 的 ULA —— 照它写，别"顺手现代化"）
    if segments[0] & 0xffc0 == 0xfec0 {
        return Scope::Private;
    }
    Scope::Public
}

/// 信任档（`EndpointTrust.parse`）。
pub fn parse_trust(wire: &str) -> Option<&'static str> {
    match wire.trim() {
        "public" => Some("public"),
        "loopback" => Some("loopback"),
        "private-network" => Some("private-network"),
        "unsafe-any" => Some("unsafe-any"),
        _ => None,
    }
}

/// 自动推断（`EndpointTrust.infer`）：回环 → loopback、内网/链路本地 → private-network、其余 → public。
pub fn infer_trust(host: &str) -> &'static str {
    match scope_of(host) {
        Scope::Loopback => "loopback",
        Scope::Private | Scope::LinkLocal => "private-network",
        _ => "public",
    }
}

/// 按信任级别校验主机（`EndpointGuard.assertAllowed`）。
///
/// 失败返回**人话**（不带错误码）：调用方统一包成 `ApiError::ai("CONFIG_ERROR", …)`，
/// 与 Kotlin 的 `AiException(CONFIG_ERROR, …)` 同形。
pub fn assert_allowed(host: &str, trust: &str) -> Result<(), String> {
    let scope = scope_of(host);
    // 云元数据 / 保留地址是硬红线，任何信任级别（含 unsafe-any）都不放行
    if scope == Scope::Blocked {
        return Err("该地址属于云元数据/保留地址，已拒绝".to_string());
    }
    let allowed = match trust {
        "unsafe-any" => true,
        "public" => scope == Scope::Public,
        "loopback" => scope == Scope::Loopback,
        // ⚠ 内网档**也放行链路本地**（与 Kotlin 同）——但链路本地在上面已经是 BLOCKED，
        // 所以这里写出来只是为了让这张表与 Kotlin 一字不差
        "private-network" => matches!(scope, Scope::Private | Scope::Loopback | Scope::LinkLocal),
        // 认不出来的档位：一个字都不放行（宁可拦住，也不要静默当成最宽的那档）
        _ => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "端点信任级别 {trust} 不允许访问主机 {host}（当前判定：{}）",
            scope.wire()
        ))
    }
}

#[cfg(test)]
mod tests {
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
        assert!(assert_allowed("fe80::1", &infer_trust("fe80::1")).is_err());
    }

    /// 失败文案要**带当前判定**（用户排错全靠那半句）。
    #[test]
    fn the_error_message_names_the_scope() {
        let err = assert_allowed("127.0.0.1", "public").unwrap_err();
        assert_eq!(err, "端点信任级别 public 不允许访问主机 127.0.0.1（当前判定：LOOPBACK）");
    }
}

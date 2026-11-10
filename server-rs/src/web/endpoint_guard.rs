//! **端点信任闸门**（SSRF 防护，AIH-017）—— `AddressScope` + `EndpointGuard` 的逐条对照。
//!
//! 它是"填写了一个 Base URL"与"实际向该地址发请求"之间唯一的判定：
//! 保存时就拦（不等请求发出），这样"填了一个内网地址"当场看到人话，
//! 而不是几分钟后一条来历不明的 `PROVIDER_UNREACHABLE`。
//!
//! # 四个信任档（AIH-017：默认最严，放宽必须显式选择）
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
//! 一个外部可控的 URL 直通出网客户端，等于把云元数据端点交给调用方。
//!
//! # 与 Java 判定的一致性（为什么不用 crate）
//!
//! Kotlin 侧用的是 `InetAddress.isLoopbackAddress / isSiteLocalAddress / isLinkLocalAddress`，
//! 语义很细（尤其 IPv6 的 `fec0::/10` vs `fc00::/7`）。这里**逐条照它写**，
//! 但只对**字面量地址**判定：域名一律当 `PUBLIC`（Kotlin 同样留给请求时做 DNS 复核，
//! 见 `EndpointGuard.resolveAndCheck`）。判定纯函数、无 I/O，所以每一档都能单测。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// 地址归属（`enum Scope`，名称逐字对齐 —— 它会出现在界面显示的说明中）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Loopback,
    Private,
    LinkLocal,
    Public,
    Blocked,
}

impl Scope {
    /// 界面显示的名称（Kotlin 将枚举名直接插入文案，因此为**大写**）。
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
mod tests;

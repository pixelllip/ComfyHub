//! 出网走不走代理（P4 第 29 步）—— 逐条对照 Kotlin `OutboundProxy`（AGENTS §4）。
//!
//! # 为什么必须自己写这一层
//!
//! 本机 `civitai.com` / `api.deepseek.com` 这类目标**直连不通**，只有 `127.0.0.1:7890`
//! 那个代理通；而 **`reqwest` 默认不看 Windows 的系统代理**。反过来，
//! 一旦把代理无差别套上去，`127.0.0.1:8188`（本机 ComfyUI）与局域网里的模型网关
//! **当场失联** —— Java 那边实测过这一条（`OutboundProxy` 的类注释里写着现场）。
//!
//! 所以规则是**两条一起**：
//! ① 公网地址按本机设置走代理；② **本机 / 内网地址永远直连**。
//!
//! # 优先级（与 Kotlin 逐条一致）
//!
//! 1. `COMFYHUB_HTTPS_PROXY` → `HTTPS_PROXY` / `https_proxy` / `HTTP_PROXY` / `http_proxy`
//!    （写成 `direct` / `none` / `off` / `-` / `no` = **明确要求直连**，用来盖掉系统代理）；
//! 2. 系统代理（Windows 的 WinINET 设置：`HKCU\…\Internet Settings` 的
//!    `ProxyEnable` / `ProxyServer` / `ProxyOverride`）；
//! 3. 都没有 → 直连。
//!
//! ⚠ **第 2 档在 Rust 侧是"尽力而为"**：读注册表要过 `reg query` 子进程（不引 Win32 绑定）。
//! 读不到就当"没配"（直连）—— 那与"代理没配"在行为上一样，而**不会**把内网也塞给代理。
//! 这与 Kotlin 的差别是：Kotlin 那边靠 `-Djava.net.useSystemProxies=true` 拿系统代理，
//! Rust 这边拿不到时**不会**报错，只是回到直连（那时用户看到的错误是
//! "连不上"而不是"走错了路"，且错误信息里会写明本次是直连）。

use std::process::Command;

/// 本项目的显式优先级最高的代理变量。
pub const ENV_PROXY: &str = "COMFYHUB_HTTPS_PROXY";
/// 通用的几个写法（大小写都认）。
pub const ENV_FALLBACKS: [&str; 4] = ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"];
/// 明确要求"别走代理"的写法。
const DIRECT_WORDS: [&str; 5] = ["direct", "none", "off", "-", "no"];
/// 一看就是本机 / 内网的名字后缀。
const LOCAL_SUFFIXES: [&str; 7] = [
    ".local",
    ".lan",
    ".internal",
    ".intranet",
    ".home",
    ".home.arpa",
    ".localdomain",
];

/// 这次出网会怎么走（**只用来写进错误信息**）。
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// 直连
    Direct,
    /// 走这个 HTTP 代理（`host:port`）
    Proxy(String),
}

impl Route {
    /// 给人看的短句（进错误信息 —— 用户看不到我们的日志，但看得到工具报错）。
    pub fn describe(&self) -> String {
        match self {
            Route::Direct => "直连（DIRECT）".to_string(),
            Route::Proxy(addr) => format!("HTTP 代理 {addr}"),
        }
    }
}

/// 这个主机名该不该**永不代理**。
///
/// 判据刻意宽松（**宁可直连**）：字面量 loopback / 私有网段 / 链路本地 / ULA，
/// 以及 `localhost`、`*.local`、`*.lan`、`*.internal` 这些一看就是本机内网的名字。
/// **绝不为它做 DNS**：只有"看起来就是 IP 字面量"的才去解析。
///
/// ⚠ 解析不出来的**按本机处理**：代理本来就只该用来解决"公网连不上"，
/// 把不确定的东西交给代理反而会把本机调用打死。
pub fn is_local_host(host: &str) -> bool {
    let h = host
        .trim()
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase();
    if h.is_empty() || h == "localhost" || h.ends_with(".localhost") {
        return true;
    }
    if LOCAL_SUFFIXES.iter().any(|s| h.ends_with(s)) {
        return true;
    }
    if !looks_like_ip_literal(&h) {
        return false;
    }
    match h.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
        }
        Ok(std::net::IpAddr::V6(v6)) => {
            v6.is_loopback()
                || v6.is_unspecified()
                // fc00::/7（IPv6 的"内网"）
                || (v6.segments()[0] & 0xfe00) == 0xfc00
        }
        Err(_) => true,
    }
}

fn looks_like_ip_literal(h: &str) -> bool {
    h.contains(':') || h.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// 解析一个代理写法：`http://host:port`、`host:port`、`host`。
///
/// 只造 **HTTP** 代理（与 Kotlin 同：Clash 这类混合端口的系统代理本来就以 HTTP 形式报出来）。
/// 解析不出主机名 → `None`（当"没配"处理）。
pub fn parse_proxy(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() || DIRECT_WORDS.contains(&value.to_ascii_lowercase().as_str()) {
        return None;
    }
    let without_scheme = value
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(value);
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    // 去掉 user:pass@（本机代理一般没有，但别人的写法要认）
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = authority.trim();
    if host.is_empty() {
        return None;
    }
    Some(host.to_string())
}

/// 按优先级读出配置：显式变量 → 通用变量 → 系统代理。返回值 `None` = 直连。
pub fn configured_proxy(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    let explicit = env(ENV_PROXY).filter(|v| !v.trim().is_empty());
    if let Some(raw) = explicit {
        return parse_proxy(&raw);
    }
    for name in ENV_FALLBACKS {
        if let Some(raw) = env(name).filter(|v| !v.trim().is_empty()) {
            return parse_proxy(&raw);
        }
    }
    system_proxy()
}

/// 这次请求会怎么走（**纯函数 + 注入的 env/系统代理**，所以每一档都能单测）。
pub fn route_for(
    host: &str,
    env: impl Fn(&str) -> Option<String>,
    system: impl Fn() -> Option<String>,
) -> Route {
    if is_local_host(host) {
        return Route::Direct;
    }
    let explicit = env(ENV_PROXY).filter(|v| !v.trim().is_empty());
    if let Some(raw) = explicit {
        return parse_proxy(&raw).map(Route::Proxy).unwrap_or(Route::Direct);
    }
    for name in ENV_FALLBACKS {
        if let Some(raw) = env(name).filter(|v| !v.trim().is_empty()) {
            return parse_proxy(&raw).map(Route::Proxy).unwrap_or(Route::Direct);
        }
    }
    system()
        .and_then(|raw| parse_proxy(&raw))
        .map(Route::Proxy)
        .unwrap_or(Route::Direct)
}

/// 进程真正用的那一档。
pub fn route(host: &str) -> Route {
    route_for(host, |name| std::env::var(name).ok(), system_proxy)
}

/// 系统代理（Windows：`HKCU\…\Internet Settings`）。
///
/// 非 Windows / 读不到 → `None`。**不会**因此报错：那等于"没配代理"。
#[cfg(windows)]
fn system_proxy() -> Option<String> {
    let output = Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            "/v",
            "ProxyEnable",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    // `ProxyEnable    REG_DWORD    0x1`
    let enabled = text
        .lines()
        .find(|line| line.contains("ProxyEnable"))
        .and_then(|line| line.split_whitespace().last())
        .map(|value| {
            value
                .trim_start_matches("0x")
                .trim_start_matches('0')
                .eq("1")
                || value == "0x1"
        })
        .unwrap_or(false);
    if !enabled {
        return None;
    }
    let output = Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            "/v",
            "ProxyServer",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let value = text
        .lines()
        .find(|line| line.contains("ProxyServer"))
        .and_then(|line| line.split("REG_SZ").nth(1))
        .map(str::trim)
        .filter(|v| !v.is_empty())?;
    // 有的写法是 `http=host:port;https=host:port`，取 https 那一档
    if let Some(https) = value
        .split(';')
        .find_map(|part| part.trim().strip_prefix("https="))
    {
        return Some(https.to_string());
    }
    Some(value.to_string())
}

#[cfg(not(windows))]
fn system_proxy() -> Option<String> {
    None
}

#[cfg(test)]
mod tests;

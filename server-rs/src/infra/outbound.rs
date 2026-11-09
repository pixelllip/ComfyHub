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
//!    `ProxyEnable` / `ProxyServer` / `ProxyOverride` / `AutoConfigURL`）；
//! 3. 都没有 → 直连。
//!
//! # 系统代理这一档的四条纪律
//!
//! * **一次 `reg query` 读整个键**：早先分两次查 `ProxyEnable` / `ProxyServer`，
//!   每次请求都要多起一个子进程；而且 `ProxyOverride` 根本没读 —— 系统里写着的
//!   "这些地址别走代理"被当成了耳旁风。
//! * **`ProxyOverride` 必须照办**（含 `<local>`）：它是"本机 / 内网直连"这条纪律的
//!   用户可编辑版本，不认它就会把用户自己指定的域名塞给代理。
//! * **不带点的名字按内网处理**：`nas` / `comfy` / 单标签主机名在 Windows 的
//!   `<local>` 里就是"本机网络"，公网域名一定带点。
//! * **PAC（`AutoConfigURL`）读不了就如实说**：它是一段 JS，Rust 侧没解释器。
//!   那种机器上现在是"直连"，**不是**"配了代理却不用"—— `status()` 会把这句话
//!   原样带给界面（`/api/system/outbound`），别让它变成又一个"猜"。
//!
//! ⚠ 读注册表要过 `reg` 子进程（不引 Win32 绑定）。读不到就当"没配"（直连）——
//! 那与"代理没配"在行为上一样，而**不会**把内网也塞给代理。

use std::process::Command;
use std::time::Duration;

use serde::Serialize;

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

/// WinINET 的设置键（`HKCU`：当前用户，后端由 App 拉起，跟着用户走）。
#[cfg(windows)]
const INTERNET_SETTINGS_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";

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

/// 系统代理的一份快照（WinINET 那几个值）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemProxy {
    /// `ProxyEnable = 1`
    pub enabled: bool,
    /// `ProxyServer` 原样（可能是 `host:port`，也可能是 `http=…;https=…`）
    pub server: Option<String>,
    /// `ProxyOverride` 拆开的列表（已小写；可能含 `<local>` 与 `127.*` 这类通配）
    pub overrides: Vec<String>,
    /// `AutoConfigURL`（PAC 脚本地址）。**我们不会去求值**，见模块头。
    pub auto_config: Option<String>,
}

impl SystemProxy {
    /// 系统代理真正要用的那个地址（`None` = 系统没开代理 / 读不出主机名）。
    ///
    /// 支持 `http=host:port;https=host:port` 的写法：优先 `https=`（我们出的都是 HTTPS），
    /// 只有 `http=` 时也认。⚠ 只认 HTTP 代理：`socks=` 那种我们造不出来，
    /// 宁可当"没配"（直连、错误信息里写直连）也不要造一个坏代理出来。
    pub fn proxy_server(&self) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let raw = self.server.as_deref()?;
        let per_scheme = raw.split(';').any(|part| part.trim().contains('='));
        for scheme in ["https", "http"] {
            let prefix = format!("{scheme}=");
            if let Some(addr) = raw
                .split(';')
                .find_map(|part| part.trim().strip_prefix(prefix.as_str()))
            {
                return parse_proxy(addr);
            }
        }
        if per_scheme {
            return None;
        }
        parse_proxy(raw)
    }

    /// 这个主机是否被 `ProxyOverride` 点名"别走代理"。
    ///
    /// 认三种写法：`<local>`（不带点的名字）、通配（`127.*` / `*.example.com`）、精确匹配。
    pub fn bypasses(&self, host: &str) -> bool {
        let h = normalize_host(host);
        self.overrides.iter().any(|pattern| {
            let p = pattern.trim();
            if p.is_empty() {
                return false;
            }
            if p == "<local>" {
                return !h.contains('.');
            }
            wildcard_matches(p, &h)
        })
    }
}

/// 主机名的比较口径：去方括号、去空白、小写。
fn normalize_host(host: &str) -> String {
    host.trim()
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase()
}

/// 这个主机名该不该**永不代理**。
///
/// 判据刻意宽松（**宁可直连**）：字面量 loopback / 私有网段 / 链路本地 / ULA，
/// `localhost`、`*.local`、`*.lan`、`*.internal` 这些一看就是本机内网的名字，
/// 以及**不带点的单标签名字**（`nas` / `comfy` —— Windows 的 `<local>` 就是这个口径；
/// 公网域名一定带点）。
/// **绝不为它做 DNS**：只有"看起来就是 IP 字面量"的才去解析。
///
/// ⚠ 解析不出来的**按本机处理**：代理本来就只该用来解决"公网连不上"，
/// 把不确定的东西交给代理反而会把本机调用打死。
pub fn is_local_host(host: &str) -> bool {
    let h = normalize_host(host);
    if h.is_empty() || h == "localhost" || h.ends_with(".localhost") {
        return true;
    }
    if LOCAL_SUFFIXES.iter().any(|s| h.ends_with(s)) {
        return true;
    }
    if !looks_like_ip_literal(&h) {
        // 不带点 = 单标签主机名（`nas` / `comfy` / 老式 NetBIOS 名），按内网直连。
        return !h.contains('.');
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

/// `ProxyOverride` 里的一条通配（`*` 可出现在任意位置）是否命中这个主机。
///
/// 覆盖 WinINET 实际会用到的形状：`127.*`、`*.example.com`、`<local>`（在外面单独判）、
/// 精确主机名。不是通配表就只做精确比较 —— **宁可多走代理，也不乱认**。
fn wildcard_matches(pattern: &str, host: &str) -> bool {
    let pattern = pattern.trim().to_ascii_lowercase();
    if pattern.is_empty() {
        return false;
    }
    if !pattern.contains('*') {
        return host == pattern;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let head = parts[0];
    if !host.starts_with(head) {
        return false;
    }
    let mut rest = &host[head.len()..];
    for (index, part) in parts.iter().enumerate().skip(1) {
        if part.is_empty() {
            continue;
        }
        if index == parts.len() - 1 {
            return rest.len() >= part.len() && rest.ends_with(part);
        }
        match rest.find(part) {
            Some(pos) => rest = &rest[pos + part.len()..],
            None => return false,
        }
    }
    true
}

/// 按优先级读出配置：显式变量 → 通用变量 → 系统代理。返回值 `None` = 直连。
pub fn configured_proxy(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    if let Some(raw) = env(ENV_PROXY).filter(|v| !v.trim().is_empty()) {
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
    system: impl Fn() -> Option<SystemProxy>,
) -> Route {
    if is_local_host(host) {
        return Route::Direct;
    }
    if let Some(raw) = env(ENV_PROXY).filter(|v| !v.trim().is_empty()) {
        return parse_proxy(&raw).map(Route::Proxy).unwrap_or(Route::Direct);
    }
    for name in ENV_FALLBACKS {
        if let Some(raw) = env(name).filter(|v| !v.trim().is_empty()) {
            return parse_proxy(&raw).map(Route::Proxy).unwrap_or(Route::Direct);
        }
    }
    let Some(system) = system() else {
        return Route::Direct;
    };
    // 系统代理的例外表（`ProxyOverride`）只对它自己那一档生效：
    // 用户显式设了 `COMFYHUB_HTTPS_PROXY` 就照他的来。
    if system.bypasses(host) {
        return Route::Direct;
    }
    system
        .proxy_server()
        .map(Route::Proxy)
        .unwrap_or(Route::Direct)
}

/// 进程真正用的那一档。
pub fn route(host: &str) -> Route {
    route_for(host, |name| std::env::var(name).ok(), system_proxy_settings)
}

/// 按目标主机造一个 HTTP 客户端（**诊断探针**用；工具各自还有自己的错误码与超时口径）。
pub fn http_client(
    host: &str,
    connect_timeout: Duration,
    timeout: Duration,
) -> reqwest::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none());
    match route(host) {
        Route::Proxy(addr) => {
            builder = builder.proxy(reqwest::Proxy::all(format!("http://{addr}"))?);
        }
        Route::Direct => {
            // 环境变量里的 `HTTPS_PROXY` reqwest **是看的** —— 显式覆盖掉，
            // 否则本机/内网目标会被它接走。
            builder = builder.no_proxy();
        }
    }
    builder.build()
}

/// 当前出网配置的**人话版**（`GET /api/system/outbound`）—— 界面上要能看见"吃不吃代理"。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OutboundStatus {
    /// `proxy` = 公网走代理；`direct` = 公网直连
    pub route: String,
    /// 代理地址（`host:port`）；直连时是 null
    pub proxy: Option<String>,
    /// 这份配置是从哪来的：`env:COMFYHUB_HTTPS_PROXY` / `env:HTTPS_PROXY` / `system` / `none`
    pub source: String,
    pub system: SystemStatus,
    /// 读得到但用不上的情况（目前只有 PAC）
    pub warning: Option<String>,
}

/// 系统代理那几项（照原样给界面，方便用户对着系统设置核对）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatus {
    pub enabled: bool,
    pub server: Option<String>,
    pub overrides: Vec<String>,
    pub auto_config_url: Option<String>,
}

impl Default for SystemStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            server: None,
            overrides: Vec::new(),
            auto_config_url: None,
        }
    }
}

/// PAC 那句提醒：**读不了就明说**，别让用户以为"配了代理却不用"。
const PAC_WARNING: &str = "系统的联网设置用的是自动配置脚本（PAC），这个脚本本项目读不了，\
所以现在公网是直连。要让请求走代理，请在系统设置里改成手动填代理地址，\
或者设环境变量 COMFYHUB_HTTPS_PROXY（例如 http://127.0.0.1:7890）。";

/// 进程真正用的那一份（探针 / 界面读它）。
pub fn status() -> OutboundStatus {
    status_for(|name| std::env::var(name).ok(), system_proxy_settings)
}

/// 纯函数版（注入 env 与系统设置），所以每一档都能单测。
pub fn status_for(
    env: impl Fn(&str) -> Option<String>,
    system: impl Fn() -> Option<SystemProxy>,
) -> OutboundStatus {
    let sys = system().unwrap_or_default();
    let (proxy, source) = match env(ENV_PROXY).filter(|v| !v.trim().is_empty()) {
        Some(raw) => (parse_proxy(&raw), format!("env:{ENV_PROXY}")),
        None => {
            let mut hit = None;
            for name in ENV_FALLBACKS {
                if let Some(raw) = env(name).filter(|v| !v.trim().is_empty()) {
                    hit = Some((parse_proxy(&raw), format!("env:{name}")));
                    break;
                }
            }
            match hit {
                Some(pair) => pair,
                None => match sys.proxy_server() {
                    Some(addr) => (Some(addr), "system".to_string()),
                    None => (None, "none".to_string()),
                },
            }
        }
    };
    // PAC 只在"它本来该提供代理、我们却用不上"的时候才提醒：
    // 用户已经用环境变量指定了代理（或显式 direct），就不必再唠叨。
    let warning = if source == "none" && sys.auto_config.is_some() {
        Some(PAC_WARNING.to_string())
    } else {
        None
    };
    OutboundStatus {
        route: if proxy.is_some() { "proxy" } else { "direct" }.to_string(),
        proxy,
        source,
        warning,
        system: SystemStatus {
            enabled: sys.enabled,
            server: sys.server,
            overrides: sys.overrides,
            auto_config_url: sys.auto_config,
        },
    }
}

/// 系统代理（Windows：`HKCU\…\Internet Settings`）。
///
/// 非 Windows / 读不到 → `None`。**不会**因此报错：那等于"没配代理"。
#[cfg(windows)]
fn system_proxy_settings() -> Option<SystemProxy> {
    let output = Command::new("reg")
        .args(["query", INTERNET_SETTINGS_KEY])
        .output()
        .ok()?;
    if !output.status.success() {
        // 键不存在 = 这台机器没配过系统代理，按"没配"处理。
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Some(parse_reg_values(&text))
}

#[cfg(not(windows))]
fn system_proxy_settings() -> Option<SystemProxy> {
    None
}

/// 本机系统代理的地址（`None` = 没开 / 读不到）。
#[cfg(windows)]
fn system_proxy() -> Option<String> {
    system_proxy_settings()?.proxy_server()
}

#[cfg(not(windows))]
fn system_proxy() -> Option<String> {
    None
}

/// 把 `reg query` 的输出拆成"值名 → 值"。
///
/// 一行长这样：`    ProxyServer    REG_SZ    127.0.0.1:7890`。
/// 值里可能有空格（`ProxyOverride` 一般没有，但别人的写法要认），所以按
/// **类型标记**切，而不是按空白切。
fn parse_reg_values(text: &str) -> SystemProxy {
    let mut values = std::collections::HashMap::<String, String>::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("HKEY_") {
            continue;
        }
        let Some(type_at) = line.find("REG_") else {
            continue;
        };
        let (name, rest) = line.split_at(type_at);
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let mut parts = rest.splitn(2, char::is_whitespace);
        let _kind = parts.next();
        let value = parts.next().unwrap_or("").trim().to_string();
        values.insert(name.to_string(), value);
    }
    let non_empty = |key: &str| {
        values
            .get(key)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    SystemProxy {
        enabled: values
            .get("ProxyEnable")
            .map(|v| dword_is_on(v))
            .unwrap_or(false),
        server: non_empty("ProxyServer"),
        overrides: values
            .get("ProxyOverride")
            .map(|v| parse_overrides(v))
            .unwrap_or_default(),
        auto_config: non_empty("AutoConfigURL"),
    }
}

/// `REG_DWORD` 的"开"判断（`0x1` / `1` 都算；`0x0` / `0` 都不算）。
///
/// ⚠ 老实现是 `trim_start_matches("0x").trim_start_matches('0') == "1"` ——
/// 对 `0x1` 碰巧成立，但把"按十六进制解析"写成了字符串修剪，`0x10` 之类会判错。
fn dword_is_on(raw: &str) -> bool {
    let t = raw.trim();
    let parsed = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => t.parse::<u32>(),
    };
    parsed.map(|v| v != 0).unwrap_or(false)
}

/// `ProxyOverride` 是分号分隔的一串；统一小写，方便比较。
fn parse_overrides(raw: &str) -> Vec<String> {
    raw.split(';')
        .map(|part| part.trim().to_ascii_lowercase())
        .filter(|part| !part.is_empty())
        .collect()
}

#[cfg(test)]
mod tests;

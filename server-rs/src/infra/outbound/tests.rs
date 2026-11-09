//! `outbound` 的单元测试（主体代码在 [`outbound`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn no_env(_: &str) -> Option<String> {
    None
}

/// 造一份"只开了手动代理"的系统设置（用例用，别碰真注册表）。
fn sys_proxy(addr: &str) -> SystemProxy {
    SystemProxy {
        enabled: true,
        server: Some(addr.to_string()),
        overrides: Vec::new(),
        auto_config: None,
    }
}

/// 本机 / 内网**永远直连** —— 这条是"打开系统代理会打死本机 ComfyUI"的解药。
#[test]
fn local_and_private_hosts_are_never_proxied() {
    for host in [
        "127.0.0.1",
        "localhost",
        "api.localhost",
        "192.168.1.10",
        "10.0.0.5",
        "172.16.3.4",
        "169.254.1.1",
        "0.0.0.0",
        "::1",
        "fd00::1",
        "box.local",
        "nas.lan",
        "git.internal",
        "",
        // 单标签主机名 = Windows `<local>` 的口径：内网，直连。
        "nas",
        "comfy",
    ] {
        assert!(is_local_host(host), "{host} 应当直连");
        assert_eq!(
            route_for(
                host,
                |_| Some("127.0.0.1:7890".into()),
                || Some(sys_proxy("127.0.0.1:7890"))
            ),
            Route::Direct,
            "{host} 即使配了代理也必须直连"
        );
    }
    // 公网
    for host in ["api.deepseek.com", "civitai.com", "8.8.8.8", "1.1.1.1"] {
        assert!(!is_local_host(host), "{host} 是公网");
    }
    // ⚠ 172.32 不在私有段（172.16~172.31）—— 判错了会把公网目标按本机直连
    assert!(!is_local_host("172.32.0.1"));
}

/// 优先级：显式变量 > 通用变量 > 系统代理；`direct` 那一档盖掉系统代理。
#[test]
fn proxy_priority_and_direct_words() {
    let host = "api.deepseek.com";
    assert_eq!(
        route_for(
            host,
            |n| (n == ENV_PROXY).then(|| "127.0.0.1:7890".into()),
            || Some(sys_proxy("9.9.9.9:1"))
        ),
        Route::Proxy("127.0.0.1:7890".into())
    );
    assert_eq!(
        route_for(
            host,
            |n| (n == "HTTPS_PROXY").then(|| "http://127.0.0.1:7890/".into()),
            || None
        ),
        Route::Proxy("127.0.0.1:7890".into()),
        "带 scheme 与尾斜杠的写法也要认"
    );
    // 显式 direct 盖掉系统代理
    assert_eq!(
        route_for(
            host,
            |n| (n == ENV_PROXY).then(|| "direct".into()),
            || Some(sys_proxy("9.9.9.9:1"))
        ),
        Route::Direct
    );
    // 都没配 → 系统代理
    assert_eq!(
        route_for(host, no_env, || Some(sys_proxy("127.0.0.1:7890"))),
        Route::Proxy("127.0.0.1:7890".into())
    );
    // 都没有 → 直连
    assert_eq!(route_for(host, no_env, || None), Route::Direct);
    // 读不出主机名的写法按"没配"处理（回落系统代理，而不是变成一个坏代理）
    assert_eq!(parse_proxy("http://"), None);
    assert_eq!(parse_proxy("   "), None);
    assert_eq!(parse_proxy("none"), None);
    assert_eq!(
        parse_proxy("http://user:pass@10.0.0.1:8080"),
        Some("10.0.0.1:8080".into())
    );
}

/// `ProxyOverride` 是"用户自己写的直连名单"，必须照办（含 `<local>` 与通配）。
#[test]
fn system_proxy_override_list_is_honoured() {
    let system = SystemProxy {
        enabled: true,
        server: Some("127.0.0.1:7890".into()),
        overrides: parse_overrides("localhost;127.*;192.168.*;*.example.com;<local>"),
        auto_config: None,
    };
    // 命中例外 → 直连
    for host in [
        "127.0.0.1",
        "127.1.2.3",
        "192.168.5.9",
        "nas",
        "a.example.com",
    ] {
        assert!(system.bypasses(host), "{host} 在例外表里");
        assert_eq!(
            route_for(host, no_env, || Some(system.clone())),
            Route::Direct,
            "{host} 在例外表里，即使它长着公网的样子也不该走代理"
        );
    }
    // 没命中 → 走代理
    for host in ["civitai.com", "example.com", "danbooru.donmai.us"] {
        assert!(!system.bypasses(host), "{host} 不在例外表里");
        assert_eq!(
            route_for(host, no_env, || Some(system.clone())),
            Route::Proxy("127.0.0.1:7890".into()),
            "{host} 应当走系统代理"
        );
    }
    // 显式环境变量不该被系统例外表盖掉（那是用户另外下的指令）
    assert_eq!(
        route_for(
            "a.example.com",
            |n| (n == ENV_PROXY).then(|| "10.0.0.1:8888".into()),
            || Some(system.clone())
        ),
        Route::Proxy("10.0.0.1:8888".into())
    );
}

/// 通配写法：`*` 在开头 / 结尾 / 中间都要认；不是通配就精确比较。
#[test]
fn proxy_override_wildcards() {
    assert!(wildcard_matches("127.*", "127.0.0.1"));
    assert!(wildcard_matches("192.168.*", "192.168.1.1"));
    assert!(!wildcard_matches("192.168.*", "192.169.1.1"));
    assert!(wildcard_matches("*.example.com", "a.example.com"));
    assert!(!wildcard_matches("*.example.com", "example.com"));
    assert!(wildcard_matches("*", "anything"));
    assert!(wildcard_matches("civitai.com", "civitai.com"));
    assert!(!wildcard_matches("civitai.com", "www.civitai.com"));
    assert!(!wildcard_matches("", "civitai.com"));
}

/// `reg query` 的输出解析：一行一个值，值里允许有空格；`REG_DWORD` 按数字判。
#[test]
fn reg_query_output_is_parsed() {
    let text = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\r\n    \
        ProxyEnable    REG_DWORD    0x1\r\n    \
        ProxyServer    REG_SZ    127.0.0.1:7890\r\n    \
        ProxyOverride    REG_SZ    localhost;127.*;192.168.*;<local>\r\n    \
        AutoConfigURL    REG_SZ    http://example.com/proxy.pac\r\n";
    let sys = parse_reg_values(text);
    assert!(sys.enabled);
    assert_eq!(sys.server.as_deref(), Some("127.0.0.1:7890"));
    assert_eq!(
        sys.overrides,
        vec!["localhost", "127.*", "192.168.*", "<local>"]
    );
    assert_eq!(
        sys.auto_config.as_deref(),
        Some("http://example.com/proxy.pac")
    );
    assert_eq!(sys.proxy_server().as_deref(), Some("127.0.0.1:7890"));

    // 关掉的那一档：`ProxyEnable=0` 时即便有 ProxyServer 也不该走代理
    let off = parse_reg_values(
        "    ProxyEnable    REG_DWORD    0x0\r\n    ProxyServer    REG_SZ    127.0.0.1:7890\r\n",
    );
    assert!(!off.enabled);
    assert_eq!(off.proxy_server(), None);
    // 分方案的写法：取 https 那一档
    let per_scheme = parse_reg_values(
        "    ProxyEnable    REG_DWORD    0x1\r\n    ProxyServer    REG_SZ    http=10.0.0.1:8080;https=10.0.0.2:8443\r\n",
    );
    assert_eq!(per_scheme.proxy_server().as_deref(), Some("10.0.0.2:8443"));
    // 只写了 socks= 的那种：我们造不出 SOCKS 客户端，宁可当"没配"也别造一个坏代理
    let socks = parse_reg_values(
        "    ProxyEnable    REG_DWORD    0x1\r\n    ProxyServer    REG_SZ    socks=127.0.0.1:1080\r\n",
    );
    assert_eq!(socks.proxy_server(), None);
    // DWORD 的"开"按数值判，不做字符串修剪
    assert!(dword_is_on("0x1"));
    assert!(dword_is_on("1"));
    assert!(!dword_is_on("0x0"));
    assert!(!dword_is_on("0"));
    assert!(dword_is_on("0x10"));
}

/// `status()` 的四个来源各给一句话，界面照这句显示（别让用户靠猜）。
#[test]
fn outbound_status_reports_its_source() {
    let system = sys_proxy("127.0.0.1:7890");
    let status = status_for(no_env, || Some(system.clone()));
    assert_eq!(status.route, "proxy");
    assert_eq!(status.proxy.as_deref(), Some("127.0.0.1:7890"));
    assert_eq!(status.source, "system");
    assert!(status.warning.is_none());

    let env_status = status_for(
        |n| (n == ENV_PROXY).then(|| "10.0.0.1:8888".into()),
        || Some(system.clone()),
    );
    assert_eq!(env_status.source, format!("env:{ENV_PROXY}"));
    assert_eq!(env_status.proxy.as_deref(), Some("10.0.0.1:8888"));

    let fallback = status_for(
        |n| (n == "http_proxy").then(|| "http://10.0.0.2:3128".into()),
        || None,
    );
    assert_eq!(fallback.source, "env:http_proxy");
    assert_eq!(fallback.proxy.as_deref(), Some("10.0.0.2:3128"));

    // 明确 direct 的那一档：有来源、但没代理
    let forced = status_for(
        |n| (n == ENV_PROXY).then(|| "direct".into()),
        || Some(sys_proxy("1.1.1.1:1")),
    );
    assert_eq!(forced.route, "direct");
    assert_eq!(forced.source, format!("env:{ENV_PROXY}"));
    assert_eq!(forced.proxy, None);

    // 什么都没有
    let none = status_for(no_env, || None);
    assert_eq!(none.route, "direct");
    assert_eq!(none.source, "none");
    assert!(none.warning.is_none());
}

/// PAC（`AutoConfigURL`）读不了：必须**如实说出来**，别装成"我用了代理"。
#[test]
fn pac_is_reported_not_silently_ignored() {
    let pac = SystemProxy {
        enabled: false,
        server: None,
        overrides: Vec::new(),
        auto_config: Some("http://wpad/wpad.dat".into()),
    };
    let status = status_for(no_env, || Some(pac.clone()));
    assert_eq!(status.route, "direct");
    assert!(status.warning.as_deref().unwrap_or("").contains("PAC"));
    // 但用户自己用环境变量指定了代理时，就不该再唠叨 PAC
    let explicit = status_for(
        |n| (n == ENV_PROXY).then(|| "127.0.0.1:7890".into()),
        || Some(pac.clone()),
    );
    assert!(explicit.warning.is_none());
}

/// `Route::describe()` 是**给用户看的那句话**（错误信息里要能判断"是代理的事还是对方的事"）。
#[test]
fn route_description_is_human_readable() {
    assert_eq!(Route::Direct.describe(), "直连（DIRECT）");
    assert_eq!(
        Route::Proxy("127.0.0.1:7890".into()).describe(),
        "HTTP 代理 127.0.0.1:7890"
    );
}

/// 本机的真实配置：`COMFYHUB_HTTPS_PROXY` 没有时按系统代理走（本机系统代理是 7890）。
///
/// ⚠ 这条**不断言具体值**：CI / 别的机器上代理可能是另一个地址或压根没有。
/// 它只钉"公网主机拿到的是一个 Route（不是 panic）"与"状态能拼出人话"这两个不变量。
#[test]
fn real_environment_never_panics() {
    let public = route("api.deepseek.com");
    assert!(!public.describe().is_empty());
    assert_eq!(route("127.0.0.1"), Route::Direct);
    let status = status();
    assert!(status.route == "proxy" || status.route == "direct");
    assert!(!status.source.is_empty());
}

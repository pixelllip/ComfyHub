//! `outbound` 的单元测试（主体代码在 [`outbound`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn no_env(_: &str) -> Option<String> {
    None
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
    ] {
        assert!(is_local_host(host), "{host} 应当直连");
        assert_eq!(
            route_for(
                host,
                |_| Some("127.0.0.1:7890".into()),
                || Some("127.0.0.1:7890".into())
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
            || Some("9.9.9.9:1".into())
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
            || Some("9.9.9.9:1".into())
        ),
        Route::Direct
    );
    // 都没配 → 系统代理
    assert_eq!(
        route_for(host, no_env, || Some("127.0.0.1:7890".into())),
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
/// 它只钉"公网主机拿到的是一个 Route（不是 panic）"这个不变量。
#[test]
fn real_environment_never_panics() {
    let public = route("api.deepseek.com");
    assert!(!public.describe().is_empty());
    assert_eq!(route("127.0.0.1"), Route::Direct);
}

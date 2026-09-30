//! `net_urls` 的单元测试（主体代码在 [`net_urls`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn v4(s: &str) -> IpAddr {
    IpAddr::V4(s.parse().unwrap())
}

#[test]
fn 三档分类按地址形状_不按网卡名() {
    assert_eq!(classify(&v4("127.0.0.1")), "loopback");
    assert_eq!(classify(&v4("10.0.2.2")), "lan");
    assert_eq!(classify(&v4("172.16.5.5")), "lan");
    assert_eq!(classify(&v4("172.32.5.5")), "other");
    assert_eq!(classify(&v4("192.168.1.9")), "lan");
    // CGNAT 边界：100.64.0.0/10 = 100.64.x ~ 100.127.x
    assert_eq!(classify(&v4("100.64.0.5")), "tailscale");
    assert_eq!(classify(&v4("100.127.255.1")), "tailscale");
    assert_eq!(classify(&v4("100.63.0.1")), "other");
    assert_eq!(classify(&v4("100.128.0.1")), "other");
    assert_eq!(classify(&v4("8.8.8.8")), "other");
    assert_eq!(classify(&"::1".parse().unwrap()), "loopback");
}

#[test]
fn 跳过未指定与链路本地_不编地址() {
    let eps = assemble(
        8081,
        &[
            v4("0.0.0.0"),
            v4("169.254.10.1"),
            v4("192.168.1.9"),
            "fe80::1".parse().unwrap(),
        ],
    );
    let urls: Vec<&str> = eps.iter().map(|e| e.url.as_str()).collect();
    assert_eq!(urls, vec!["http://192.168.1.9:8081"]);
    assert_eq!(eps[0].kind, "lan");
}

#[test]
fn 顺序稳定_回环与_tailscale_在前_且按_url_去重() {
    let eps = assemble(
        8081,
        &[
            v4("192.168.1.9"),
            v4("100.64.0.5"),
            v4("127.0.0.1"),
            v4("127.0.0.1"),
            v4("10.1.2.3"),
        ],
    );
    let urls: Vec<&str> = eps.iter().map(|e| e.url.as_str()).collect();
    assert_eq!(
        urls,
        vec![
            "http://127.0.0.1:8081",
            "http://100.64.0.5:8081",
            "http://192.168.1.9:8081",
            "http://10.1.2.3:8081",
        ]
    );
}

#[test]
fn 端口为_0_时回空_宁可什么都不说也不报一个连不上的地址() {
    assert!(assemble(0, &[v4("127.0.0.1"), v4("192.168.1.9")]).is_empty());
    let js = urls_json(0);
    assert_eq!(js["urls"].as_array().unwrap().len(), 0);
    assert_eq!(js["port"], 0);
}

#[test]
fn ipv6_拼_url_要带方括号() {
    let eps = assemble(8081, &["::1".parse().unwrap()]);
    assert_eq!(eps[0].url, "http://[::1]:8081");
    assert_eq!(eps[0].kind, "loopback");
}

//! `GET /api/net/urls` —— 这台桌面上有哪些**可达地址**（给「同步与连接」页当候选）。
//!
//! # 为什么由后端出这份列表
//!
//! 手机没法自己扫网段猜桌面的地址（那是攻击行为，而且多数网络里扫不出来）。
//! 而**本机已知**自身有哪些网卡 —— 实际使用中会在三条路径之间切换：
//! 回环（同机 / 模拟器）、局域网（同一 Wi-Fi）、Tailscale（异地组网）。
//! 把这三类如实列出来，手机只需要"点一下"。
//!
//! # 三条纪律
//!
//! 1. **只报事实**：网卡上是什么地址就报什么，**不按网卡名猜**（`vEthernet` / `docker0`
//!    之类一律照实收下 —— 它们有时真的能用，有时不能用，而"能不能用"由客户端打一次
//!    真实请求决定，不由我们替它判断）；
//! 2. **端口必须是"我们实际监听的那个"**（`--port 0` 时由系统挑），不是请求里的 Host：
//!    手机拿它去拼候选地址，写错了就是"点了连不上"；
//! 3. **一条都没有就回空数组**，绝不编一个地址出来（`port == 0` 时也回空：
//!    那种构造出来的 AppState 没有监听任何端口，报出去的地址必然连不上）。
//!
//! 分类**只用于显示**（客户端据它渲染「回环 / 局域网 / Tailscale」三个人话标签），
//! 不参与"连不连得上"的判断 —— 那条判断在 `lib/core/sync_endpoints.dart` 里由真实请求给。

use std::net::{IpAddr, Ipv4Addr};

use serde_json::{json, Value};

/// 一个候选地址。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// `loopback` / `lan` / `tailscale` / `other`（与 Dart 侧 `SyncEndpointKind` 同名同值）。
    pub kind: &'static str,
    pub url: String,
}

/// 按地址形状分类（**纯函数**，用例直接喂假地址，不依赖这台机器的网卡）。
pub fn classify(ip: &IpAddr) -> &'static str {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        // IPv6 只认回环：其他（link-local / 临时地址）无实际用途，归入 other
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                "loopback"
            } else {
                "other"
            }
        }
    }
}

fn classify_v4(ip: &Ipv4Addr) -> &'static str {
    let o = ip.octets();
    if ip.is_loopback() {
        return "loopback";
    }
    // Tailscale 走 CGNAT 100.64.0.0/10 —— 它看着像公网，其实是私网组网。
    // 在私网之前判断：不能归入 other（否则无法识别"该条为异地线路"）。
    if o[0] == 100 && (64..=127).contains(&o[1]) {
        return "tailscale";
    }
    if o[0] == 10 || (o[0] == 172 && (16..=31).contains(&o[1])) || (o[0] == 192 && o[1] == 168) {
        return "lan";
    }
    "other"
}

/// 该地址是否值得返回。
///
/// 跳过三类：**未指定**（`0.0.0.0` / `::`，客户端连不上）、**链路本地**（169.254/16、fe80::，
/// 那是"未获取 DHCP"的残留）、**多播**。其他如实收集（含公网地址 —— 自行使用 Tailscale
/// Funnel 或反代暴露出来时，那就是他想要的候选）。
fn usable(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !v4.is_unspecified() && !v4.is_multicast() && !v4.is_link_local(),
        IpAddr::V6(v6) => {
            !v6.is_unspecified() && !v6.is_multicast() && !(v6.segments()[0] & 0xffc0 == 0xfe80)
        }
    }
}

/// 排序权重：回环最前（本机最常用），然后 Tailscale（异地），再局域网，最后其他。
fn weight(kind: &str) -> u8 {
    match kind {
        "loopback" => 0,
        "tailscale" => 1,
        "lan" => 2,
        _ => 3,
    }
}

/// 把"我们监听的端口 + 一批地址"组装成候选列表（**纯函数**，用例喂假地址）。
///
/// 去重按 URL；顺序稳定（见 [`weight`]，同类内保持传入顺序）—— 界面上的候选顺序
/// 每次结果一致，选择的那一条不会变化。
pub fn assemble(port: u16, ips: &[IpAddr]) -> Vec<Endpoint> {
    if port == 0 {
        return Vec::new();
    }
    let mut out: Vec<Endpoint> = Vec::new();
    for ip in ips.iter().filter(|ip| usable(ip)) {
        let host = match ip {
            IpAddr::V4(v4) => v4.to_string(),
            // IPv6 拼 URL 必须带方括号，否则 `http://::1:8081` 解析不出来
            IpAddr::V6(v6) => format!("[{v6}]"),
        };
        let url = format!("http://{host}:{port}");
        if out.iter().any(|e| e.url == url) {
            continue;
        }
        out.push(Endpoint {
            kind: classify(ip),
            url,
        });
    }
    out.sort_by_key(|e| weight(e.kind));
    out
}

/// 现探本机网卡 + 回环，组装候选（[`assemble`] 的真实现）。
pub fn collect(port: u16) -> Vec<Endpoint> {
    let mut ips: Vec<IpAddr> = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];
    match if_addrs::get_if_addrs() {
        Ok(list) => {
            for itf in list {
                // 回环网卡已经在上面放过了（避免出现两条一模一样的回环）
                if itf.is_loopback() {
                    continue;
                }
                ips.push(itf.ip());
            }
        }
        Err(e) => {
            // 探不到网卡**不是错误**：至少还有回环那一条（同机 / 模拟器那条路仍然可用）。
            // 但要说出来 —— 静默只剩回环会让"局域网连不上"看起来像"桌面没配好"。
            eprintln!("[net-urls] 枚举网卡失败（只报回环）：{e}");
        }
    }
    assemble(port, &ips)
}

/// **配对二维码**里的候选地址：回环一律不要，局域网排最前。
///
/// 二维码是给**另一台设备**扫的。回环地址在手机上指向手机自己 —— 拿它去配对
/// 必然 `Connection refused`，而且报错完全看不出是地址问题（2026-10-31 真机现场）。
/// 所以这条不是"排序偏好"，是**正确性**：回环绝不能出现在二维码的候选里。
///
/// 局域网排在 Tailscale 之前：同一 Wi-Fi 是最常见的配对场景；Tailscale 使用者
/// 仍能拿到自己的地址，只是不在第一顺位。
pub fn pairing_candidates(mut endpoints: Vec<Endpoint>) -> Vec<Endpoint> {
    endpoints.retain(|e| e.kind != "loopback");
    endpoints.sort_by_key(|e| match e.kind {
        "lan" => 0,
        "tailscale" => 1,
        _ => 2,
    });
    endpoints
}

/// 现探网卡并只留配对可用的地址（[`pairing_candidates`] 的真实现）。
pub fn collect_for_pairing(port: u16) -> Vec<Endpoint> {
    pairing_candidates(collect(port))
}

/// 端点响应体。
pub fn urls_json(port: u16) -> Value {
    let urls: Vec<Value> = collect(port)
        .into_iter()
        .map(|e| json!({ "kind": e.kind, "url": e.url }))
        .collect();
    json!({ "port": port, "urls": urls })
}

#[cfg(test)]
mod tests;

//! 局域网桌面发现（UDP 广播，手机侧 `DiscoveryBridge` 对应）。
//!
//! # 为什么不是 mDNS
//!
//! mDNS 要额外引一个 crate（桌面）+ Android `NsdManager`（手机）；而我们要回答的
//! 问题只有一句：**同一个局域网里，ComfyHub 桌面现在听在哪个端口**。一个 100 字节
//! 的 UDP 广播就够了，且不依赖任何系统服务/额外依赖。
//!
//! # 边界
//!
//! * **只覆盖同一广播域**（同一 Wi-Fi / 网段）。换到另一个网络时，UDP 广播过不去；
//!   那种场景用 Tailscale / 固定主机名，本模块不假装能穿透。
//! * 广播内容**不是凭据**：只有服务名、版本、端口。手机仍要用配对令牌才能读数据。
//! * 只在 `COMFYHUB_ALLOW_REMOTE=1`（后端听 `0.0.0.0`）时启动；只听回环时广播
//!   一个手机连不上的地址没有意义。
//!
//! # 报文
//!
//! ```json
//! {"service":"comfyhub","v":1,"port":4038}
//! ```
//!
//! 手机用**收到报文的源地址**拼 URL（`http://<source>:<port>`），所以报文里不写 IP：
//! 多网卡时源地址天然就是"这一包是从哪张网卡发的"，比让桌面猜更准。

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::net::UdpSocket;

/// 手机监听的发现端口（与 Android `DiscoveryBridge.PORT` 同值，改一处要同时改另一处）。
pub const DISCOVERY_PORT: u16 = 4039;

/// 广播间隔：足够快（换网络后几秒内被发现），又不会把 Wi-Fi 唤醒打满。
const INTERVAL: Duration = Duration::from_secs(3);

/// 广播报文（抽出来给单测，避免测试去碰真网卡）。
pub fn payload(port: u16) -> String {
    format!(r#"{{"service":"comfyhub","v":1,"port":{port}}}"#)
}

/// 该往哪些地址发：每张网卡的广播地址 + 全局广播兜底。
///
/// 全局广播（255.255.255.255）在部分 Windows 网卡上不会从 Wi-Fi 出去，所以
/// 优先用每张网卡自己的广播地址；拿不到时再补全局那一条。
pub fn targets() -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = Vec::new();
    if let Ok(list) = if_addrs::get_if_addrs() {
        for itf in list {
            if let if_addrs::IfAddr::V4(v4) = itf.addr {
                if v4.is_loopback() {
                    continue;
                }
                if let Some(broadcast) = v4.broadcast {
                    let addr = SocketAddr::from((broadcast, DISCOVERY_PORT));
                    if !out.contains(&addr) {
                        out.push(addr);
                    }
                }
            }
        }
    }
    let global = SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT));
    if !out.contains(&global) {
        out.push(global);
    }
    out
}

/// 起一个后台任务，周期性广播当前真实端口。
///
/// `port == 0`（测试构造的 AppState / 没监听）时**什么都不发** —— 宁可发现不到，
/// 也不要让手机拿到一个 `:0` 的地址。
pub fn spawn(port: u16) {
    if port == 0 {
        return;
    }
    tokio::spawn(async move {
        let socket = match UdpSocket::bind("0.0.0.0:0").await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[discovery] 绑定 UDP 失败（手机将无法自动发现）：{e}");
                return;
            }
        };
        if let Err(e) = socket.set_broadcast(true) {
            eprintln!("[discovery] 开广播失败（手机将无法自动发现）：{e}");
            return;
        }
        let message = payload(port);
        let mut ticker = tokio::time::interval(INTERVAL);
        loop {
            ticker.tick().await;
            for target in targets() {
                // 单张网卡发失败（VPN / 虚拟网卡没权限）不应终止其它网卡
                let _ = socket.send_to(message.as_bytes(), target).await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 报文只带服务名_版本_与真实端口() {
        let text = payload(4038);
        assert!(text.contains(r#""service":"comfyhub""#));
        assert!(text.contains(r#""v":1"#));
        assert!(text.contains(r#""port":4038"#));
        // 不把 IP 塞进去：手机要按 UDP 源地址拼 URL
        assert!(!text.contains("127.0.0.1"));
    }
}

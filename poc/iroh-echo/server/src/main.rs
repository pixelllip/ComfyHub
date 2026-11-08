//! iroh echo server —— 最小验证用，**不属于主工程**。
//!
//! 启动后打印三行可复制的东西：
//!   ENDPOINT_ID=...   手机端用它拨号（公钥，不随 IP 变）
//!   DIRECT=ip:port    直连候选（局域网 / IPv6）
//!   RELAY=...         中继地址（强制走中继时用）
//!
//! 协议：每条双向流发什么回什么，回完关流。

use std::time::Instant;

use iroh::{
    endpoint::{presets, Connection},
    protocol::{AcceptError, ProtocolHandler, Router},
    Endpoint,
};
use n0_error::StdResultExt;

const ALPN: &[u8] = b"comfyhub-poc/echo/0";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let endpoint = Endpoint::bind(presets::N0).await?;
    let router = Router::builder(endpoint).accept(ALPN, Echo).spawn();
    let ep = router.endpoint();
    ep.online().await;

    println!("ENDPOINT_ID={}", ep.id());
    let addr = ep.addr();
    for ip in addr.ip_addrs() {
        println!("DIRECT={ip}");
    }
    for relay in addr.relay_urls() {
        println!("RELAY={relay}");
    }
    for sa in ep.bound_sockets() {
        println!("BOUND={sa}");
    }
    println!("READY");

    tokio::signal::ctrl_c().await.ok();
    let _ = router.shutdown().await;
    Ok(())
}

#[derive(Debug, Clone)]
struct Echo;

impl ProtocolHandler for Echo {
    async fn accept(
        &self,
        connection: Connection,
    ) -> std::result::Result<(), AcceptError> {
        let remote = connection.remote_id();
        println!("accepted {remote}");
        loop {
            let (mut send, mut recv) = match connection.accept_bi().await {
                Ok(s) => s,
                Err(_) => break,
            };
            let t0 = Instant::now();
            let data = match recv.read_to_end(64 * 1024).await {
                Ok(d) => d,
                Err(_) => break,
            };
            send.write_all(&data).await.anyerr()?;
            send.finish().anyerr()?;
            println!(
                "echo {} bytes to {remote} in {:?}",
                data.len(),
                t0.elapsed()
            );
        }
        connection.closed().await;
        Ok(())
    }
}

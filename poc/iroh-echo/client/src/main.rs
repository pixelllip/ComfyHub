//! iroh echo client —— 最小验证用，**不属于主工程**。
//!
//! 用法：
//!   client <ENDPOINT_ID> [ip:port ...] [relay:<url>] [--relay-only]
//!
//! 行为：连上后每秒发一条 ping 并打印 RTT；断线后每 2 秒重连。
//! `--relay-only` 会清掉所有 IP 传输，强制走中继（测 ③）。

use std::time::{Duration, Instant};

use anyhow::Result;
use iroh::{endpoint::presets, Endpoint, EndpointAddr, TransportAddr};

const ALPN: &[u8] = b"comfyhub-poc/echo/0";

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let id_text = args.next().unwrap_or_default();
    if id_text.is_empty() {
        eprintln!("usage: client <ENDPOINT_ID> [ip:port ...] [relay:<url>] [--relay-only|--direct-only]");
        std::process::exit(2);
    }
    let endpoint_id: iroh::EndpointId = id_text.parse()?;

    let mut relay_only = false;
    let mut direct_only = false;
    let mut transports = Vec::new();
    for a in args {
        if a == "--relay-only" {
            relay_only = true;
            continue;
        }
        if a == "--direct-only" {
            direct_only = true;
            continue;
        }
        if let Some(url) = a.strip_prefix("relay:") {
            transports.push(TransportAddr::Relay(url.parse()?));
        } else if let Ok(sa) = a.parse() {
            transports.push(TransportAddr::Ip(sa));
        } else {
            eprintln!("忽略看不懂的参数：{a}");
        }
    }
    let addr = EndpointAddr::from_parts(endpoint_id, transports);

    let mut builder = Endpoint::builder(presets::N0);
    if relay_only {
        // 清掉所有 IP 传输 → 只能经中继（测 ③ 的强制条件）
        builder = builder.clear_ip_transports();
        println!("MODE=relay-only");
    } else if direct_only {
        // 清掉所有中继传输 → 只能直连（判断"到底有没有直连路径"）
        builder = builder.clear_relay_transports();
        println!("MODE=direct-only");
    } else {
        println!("MODE=direct");
    }
    let endpoint = builder.bind().await?;
    if !direct_only {
        endpoint.online().await;
    }
    println!("CLIENT_ID={}", endpoint.id());

    let mut attempt = 0u64;
    loop {
        attempt += 1;
        println!("CONNECT attempt={attempt}");
        match endpoint.connect(addr.clone(), ALPN).await {
            Ok(conn) => {
                println!("CONNECTED attempt={attempt}");
                loop {
                    let (mut send, mut recv) = match conn.open_bi().await {
                        Ok(s) => s,
                        Err(e) => {
                            println!("OPEN_BI_ERR={e}");
                            break;
                        }
                    };
                    let msg = format!("ping {attempt} {}", now_ms());
                    let t0 = Instant::now();
                    if let Err(e) = send.write_all(msg.as_bytes()).await {
                        println!("WRITE_ERR={e}");
                        break;
                    }
                    let _ = send.finish();
                    match recv.read_to_end(1024).await {
                        Ok(data) => println!(
                            "RTT_MS={} ECHO={}",
                            t0.elapsed().as_millis(),
                            String::from_utf8_lossy(&data)
                        ),
                        Err(e) => {
                            println!("READ_ERR={e}");
                            break;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(1000)).await;
                }
                println!("DISCONNECTED attempt={attempt}");
            }
            Err(e) => println!("CONNECT_ERR={e}"),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

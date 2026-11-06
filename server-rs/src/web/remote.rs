//! 对端地址的**可选**读法（`ConnectInfo` 的兜底版）。
//!
//! `Option<ConnectInfo<_>>` 在 axum 0.8 里不成立（它要的是 `OptionalFromRequestParts`，
//! `ConnectInfo` 没实现）。但"只允许本机"这类端点又必须能面对"没有对端地址"的调用：
//! 集成测试走的是 `Router::oneshot`，根本没有 TCP 连接。所以这里自己读一次 extensions ——
//! 取不到就是 `None`，由调用方决定怎么处理。
//!
//! ⚠ `None` **不等于"本机"**：生产路径（`into_make_service_with_connect_info`）永远有值，
//! 只有测试 / 进程内调用才会没有。需要"只许本机"的端点要把 `None` 当成允许，
//! 否则测试与 `scripts\` 里的直连探针会全部 403 —— 但**拿到值时必须认真判**。

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use std::net::SocketAddr;

pub struct MaybeRemote(pub Option<SocketAddr>);

impl<S> FromRequestParts<S> for MaybeRemote
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(MaybeRemote(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0),
        ))
    }
}

/// 这个对端是不是本机；`None`（没有连接信息）按"允许"处理（见文件头）。
pub fn is_loopback(remote: &MaybeRemote) -> bool {
    match remote.0 {
        None => true,
        Some(addr) => crate::mobile::auth::is_loopback(Some(addr.ip())),
    }
}

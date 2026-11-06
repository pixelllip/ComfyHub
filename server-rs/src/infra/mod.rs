//! `infra/` —— 基础设施。
//!
//! SQLite 连接与事务纪律、沙箱自查（完整性标签）、出网代理、时间、测试暂存目录。

pub mod backup;
pub mod confinement;
pub mod db;
pub mod iso_time;
pub mod outbound;
pub mod testpaths;

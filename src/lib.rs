#![allow(clippy::type_complexity, clippy::result_large_err)]

pub mod bounce;
pub mod config;
pub mod db;
pub mod dkim_keys;
pub mod dns_records;
pub mod http;
pub mod queue;
pub mod routing;
pub mod smtp_in;
pub mod smtp_out;
pub mod warming;

pub use bounce::{classify_smtp, BounceClass};
pub use routing::{pick_ip, RouteError, SendingIp, Stream};
pub use warming::{
    evaluate_health, isp_concurrency, isp_hourly_cap, remaining_quota, warmup_daily_cap, Health,
    Isp, Reputation, Role,
};

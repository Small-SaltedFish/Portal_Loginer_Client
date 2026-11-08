//! Campus protocol migrated from Portal_Loginer 1.4.1 (Apache-2.0).
//! Platform services own credentials, network observations and persistence.
#[cfg(windows)]
mod bound_dns;
pub mod config;
pub mod crypto;
pub mod engine;
pub mod protocol;
pub mod transport;

pub const BASELINE_COMMIT: &str = "dcaffa81af551e3f259ddb1433a170aff8b840b6";

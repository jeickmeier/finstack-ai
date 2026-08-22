//! S3-compatible blob driver.

mod config;
mod request;
mod sigv4;
mod store;

pub use config::{Addressing, S3ObjectStoreConfig};
pub use store::S3ObjectStore;

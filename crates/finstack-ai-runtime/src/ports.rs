//! Target-correct object and future bounds shared by runtime ports.

use core::future::Future;
use core::pin::Pin;
use futures_core::Stream;

/// Object bound used by runtime ports on native targets.
#[cfg(not(target_arch = "wasm32"))]
pub trait PortObject: Send + Sync + 'static {}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + Sync + 'static> PortObject for T {}

/// Object bound used by runtime ports on browser-WASM targets.
#[cfg(target_arch = "wasm32")]
pub trait PortObject: 'static {}

#[cfg(target_arch = "wasm32")]
impl<T: 'static> PortObject for T {}

/// Boxed native runtime future.
#[cfg(not(target_arch = "wasm32"))]
pub type PortFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Boxed local browser-WASM runtime future.
#[cfg(target_arch = "wasm32")]
pub type PortFuture<T> = Pin<Box<dyn Future<Output = T> + 'static>>;

/// Boxed native runtime stream.
#[cfg(not(target_arch = "wasm32"))]
pub type PortStream<T> = Pin<Box<dyn Stream<Item = T> + Send + 'static>>;

/// Boxed local browser-WASM runtime stream.
#[cfg(target_arch = "wasm32")]
pub type PortStream<T> = Pin<Box<dyn Stream<Item = T> + 'static>>;

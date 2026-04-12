use std::sync::Arc;

use smol::lock::Mutex;

pub mod conn;
pub mod rpc;
pub mod state;

pub type ArcMu<T> = Arc<Mutex<T>>;

pub fn arcmu<T>(inner: T) -> ArcMu<T> {
    Arc::new(Mutex::new(inner))
}

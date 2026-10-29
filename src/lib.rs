pub mod adapters;
pub mod api;
pub mod builtin;
pub mod builtin_runtime;
pub mod cardigann;
pub mod cardigann_exec;
pub mod engine;
pub mod models;
pub mod protocol;
pub mod store;
pub mod torrent;
#[derive(Clone)]
pub struct AppState {
    pub engine: std::sync::Arc<engine::Engine>,
}

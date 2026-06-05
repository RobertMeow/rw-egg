pub mod app;
pub mod handlers;
pub mod middleware;
pub mod state;

pub use app::build_router;
pub use state::AppState;

// Re-export xray process functions for use in handlers
pub use remnanode_xray::process::{start_xray, stop_xray as xray_stop};
mod xray_process {
    pub use remnanode_xray::process::{start_xray, stop_xray};
}

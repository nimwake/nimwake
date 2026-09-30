//! Application lifecycle.

mod command;
mod effect;
mod handle;
mod lease;
mod manager;
mod spec;
mod state;
mod transition;

pub use command::StopReason;
pub use handle::AppHandle;
pub use lease::RequestLease;
pub use manager::{spawn, spawn_with_capacity, ACQUIRE_ENQUEUE_TIMEOUT, DEFAULT_MAILBOX_CAPACITY};
pub use spec::{AppSpec, LifecycleSpec, RuntimeSpec, ServerSpec, DEFAULT_MAX_PENDING_REQUESTS};
pub use state::{AppStateName, AppStatus};

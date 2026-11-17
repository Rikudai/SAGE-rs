//! sage-core: shared contracts of S.A.G.E. (config, events, model/tool/agent traits).
//!
//! Every other crate depends only on this one plus its own libraries, so the
//! crates can be built in parallel. Change a contract here first, then its users.

pub mod config;
pub mod events;
pub mod llm;
pub mod paths;
pub mod persona;
pub mod text;
pub mod tools;

pub use config::Config;
pub use events::{AppStatus, EventBus, SageEvent};
pub use llm::*;
pub use tools::{ToolResult, ToolSpec, Toolbox};

//! Shared, platform-independent protocol types for Desktop <-> Android Agent.

pub mod capability;
pub mod envelope;
pub mod error;
pub mod frame;
pub mod methods;

pub use capability::*;
pub use envelope::*;
pub use error::*;
pub use frame::*;
pub use methods::*;

/// Current Desktop-Agent protocol major version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Maximum JSON payload carried by one length-prefixed frame.
pub const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;

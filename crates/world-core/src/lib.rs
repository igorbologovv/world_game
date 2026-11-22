#![warn(missing_docs)]
//! Shared planetary data and optional Bevy client contracts.

pub mod coordinates;
pub mod planet;

mod heights;
mod terrain_detail;
pub use heights::{LocalPool, SurfaceHeights};

#[cfg(feature = "runtime")]
pub mod runtime;

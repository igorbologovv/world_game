#![warn(missing_docs)]
//! Shared planetary data and optional Bevy client contracts.

pub mod construction;
pub mod coordinates;
pub mod planet;

mod heights;
mod inland_water;
mod terrain_detail;
pub use heights::{LocalPool, SurfaceHeights};
pub use inland_water::{LakeTile, RiverReach, WaterKind, WaterSample};

#[cfg(feature = "runtime")]
pub mod canopy;
#[cfg(feature = "runtime")]
pub mod runtime;

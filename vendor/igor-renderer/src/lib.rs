//! Hybrid forest mesh construction and LOD selection from Igor renderer.
//!
//! The library accepts host geometry and alpha sampling. The host owns scene
//! streaming, GPU assets, transforms and render passes.

mod geometry;
mod selection;

pub use geometry::{build, LodVertex, MeshLod};
pub use selection::{rank, Detail, Selection};

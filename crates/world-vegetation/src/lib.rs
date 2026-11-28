#![warn(missing_docs)]
//! Deterministic vegetation placement, streaming, and tree and plant rendering.

mod forest;

pub use forest::canopy::bake_planet_canopy;
pub use forest::{
    CoconutPalm, FernSettings, ForestClearing, ForestRenderStats, ProceduralForestPlugin,
    VegetationSettings,
};

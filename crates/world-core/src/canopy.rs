//! Runtime canopy coverage published by vegetation and consumed by terrain.

use bevy::{prelude::*, render::render_resource::ShaderType};

/// Number of camera-centered coverage layers.
pub const CANOPY_CASCADES: usize = 6;
/// Width and height of each camera-centered coverage layer in texels.
pub const CANOPY_SIDE: u32 = 80;

/// Tangent-plane projection for one local canopy layer.
#[derive(Clone, Copy, Default, ShaderType)]
pub struct CanopyCascade {
    /// East tangent in XYZ and the half width in radians in W.
    pub east: Vec4,
    /// North tangent in XYZ and the texel width in meters in W.
    pub north: Vec4,
    /// Radial center in XYZ and the layer fade weight in W.
    pub center: Vec4,
}

/// Shader layout for the terrain canopy sampler.
#[derive(Clone, Copy, Default, ShaderType)]
pub struct CanopyLayout {
    /// Local layers ordered from smallest to largest footprint.
    pub cascades: [CanopyCascade; CANOPY_CASCADES],
    /// Start and end of the impostor fade in XY, global mip floor in Z and global opacity in W.
    pub fade: Vec4,
    /// Clearing direction in XYZ and its radius in meters in W.
    pub clearing: Vec4,
}

/// Canopy coverage atlas in planet coordinates. RGB stores premultiplied linear crown color
/// and alpha stores the fraction of the texel covered by crowns. Vegetation updates the local
/// layers and their transforms together.
#[derive(Resource)]
pub struct ForestCanopy {
    /// Filterable coverage array with a stable handle throughout the stream.
    pub atlas: Handle<Image>,
    /// Seamlessly filtered orbital coverage in a mipmapped hardware cubemap.
    pub globe: Handle<Image>,
    /// Shading texture for the finest projection. Red is visible crown fraction.
    /// The remaining channels store premultiplied crown height (0–40 m), tone and warmth.
    /// The shading mips use the same projection as the first coverage layer.
    pub crowns: Handle<Image>,
    /// Projections and handoff distances for the resident layers.
    pub layout: CanopyLayout,
}

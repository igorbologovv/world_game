//! Runtime canopy coverage published by vegetation and consumed by terrain.

use bevy::{prelude::*, render::render_resource::ShaderType};

/// Number of local coverage layers before the six global cube faces.
pub const CANOPY_CASCADES: usize = 6;
/// Width and height of each coverage layer in texels.
pub const CANOPY_SIDE: u32 = 80;

/// Tangent-plane projection for one local canopy layer.
#[derive(Clone, Copy, Default, ShaderType)]
pub struct CanopyCascade {
    /// East tangent in XYZ and the half width in radians in W.
    pub east: Vec4,
    /// North tangent in XYZ and the texel width in meters in W.
    pub north: Vec4,
    /// Radial center in XYZ. W is 1 for a resident layer and 0 otherwise.
    pub center: Vec4,
}

/// Shader layout for the terrain canopy sampler.
#[derive(Clone, Copy, Default, ShaderType)]
pub struct CanopyLayout {
    /// Local layers ordered from smallest to largest footprint.
    pub cascades: [CanopyCascade; CANOPY_CASCADES],
    /// Start and end of the distant impostor fade in XY, with global face residency bits in Z.
    pub fade: Vec4,
}

/// Canopy coverage atlas in planet coordinates. RGB stores premultiplied linear crown color
/// and alpha stores the fraction of the texel covered by crowns. The last six layers follow
/// the legacy cube-face order. Vegetation updates the layers and their transforms together.
#[derive(Resource)]
pub struct ForestCanopy {
    /// Filterable coverage array with a stable handle throughout the stream.
    pub atlas: Handle<Image>,
    /// Shading texture for the finest projection. Red is visible crown fraction.
    /// The remaining channels store premultiplied crown height (0–40 m), tone and warmth.
    /// The shading mips use the same projection as the first coverage layer.
    pub crowns: Handle<Image>,
    /// Projections and handoff distances for the resident layers.
    pub layout: CanopyLayout,
}

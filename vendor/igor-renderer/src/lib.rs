//! Igor_renderer's hardware hybrid forest, embedded on a caller-owned device.

mod gpu;
mod scene_canopy;
mod shadow;
mod tree_texture;
mod trees;

/// Math types used by the upstream pipeline.
pub use glam;
pub use gpu::{PipelineKey, RegionDraw, Renderer};

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use scene_canopy::{Canopy, Draw, Geometry, Template};
use std::{error::Error, ops::Range, path::Path};

/// Indexed source geometry for a procedural tree material.
pub struct MeshPart {
    /// Model-space positions in meters.
    pub positions: Vec<[f32; 3]>,
    /// One model-space normal per position.
    pub normals: Vec<[f32; 3]>,
    /// One repeating color texture coordinate per position.
    pub uvs: Vec<[f32; 2]>,
    /// Triangle-list indices into the vertex arrays.
    pub indices: Vec<u32>,
    /// The sRGB base color texture and linear alpha channel.
    pub color: image::RgbaImage,
    /// Linear color and alpha multiplier.
    pub factor: [f32; 4],
    /// Alpha cutoff for foliage. Opaque parts use `None`.
    pub alpha_cutoff: Option<f32>,
}

/// Builds the shared GLB atlas, mesh LODs, and static shadow templates.
pub struct Models {
    geometry: trees::TreeGeometry,
    draws: Vec<Range<u32>>,
    mid: Vec<Draw>,
    proxies: Geometry,
    templates: Vec<Template>,
    model_count: usize,
    vertices: Vec<trees::MeshVertex>,
    indices: Vec<u32>,
    shadow_templates: Vec<Vec<shadow::Point>>,
}

/// Handle to a model and the normalization applied by Igor's GLB loader.
#[derive(Clone, Copy)]
pub struct Model {
    index: usize,
    scale: f32,
    bounds: Bounds,
}

impl Models {
    /// Reserve the standalone per-model share of the mid-canopy geometry budget.
    pub fn new(model_count: usize) -> Self {
        Self {
            geometry: trees::TreeGeometry::default(),
            draws: Vec::new(),
            mid: Vec::new(),
            proxies: Geometry::default(),
            templates: Vec::new(),
            model_count,
            vertices: Vec::new(),
            indices: Vec::new(),
            shadow_templates: Vec::new(),
        }
    }

    /// Load the original indexed mesh, material factors, and textures from a GLB.
    pub fn load(&mut self, path: &Path) -> Result<Model, Box<dyn Error>> {
        let scale = self.geometry.load_model(path)?;
        self.finish_model(scale)
    }

    /// Import procedural geometry using the same mesh LOD and shadow builders.
    pub fn add(&mut self, parts: Vec<MeshPart>) -> Result<Model, Box<dyn Error>> {
        let scale = self.geometry.add_mesh(parts)?;
        self.finish_model(scale)
    }

    fn finish_model(&mut self, scale: f32) -> Result<Model, Box<dyn Error>> {
        let geometry = &mut self.geometry;
        let mesh = geometry.mesh_models.pop().expect("the pending model");
        let baked =
            scene_canopy::build_template(geometry, &mesh, self.model_count, &mut self.proxies)
                .map(|(mid, template)| (mid, template, shadow::template(geometry)));
        geometry.surfels.clear();
        geometry.patches.clear();

        let (mid, template, shadow) = baked?;
        let vertex_offset = u32::try_from(self.vertices.len())?;
        let start = u32::try_from(self.indices.len())?;
        let end = u32::try_from(self.indices.len() + mesh.indices.len())?;
        self.vertices.extend_from_slice(&mesh.vertices);
        self.indices
            .extend(mesh.indices.into_iter().map(|i| i + vertex_offset));
        let model = Model {
            index: self.draws.len(),
            scale,
            bounds: Bounds {
                lo: mesh.lo,
                hi: mesh.hi,
            },
        };
        self.draws.push(start..end);
        self.mid.push(mid);
        self.templates.push(template);
        self.shadow_templates.push(shadow);
        Ok(model)
    }
}

/// Canonical tree placement supplied by the host, in a stable local metric frame.
pub struct Tree {
    /// Template from the same model collection as the region.
    pub model: Model,
    /// Root of the source mesh in the region frame.
    pub root: Vec3,
    /// Rotation around the local up axis in radians.
    pub yaw: f32,
    /// Scale relative to the original source asset dimensions.
    pub scale: f32,
}

/// Igor wind, shadow, and coverage settings read from the environment.
#[derive(Clone, Copy)]
pub struct Effects {
    flags: u32,
    wind_strength: f32,
    shadow_strength: f32,
    coverage: f32,
    grain: f32,
}

impl Default for Effects {
    fn default() -> Self {
        let wind = env("IGOR_WIND", 1u32) != 0;
        let shadows = env("IGOR_SHADOWS", 1u32) != 0;
        Self {
            flags: ((wind as u32) << 4) | ((shadows as u32) << 5),
            wind_strength: env("IGOR_WIND_STRENGTH", 1.0f32).clamp(0.0, 3.0),
            shadow_strength: env("IGOR_SHADOW_STRENGTH", 0.85f32).clamp(0.0, 1.0),
            coverage: env("IGOR_CANOPY_COVERAGE", 1.0f32).clamp(0.0, 2.0),
            grain: env("IGOR_CANOPY_GRAIN", 1.0f32).clamp(0.25, 4.0),
        }
    }
}

/// Immutable scene data for a resident streaming region.
pub struct Region {
    placed: Vec<Placed>,
    models: Vec<usize>,
    shadow: shadow::ShadowField,
    canopy: Canopy,
    mid: Vec<Draw>,
    effects: Effects,
    bounds: Bounds,
}

impl Region {
    /// Build the far canopy cells and bake canopy shadows along the region-local direction to the sun.
    pub fn new(
        trees: &[Tree],
        models: &Models,
        effects: Effects,
        sun_direction: Vec3,
    ) -> Option<Self> {
        if trees.is_empty() {
            return None;
        }
        let mut ids: Vec<_> = trees.iter().map(|tree| tree.model.index).collect();
        ids.sort_unstable();
        ids.dedup();
        let mut placed = Vec::with_capacity(trees.len());
        let mut bounds = Bounds::empty();
        for tree in trees {
            // Invert only Igor's normalization of the source asset so world-client
            // retains its authored tree scale and roots.
            let scale = tree.scale / tree.model.scale;
            let (s, c) = tree.yaw.sin_cos();
            let mut instance = Instance {
                translation: Vec3::ZERO.extend(scale).to_array(),
                rotation: [c, s, 0.0, 1.0],
            };
            instance.translation = (tree.root - instance.world(Vec3::new(8.0, 0.0, 8.0)))
                .extend(scale)
                .to_array();
            let mut tree_bounds = Bounds::empty();
            for point in tree.model.bounds.corners() {
                tree_bounds.add(instance.world(point));
            }
            bounds.merge(tree_bounds);
            placed.push(Placed {
                instance,
                model: tree.model.index,
                bounds: tree_bounds,
            });
        }
        let shadow = shadow::field(&placed, &models.shadow_templates, bounds, sun_direction);
        let canopy = Canopy::build(&placed, &models.templates, models.proxies.bytes())
            .expect("bounded streamed canopy geometry");
        for tree in &mut placed {
            tree.model = ids.binary_search(&tree.model).unwrap();
        }
        let mid = ids.iter().map(|&id| models.mid[id]).collect();
        Some(Self {
            placed,
            models: ids,
            shadow,
            canopy,
            mid,
            effects,
            bounds,
        })
    }

    /// Test whether the region intersects the view and its streaming distance.
    pub fn visible(&self, view: Camera, range: f32) -> bool {
        let view = View::new(view);
        let padding = Vec3::splat(self.canopy.padding);
        let bounds = Bounds {
            lo: self.bounds.lo - padding,
            hi: self.bounds.hi + padding,
        };
        view.origin
            .distance_squared(view.origin.clamp(bounds.lo, bounds.hi))
            <= range * range
            && view.visible(bounds)
    }
}

/// Host camera expressed in the region's local coordinate system.
#[derive(Clone, Copy)]
pub struct Camera {
    /// Eye position in the same frame as the tree instances.
    pub origin: Vec3,
    /// Direction the camera looks in the region's coordinates.
    pub forward: Vec3,
    /// Camera right axis in the region's coordinates.
    pub right: Vec3,
    /// Camera up axis in the region's coordinates.
    pub up: Vec3,
    /// Drawable viewport width in physical pixels.
    pub width: u32,
    /// Drawable viewport height in physical pixels.
    pub height: u32,
    /// The host's reverse-Z projection, including any view jitter.
    pub clip_from_local: Mat4,
    /// Tangents of half the horizontal and vertical field of view.
    pub tan_half_fov: [f32; 2],
    /// Unit direction to the host's dominant directional light, in region coordinates.
    pub sun_direction: Vec3,
    /// Start and end distances for the transition from 3D trees to terrain canopy, in meters.
    pub forest_band: [f32; 2],
    /// Linear exposure multiplier relative to the authored forest brightness.
    pub exposure_scale: f32,
}

/// Triangle and instance counts from the three forest LODs.
#[derive(Clone, Copy, Default)]
pub struct Stats {
    /// Selected trees in the full, middle, and far groups.
    pub levels: [usize; 3],
    /// Trees blending between full and middle LODs and cells blending to far LOD.
    pub transitions: [usize; 2],
    /// Triangles submitted across all levels.
    pub triangles: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    translation: [f32; 4],
    rotation: [f32; 4],
}

impl Instance {
    fn world(self, p: Vec3) -> Vec3 {
        let (c, s) = (self.rotation[0], self.rotation[1]);
        Vec3::new(c * p.x + s * p.z, p.y, -s * p.x + c * p.z) * self.translation[3]
            + Vec3::from_slice(&self.translation)
    }
}
#[derive(Clone, Copy)]
struct Bounds {
    lo: Vec3,
    hi: Vec3,
}
impl Bounds {
    fn empty() -> Self {
        Self {
            lo: Vec3::splat(f32::INFINITY),
            hi: Vec3::splat(f32::NEG_INFINITY),
        }
    }
    fn add(&mut self, p: Vec3) {
        self.lo = self.lo.min(p);
        self.hi = self.hi.max(p);
    }
    fn merge(&mut self, b: Self) {
        self.add(b.lo);
        self.add(b.hi);
    }
    fn corners(self) -> [Vec3; 8] {
        std::array::from_fn(|i| {
            Vec3::new(
                if i & 1 == 0 { self.lo.x } else { self.hi.x },
                if i & 2 == 0 { self.lo.y } else { self.hi.y },
                if i & 4 == 0 { self.lo.z } else { self.hi.z },
            )
        })
    }
}
struct Placed {
    instance: Instance,
    model: usize,
    bounds: Bounds,
}
#[derive(Clone, Copy)]
struct View {
    origin: Vec3,
    forward: Vec3,
    right: Vec3,
    up: Vec3,
    focal: f32,
    sx: f32,
    sy: f32,
}

impl View {
    fn new(camera: Camera) -> Self {
        Self {
            origin: camera.origin,
            forward: camera.forward,
            right: camera.right,
            up: camera.up,
            focal: camera.height as f32 * 0.5 / camera.tan_half_fov[1],
            sx: camera.tan_half_fov[0],
            sy: camera.tan_half_fov[1],
        }
    }

    fn visible(self, b: Bounds) -> bool {
        let center = (b.lo + b.hi) * 0.5;
        let radius = (b.hi - b.lo).length() * 0.5 + 1.0;
        let d = center - self.origin;
        let z = d.dot(self.forward);
        z + radius > 0.05
            && d.dot(self.right).abs() <= z * self.sx + radius * (1.0 + self.sx * self.sx).sqrt()
            && d.dot(self.up).abs() <= z * self.sy + radius * (1.0 + self.sy * self.sy).sqrt()
    }

    fn span(self, b: Bounds) -> f32 {
        let mut lo = glam::Vec2::splat(f32::INFINITY);
        let mut hi = glam::Vec2::splat(f32::NEG_INFINITY);
        for p in b.corners() {
            let d = p - self.origin;
            let z = d.dot(self.forward);
            if z <= 0.05 {
                return f32::INFINITY;
            }
            let xy = glam::Vec2::new(d.dot(self.right), d.dot(self.up)) * self.focal / z;
            lo = lo.min(xy);
            hi = hi.max(xy);
        }
        (hi - lo).length()
    }
}

fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

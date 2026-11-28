//! Screen-space detail selection and shared, complementary dither materials.
use bevy::{
    pbr::{ExtendedMaterial, MaterialExtension},
    prelude::*,
    reflect::TypePath,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
};

const STEPS: f32 = 12.0;
const REFERENCE_FOCAL: f32 = 935.3074;
const REFERENCE_HEIGHT: f32 = 15.0;

/// Vegetation quality controls. Distances describe a 1080p, 60-degree reference view.
#[derive(Clone, Copy, Resource)]
pub struct VegetationSettings {
    /// Reference distances from detailed to simplified and from simplified to coarse meshes.
    pub mesh_distances: [f32; 2],
    /// Outer distance for the dithered forest cutoff, in meters.
    pub forest_range: f32,
    /// Extra radial distance to request chunks before they enter their display range.
    pub preload_margin: f32,
    /// Fractional hysteresis for the projected size thresholds.
    pub hysteresis: f32,
    /// Time in seconds to blend between resident representations.
    pub transition_seconds: f32,
}

impl Default for VegetationSettings {
    fn default() -> Self {
        Self {
            mesh_distances: [97.0, 242.0],
            forest_range: 2_000.0,
            preload_margin: 260.0,
            hysteresis: 0.12,
            transition_seconds: 0.40,
        }
    }
}

impl VegetationSettings {
    pub(super) fn forest_band(self) -> Vec2 {
        let end = self.forest_range.clamp(420.0, 3_000.0);
        Vec2::new(0.79 * end, end)
    }

    pub(super) fn selection(self) -> igor_renderer::Selection {
        let near = self.mesh_distances[0].clamp(2.0, 1_200.0);
        let far = self.mesh_distances[1].clamp(near + 1.0, 2_000.0);
        let time = self.transition_seconds.clamp(0.1, 2.0);
        igor_renderer::Selection {
            middle_pixels: REFERENCE_HEIGHT * REFERENCE_FOCAL / near,
            far_pixels: REFERENCE_HEIGHT * REFERENCE_FOCAL / far,
            cell_pixels: 210.0,
            hysteresis: self.hysteresis.clamp(0.0, 0.35),
            seconds: [time, time * (0.65 / 0.40)],
        }
    }
}

pub(super) struct View {
    pub focal: f32,
    pub orthographic: bool,
}

impl View {
    pub fn new(camera: &Camera, projection: &Projection) -> Self {
        let height = camera
            .physical_viewport_size()
            .map_or(1080.0, |size| size.y.max(1) as f32);
        let (focal, orthographic) = match projection {
            Projection::Perspective(view) => (height / (2.0 * (view.fov * 0.5).tan()), false),
            Projection::Orthographic(view) => (height / view.area.height().max(0.01), true),
            Projection::Custom(_) => (height, false),
        };
        Self {
            focal,
            orthographic,
        }
    }

    pub fn pixels(&self, diameter: f32, nearest_distance: f32) -> f32 {
        diameter * self.focal
            / if self.orthographic {
                1.0
            } else {
                nearest_distance.max(0.05)
            }
    }
}

#[derive(Clone, Copy)]
struct Blend<T> {
    target: T,
    progress: f32,
}

#[derive(Clone, Copy)]
pub(super) struct LodState<T> {
    pub desired: Option<T>,
    pub current: Option<T>,
    blend: Option<Blend<T>>,
}

impl<T> Default for LodState<T> {
    fn default() -> Self {
        Self {
            desired: None,
            current: None,
            blend: None,
        }
    }
}

impl<T: Copy + Eq> LodState<T> {
    pub fn update(&mut self, target: T, step: f32) {
        self.desired = Some(target);
        let Some(current) = self.current else {
            self.current = Some(target);
            return;
        };
        if let Some(blend) = &mut self.blend {
            // Reverse a transition without losing its current coverage. A third target
            // waits until one slot is free.
            blend.progress += if target == current { -step } else { step };
            if blend.progress <= 0.0 {
                self.blend = None;
            } else if blend.progress >= 1.0 {
                self.current = Some(blend.target);
                self.blend = None;
            }
        } else if current != target {
            self.blend = Some(Blend {
                target,
                progress: step.min(1.0),
            });
        }
    }
    pub fn slots(&self, upper: u8) -> Option<[(T, u8, u8); 2]> {
        let active = self.current?;
        let split = self.blend.map_or(upper, |blend| {
            (f32::from(upper) * (1.0 - blend.progress)).round() as u8
        });
        let incoming = self.blend.map_or(active, |blend| blend.target);
        Some([(active, 0, split), (incoming, split, upper)])
    }

    pub fn is_blending(&self) -> bool {
        self.blend.is_some()
    }
}

pub(super) fn quantize(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * STEPS + 0.5).floor() as u8
}

pub(super) fn coverage(value: u8) -> f32 {
    f32::from(value) / STEPS
}

pub(super) fn smoothstep(start: f32, end: f32, value: f32) -> f32 {
    let v = ((value - start) / (end - start)).clamp(0.0, 1.0);
    v * v * (3.0 - 2.0 * v)
}

#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub(super) struct BarkFade {
    #[uniform(100)]
    coverage: Vec4,
}

impl MaterialExtension for BarkFade {
    fn fragment_shader() -> ShaderRef {
        "shaders/vegetation_fade.wesl".into()
    }
}

pub(super) type BarkMaterial = ExtendedMaterial<StandardMaterial, BarkFade>;

/// A shared PBR material variant for an opaque or alpha-clipped dither interval.
pub(super) fn dither_material(base: StandardMaterial, low: u8, high: u8) -> BarkMaterial {
    BarkMaterial {
        base,
        extension: BarkFade {
            coverage: Vec4::new(coverage(low), coverage(high), 0.0, 0.0),
        },
    }
}

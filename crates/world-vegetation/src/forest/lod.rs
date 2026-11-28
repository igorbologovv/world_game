//! Screen-space detail selection and shared, complementary dither materials.
const STEPS: f32 = 12.0;

use bevy::{
    pbr::{ExtendedMaterial, MaterialExtension},
    prelude::*,
    reflect::TypePath,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
};

/// Tree streaming distances. Igor_renderer owns projected LOD thresholds and fade timing.
#[derive(Clone, Copy, Resource)]
pub struct VegetationSettings {
    /// Outer distance of the streamed forest and its terrain canopy coverage, in meters.
    pub forest_range: f32,
    /// Extra radial distance to request chunks before they enter their display range.
    pub preload_margin: f32,
}

impl Default for VegetationSettings {
    fn default() -> Self {
        Self {
            forest_range: 2_000.0,
            preload_margin: 260.0,
        }
    }
}

impl VegetationSettings {
    pub(super) fn forest_band(self) -> Vec2 {
        let end = self.forest_range.clamp(420.0, 3_000.0);
        Vec2::new(0.79 * end, end)
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

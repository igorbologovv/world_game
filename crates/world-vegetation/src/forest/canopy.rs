//! Bounded canopy atlas made from the same sites as the detailed forest.

mod bake;
mod crowns;
mod globe;

pub use bake::bake_planet_canopy;

use std::{collections::HashMap, sync::Arc};

use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    math::{DVec2, DVec3},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureViewDimension},
    tasks::{block_on, poll_once, AsyncComputeTaskPool, Task},
};
use world_core::{
    canopy::{CanopyCascade, CanopyLayout, ForestCanopy, CANOPY_CASCADES, CANOPY_SIDE},
    planet::RADIUS,
    runtime::{GridTransform, ReferenceFrames, WorldAssets, WorldCamera, WorldReady},
    SurfaceHeights,
};

use super::{
    lod::VegetationSettings,
    placement::{NodeKey, PatchKey, Site, PATCH_METERS},
    proxy::half_width,
    streaming::{self, ForestStream},
    ForestClearing,
};
use crowns::CrownRaster;

const SIDE: usize = CANOPY_SIDE as usize;
const TEXELS: usize = SIDE * SIDE;
const LAYERS: usize = CANOPY_CASCADES;
const HALF_WIDTHS: [f64; CANOPY_CASCADES] =
    [2_900.0, 8_700.0, 26_100.0, 78_300.0, 234_900.0, 704_700.0];
const PRIORITY: [usize; CANOPY_CASCADES] = [0, 2, 4, 1, 3, 5];
const MAX_JOBS: usize = 3;

#[derive(Resource, Default)]
struct AtlasStream {
    renderer: bool,
    jobs: [Option<Task<Layer>>; LAYERS],
    centers: [Option<DVec3>; CANOPY_CASCADES],
}

struct Layer {
    bytes: Vec<u8>,
    frame: Frame,
    crowns: Option<Vec<u8>>,
}

#[derive(Clone, Copy)]
struct Frame {
    up: DVec3,
    east: DVec3,
    north: DVec3,
    half_width: f64,
}

impl Frame {
    fn new(up: DVec3, half_width: f64) -> Self {
        let east = up.any_orthonormal_vector();
        Self {
            up,
            east,
            north: up.cross(east),
            half_width,
        }
    }

    fn project(self, up: DVec3) -> DVec2 {
        let projected = DVec2::new(up.dot(self.east), up.dot(self.north));
        projected * (RADIUS / (self.half_width * up.dot(self.up)))
    }

    fn direction(self, square: DVec2) -> DVec3 {
        (self.up + (self.east * square.x + self.north * square.y) * (self.half_width / RADIUS))
            .normalize()
    }

    fn meters_per_texel(self) -> f64 {
        self.half_width * 2.0 / f64::from(CANOPY_SIDE)
    }

    fn uniform(self) -> CanopyCascade {
        CanopyCascade {
            east: self
                .east
                .as_vec3()
                .extend((self.half_width / RADIUS) as f32),
            north: self.north.as_vec3().extend(self.meters_per_texel() as f32),
            center: self.up.as_vec3().extend(1.0),
        }
    }
}

pub(super) fn install(app: &mut App) {
    globe::install(app);
    app.insert_resource(AtlasStream {
        renderer: app.get_sub_app(bevy::render::RenderApp).is_some(),
        ..default()
    })
    .add_systems(Startup, create.after(WorldReady))
    .add_systems(Update, stream.after(streaming::stream));
}

fn create(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    heights: Res<SurfaceHeights>,
    assets: Res<WorldAssets>,
    stream: Res<AtlasStream>,
) {
    let mut image = Image::new(
        Extent3d {
            width: CANOPY_SIDE,
            height: CANOPY_SIDE,
            depth_or_array_layers: LAYERS as u32,
        },
        TextureDimension::D2,
        vec![0; TEXELS * LAYERS * 4],
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::default(),
    );
    image.texture_view_descriptor = Some(bevy::render::render_resource::TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..default()
    });
    image.sampler = ImageSampler::linear();
    let globe = globe::Globe::new(&mut images);
    if stream.renderer {
        globe.start(&assets, &heights);
    }
    commands.insert_resource(ForestCanopy {
        atlas: images.add(image),
        globe: globe.image.clone(),
        crowns: images.add(crowns::image()),
        layout: CanopyLayout::default(),
    });
    commands.insert_resource(globe);
}

fn stream(
    cameras: Query<(Entity, GridTransform), With<WorldCamera>>,
    frames: ReferenceFrames,
    heights: Res<SurfaceHeights>,
    settings: Res<VegetationSettings>,
    clearing: Option<Res<ForestClearing>>,
    forest: Res<ForestStream>,
    mut stream: ResMut<AtlasStream>,
    mut canopy: ResMut<ForestCanopy>,
    mut images: ResMut<Assets<Image>>,
    time: Res<Time>,
) {
    let Ok((entity, camera)) = cameras.single() else {
        return;
    };
    let Some(frame) = frames.parent_frame(entity) else {
        return;
    };
    let eye = frame.grid_position_double(&camera.cell, &camera.transform);
    let Some(up) = eye.try_normalize() else {
        return;
    };
    let fade = settings.proxy_range();
    if canopy.layout.fade.truncate().truncate() != fade {
        canopy.layout.fade.x = fade.x;
        canopy.layout.fade.y = fade.y;
    }

    let clearing = clearing.as_deref().copied();
    let clearing_uniform = clearing.map_or(Vec4::ZERO, |clearing| {
        clearing
            .direction
            .as_vec3()
            .extend(clearing.radius.abs() as f32)
    });
    if canopy.layout.clearing != clearing_uniform {
        canopy.layout.clearing = clearing_uniform;
    }
    for index in 0..LAYERS {
        let weight = canopy.layout.cascades[index].center.w;
        if stream.centers[index].is_some() && weight < 1.0 {
            canopy.layout.cascades[index].center.w =
                (weight + time.delta_secs().min(0.05) * 4.0).min(1.0);
        }
    }

    let stream = &mut *stream;
    for (index, job) in stream.jobs.iter_mut().enumerate() {
        let Some(layer) = job.as_mut().and_then(|job| block_on(poll_once(job))) else {
            continue;
        };
        *job = None;
        let start = index * TEXELS * 4;
        {
            let mut atlas = images
                .get_mut(&canopy.atlas)
                .expect("canopy retains its atlas");
            atlas.data.as_mut().expect("canopy atlas has CPU backing")[start..start + TEXELS * 4]
                .copy_from_slice(&layer.bytes);
        }
        if let Some(crowns) = layer.crowns {
            // Updating an existing Bevy image in place uploads only its base mip.
            // Publish a fresh texture and projection together for the full mip chain.
            let mut image = crowns::image();
            image.data = Some(crowns);
            canopy.crowns = images.add(image);
        }
        let weight = canopy.layout.cascades[index].center.w;
        canopy.layout.cascades[index] = layer.frame.uniform();
        canopy.layout.cascades[index].center.w = weight;
        stream.centers[index] = Some(layer.frame.up);
        // Publish at most one completed local layer per frame.
        break;
    }

    let mut budget = MAX_JOBS - stream.jobs.iter().flatten().count();
    if budget == 0 {
        return;
    }
    let pool = AsyncComputeTaskPool::get();
    let overground = eye.length() - RADIUS - heights.sample(up).unwrap_or(0.0);
    let finest_globe_resident = canopy.layout.fade.z == 0.0 && canopy.layout.fade.w == 1.0;
    for index in PRIORITY {
        let width = HALF_WIDTHS[index];
        // The largest projection is coarser than every native cubemap face.
        if (index == LAYERS - 1 && finest_globe_resident)
            || stream.jobs[index].is_some()
            || overground > width * 0.9
            || stream.centers[index]
                .is_some_and(|center| center.distance(up) * RADIUS < width * 0.24)
        {
            continue;
        }
        let frame = Frame::new(up, width);
        let heights = heights.clone();
        let cached = (index == 0).then(|| forest.cached_sites());
        stream.jobs[index] = Some(pool.spawn(async move {
            let (bytes, crowns) = match cached {
                Some(cached) => {
                    let mut raster = CrownRaster::new(frame.half_width);
                    let bytes = fine_layer(frame, &heights, clearing, cached, Some(&mut raster));
                    (bytes, Some(raster.finish()))
                }
                None => (coarse_local(frame, &heights, clearing), None),
            };
            Layer {
                bytes,
                frame,
                crowns,
            }
        }));
        budget -= 1;
        if budget == 0 {
            return;
        }
    }
}

// The impostor width describes the crown span of its actual species and variant.
// Area-weighted RGB gives wider trees proportionally more influence on canopy color.
fn crown(site: Site) -> Vec4 {
    let radius = half_width(site.species, site.variant) * site.height();
    let area = std::f32::consts::PI * radius * radius;
    let foliage =
        Vec3::from_array(site.species.colors().1) * (0.46 + 0.045 * (site.yaw * 2.73).sin());
    foliage.extend(1.0) * area
}

fn cover(area_ratio: Vec4) -> Vec4 {
    if area_ratio.w == 0.0 {
        return Vec4::ZERO;
    }
    // Convert accumulated crown area to fractional cover, allowing crowns to overlap.
    area_ratio * ((1.0 - (-area_ratio.w).exp()) / area_ratio.w)
}

fn encode(sample: Vec4) -> [u8; 4] {
    (sample.clamp(Vec4::ZERO, Vec4::ONE) * 255.0)
        .round()
        .to_array()
        .map(|value| value as u8)
}

fn fine_layer(
    frame: Frame,
    heights: &SurfaceHeights,
    clearing: Option<ForestClearing>,
    cached: HashMap<PatchKey, Arc<[Site]>>,
    mut crowns: Option<&mut CrownRaster>,
) -> Vec<u8> {
    let mut sums = vec![Vec4::ZERO; TEXELS];
    let east = frame.east.as_vec3();
    let north = frame.north.as_vec3();
    // Beyond the polar tree line, the placement grid has more columns than
    // the projected area warrants. No sites can survive the latitude gate.
    if frame.up.y.abs() < 0.962 {
        let margin = PATCH_METERS * 2.0;
        let reach = (frame.half_width + margin) * std::f64::consts::SQRT_2 + 480.0;
        for patch in NodeKey::around(frame.up, reach)
            .into_iter()
            .flat_map(NodeKey::leaves)
        {
            if frame.project(patch.center()).abs().max_element() > 1.0 + margin / frame.half_width {
                continue;
            }
            let mut add = |site: Site| {
                let square = frame.project(site.position.normalize());
                if let Some(raster) = crowns.as_deref_mut() {
                    let axis = site.rotation() * Vec3::X;
                    let orientation = Vec2::new(axis.dot(east), axis.dot(north)).normalize();
                    raster.add(site, square, orientation);
                }
                let xy = (square * 0.5 + 0.5) * f64::from(CANOPY_SIDE) - 0.5;
                let low = xy.floor();
                let blend = xy - low;
                let low = low.as_ivec2();
                let value = crown(site);
                for (offset, weight) in [
                    (IVec2::ZERO, (1.0 - blend.x) * (1.0 - blend.y)),
                    (IVec2::X, blend.x * (1.0 - blend.y)),
                    (IVec2::Y, (1.0 - blend.x) * blend.y),
                    (IVec2::ONE, blend.x * blend.y),
                ] {
                    let pixel = low + offset;
                    if pixel.min_element() >= 0 && pixel.max_element() < SIDE as i32 {
                        sums[pixel.y as usize * SIDE + pixel.x as usize] += value * weight as f32;
                    }
                }
            };
            if let Some(sites) = cached.get(&patch) {
                sites.iter().copied().for_each(&mut add);
            } else {
                patch.sample_sites(heights, clearing, 1).for_each(&mut add);
            }
        }
    }
    let texel_area = frame.meters_per_texel().powi(2) as f32;
    sums.into_iter()
        .flat_map(|sum| encode(cover(sum / texel_area)))
        .collect()
}

fn coarse_local(
    frame: Frame,
    heights: &SurfaceHeights,
    clearing: Option<ForestClearing>,
) -> Vec<u8> {
    raster_coarse(heights, clearing, frame.meters_per_texel(), |pixel| {
        frame.direction((pixel + 0.5) * (2.0 / f64::from(CANOPY_SIDE)) - 1.0)
    })
}

fn raster_coarse(
    heights: &SurfaceHeights,
    clearing: Option<ForestClearing>,
    texel_meters: f64,
    direction: impl Fn(DVec2) -> DVec3,
) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(TEXELS * 4);
    let mut sample_cache = HashMap::new();
    let angular_offset = 0.27 * texel_meters.max(PATCH_METERS) / RADIUS;
    // Stride 8 near the geometry handoff, then four canonical candidates per
    // patch for broader averages. Work is bounded by texels, not terrain area.
    let stride = if texel_meters < 500.0 { 8 } else { 28 };
    for y in 0..CANOPY_SIDE {
        for x in 0..CANOPY_SIDE {
            let up = direction(DVec2::new(f64::from(x), f64::from(y)));
            let east = up.any_orthonormal_vector() * angular_offset;
            let north = up.cross(east);
            let mut sum = Vec4::ZERO;
            for offset in [east + north, east - north, -east + north, -east - north] {
                let patch = PatchKey::containing((up + offset).normalize());
                sum += *sample_cache.entry(patch).or_insert_with(|| {
                    if patch.center().y.abs() > 0.9601 {
                        return Vec4::ZERO;
                    }
                    let area = patch.area().max(1.0) as f32;
                    let crowns = patch
                        .sample_sites(heights, clearing, stride)
                        .map(crown)
                        .sum::<Vec4>();
                    cover(crowns * (stride as f32 / area))
                });
            }
            pixels.extend(encode(sum * 0.25));
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use world_core::coordinates::terrain_aligned_direction;

    #[test]
    fn near_canopy_uses_canonical_crowns_and_respects_clearings() {
        let heights = SurfaceHeights::load(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets"
        )));
        let up = terrain_aligned_direction(world_regions::PALM_COAST_REGION.spawn.direction);
        let frame = Frame::new(up, 220.0);
        let coverage = fine_layer(frame, &heights, None, HashMap::new(), None);
        assert!(coverage.chunks_exact(4).any(|pixel| pixel[3] > 24));
        let mut raster = CrownRaster::new(frame.half_width);
        assert_eq!(
            coverage,
            fine_layer(frame, &heights, None, HashMap::new(), Some(&mut raster))
        );
        let clearing = ForestClearing {
            direction: up,
            radius: 400.0,
        };
        let cleared = fine_layer(frame, &heights, Some(clearing), HashMap::new(), None);
        assert!(cleared.iter().all(|&value| value == 0));

        let patch = NodeKey::around(up, 1_200.0)
            .into_iter()
            .flat_map(NodeKey::leaves)
            .find(|patch| patch.sample_sites(&heights, None, 8).next().is_some())
            .unwrap();
        let sites = patch.sites(&heights);
        assert!(patch
            .sample_sites(&heights, None, 8)
            .all(|site| sites.contains(&site)));
    }

    #[test]
    fn clipmap_projection_round_trips_across_cube_edges() {
        let frame = Frame::new(DVec3::new(1.0, 0.23, -1.0).normalize(), HALF_WIDTHS[3]);
        for square in [DVec2::new(-0.7, 0.42), DVec2::ZERO, DVec2::new(0.55, -0.9)] {
            assert!(frame
                .project(frame.direction(square))
                .abs_diff_eq(square, 1e-12));
        }
    }
}

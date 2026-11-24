use std::{fs, path::Path};

use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::NoFrustumCulling,
    image::{
        CompressedImageFormats, ImageAddressMode, ImageFilterMode, ImageFormat, ImageSampler,
        ImageSamplerDescriptor, ImageType,
    },
    light::NotShadowCaster,
    material::AlphaMode,
    math::{DVec2, DVec3},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    reflect::TypePath,
    render::render_resource::{AsBindGroup, Extent3d, TextureDimension, TextureFormat},
    shader::ShaderRef,
};
use world_core::{
    coordinates::{cube_direction, height_source_uv, terrain_baked_uv, CUBE_CORRECTION},
    planet::{decode_height, HEIGHT_MAP_SIZE, OCEAN_ATLAS_PATH, RADIUS, WATER_SURFACE_ELEVATION},
    runtime::{GridTransform, PlanetRoot, ReferenceFrame, ReferenceFrames, WorldCamera},
    SurfaceHeights,
};

const WATER_RADIUS: f64 = RADIUS + WATER_SURFACE_ELEVATION;
const PATCHES_PER_FACE: u32 = 8;
// Ocean vertices lie on the water sphere. Between vertices the triangles
// approximate its surface, including a few meters of chord sag at Mars scale.
const SEGMENTS: u32 = 48;
const DETAIL_RADIUS: f32 = 24_000.0;
const DETAIL_MAX_ALTITUDE: f32 = 1_500.0;
const DETAIL_SECTORS: u32 = 224;
// The previous sRGB texture classified a pixel as water at 0.25 linear.
const MIN_WATER: u8 = 137;
const DEPTH_OFFSET: f64 = 16.0;
const DEPTH_RANGE: f64 = 96.0;
const WAVE_NORMALS_PATH: &str = "textures/ocean/wave-normals.ktx2";
const STORM_FADE_SECONDS: f64 = 3.0;

#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub(super) struct OceanMaterial {
    #[texture(0)]
    #[sampler(1)]
    mask: Handle<Image>,
    #[texture(2)]
    #[sampler(3)]
    wave_normals: Handle<Image>,
    #[uniform(4)]
    geometry: Vec4,
    #[uniform(4)]
    projection: Vec2,
    #[uniform(4)]
    origin_radial: Vec4,
    #[uniform(4)]
    storm_center: Vec4,
    #[uniform(4)]
    storm_fade: Vec4,
}

impl OceanMaterial {
    pub(super) fn new(mask: Handle<Image>, wave_normals: Handle<Image>) -> Self {
        Self {
            mask,
            wave_normals,
            geometry: Vec4::new(
                WATER_RADIUS as f32,
                DETAIL_RADIUS,
                DETAIL_MAX_ALTITUDE,
                (SEGMENTS * PATCHES_PER_FACE) as f32,
            ),
            // The normalized offset in the baked terrain's UV-to-texel mapping.
            projection: Vec2::new(
                CUBE_CORRECTION as f32,
                (terrain_baked_uv(DVec2::ZERO).x - 0.5 / f64::from(HEIGHT_MAP_SIZE)) as f32,
            ),
            origin_radial: origin_radial(DVec3::ZERO),
            storm_center: Vec4::ZERO,
            storm_fade: Vec4::ZERO,
        }
    }
}

impl Material for OceanMaterial {
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn vertex_shader() -> ShaderRef {
        "shaders/ocean.wesl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/ocean.wesl".into()
    }
}

#[derive(Component)]
pub(super) struct OceanPatch {
    direction: DVec3,
    angle_cos: f64,
    angle_sin: f64,
}

#[derive(Component)]
pub(super) struct OceanDetail;

fn origin_radial(origin: DVec3) -> Vec4 {
    origin
        .normalize_or_zero()
        .as_vec3()
        .extend((origin.length() - WATER_RADIUS) as f32)
}

#[derive(Default)]
pub(super) struct StormControl {
    center: Vec3,
    from: f32,
    target: f32,
    started: f64,
    render_start: f32,
}

impl StormControl {
    fn strength_at(&self, now: f64) -> f32 {
        let t = ((now - self.started) / STORM_FADE_SECONDS).clamp(0.0, 1.0) as f32;
        let weight = t * t * (3.0 - 2.0 * t);
        self.from + (self.target - self.from) * weight
    }

    fn toggle(&mut self, now: f64, render_start: f32, direction: Vec3) {
        let current = self.strength_at(now);
        if current == 0.0 && self.target == 0.0 {
            self.center = direction;
        }
        self.from = current;
        self.target = 1.0 - self.target;
        self.started = now;
        self.render_start = render_start;
    }

    fn finish_fade(&mut self, now: f64) {
        if now - self.started >= STORM_FADE_SECONDS {
            self.from = self.target;
        }
    }

    fn shader_settings(&self, wrap_seconds: f32) -> (Vec4, Vec4) {
        (
            self.center.extend(self.render_start),
            Vec4::new(
                self.from,
                self.target,
                STORM_FADE_SECONDS as f32,
                wrap_seconds,
            ),
        )
    }
}

pub(super) fn sync_material(
    root: Res<PlanetRoot>,
    grids: Query<&ReferenceFrame>,
    cameras: Query<GridTransform, With<WorldCamera>>,
    detail: Query<&MeshMaterial3d<OceanMaterial>, With<OceanDetail>>,
    keyboard: Option<Res<ButtonInput<KeyCode>>>,
    time: Res<Time>,
    mut materials: ResMut<Assets<OceanMaterial>>,
    mut storm: Local<StormControl>,
    mut last_settings: Local<Option<(AssetId<OceanMaterial>, DVec3, Vec4, Vec4)>>,
) {
    let Ok(grid) = grids.get(root.entity()) else {
        return;
    };
    let now = time.elapsed_secs_f64();
    if keyboard.is_some_and(|keys| keys.just_pressed(KeyCode::F7)) {
        if let Ok(camera) = cameras.single() {
            let eye = grid.grid_position_double(&camera.cell, &camera.transform);
            let direction = eye.normalize_or_zero().as_vec3();
            if direction != Vec3::ZERO {
                storm.toggle(now, time.elapsed_secs_wrapped(), direction);
            }
        }
    }
    storm.finish_fade(now);
    let Ok(material) = detail.single() else {
        return;
    };
    let floating_origin = grid.local_floating_origin();
    let origin = grid.grid_position_double(
        &floating_origin.cell(),
        &Transform::from_translation(floating_origin.translation()),
    );
    let (storm_center, storm_fade) = storm.shader_settings(time.wrap_period().as_secs_f32());
    let revision = (material.0.id(), origin, storm_center, storm_fade);
    if *last_settings == Some(revision) {
        return;
    }
    if let Some(mut material) = materials.get_mut(&material.0) {
        material.origin_radial = origin_radial(origin);
        material.storm_center = storm_center;
        material.storm_fade = storm_fade;
        *last_settings = Some(revision);
    }
}

pub(super) fn spawn_detail(
    root: &PlanetRoot,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    material: Handle<OceanMaterial>,
) {
    root.spawn(
        commands,
        (
            Mesh3d(meshes.add(build_detail())),
            MeshMaterial3d(material),
            OceanDetail,
            NoFrustumCulling,
            NotShadowCaster,
        ),
    );
}

fn build_detail() -> Mesh {
    let mut positions = vec![[0.0; 3]];
    let mut spacing = vec![[4.0; 2]];
    let mut indices = Vec::new();
    let sectors = DETAIL_SECTORS;
    let angles: Vec<_> = (0..sectors)
        .map(|i| (std::f32::consts::TAU * i as f32 / sectors as f32).sin_cos())
        .collect();
    let mut radius = 0.0;
    let mut start = 1;
    // The disk extends beyond the shader's circular handoff. Ring spacing grows
    // with distance, so individual ripples use vertices only near the camera.
    while radius < DETAIL_RADIUS + 150.0 {
        let step = 4.0 + 0.017 * radius;
        radius += step;
        let arc = std::f32::consts::TAU * radius / sectors as f32;
        for (sin, cos) in angles.iter().copied() {
            positions.push([radius * cos, radius * sin, 0.0]);
            spacing.push([step, arc]);
        }
        for i in 0..sectors {
            let a = start + i;
            let b = start + (i + 1) % sectors;
            if start == 1 {
                indices.extend_from_slice(&[0, a, b]);
            } else {
                let c = a - sectors;
                let d = b - sectors;
                indices.extend_from_slice(&[c, a, b, c, b, d]);
            }
        }
        start += sectors;
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    // UV1 selects the camera-centered geometry path and carries vertex spacing.
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, spacing);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

pub(super) struct OceanSurface {
    pub(super) mask: Image,
    pub(super) wave_normals: Image,
    pub(super) patches: Vec<(Mesh, OceanPatch)>,
}

pub(super) fn load(heights: &SurfaceHeights, assets: &Path) -> Option<OceanSurface> {
    let path = assets.join(OCEAN_ATLAS_PATH);
    let mask = match image::open(&path) {
        Ok(mask) => mask.into_luma8(),
        Err(error) => {
            warn!("Could not load water mask {}: {error}", path.display());
            return None;
        }
    };
    let side = mask.width() / 3;
    if side == 0 || mask.width() != side * 3 || mask.height() != side * 2 {
        warn!("Invalid water mask dimensions in {}", path.display());
        return None;
    }

    let mut patches = Vec::new();
    for face in 0..6 {
        let origin = UVec2::new(face as u32 % 3, face as u32 / 3) * side;
        for y in 0..PATCHES_PER_FACE {
            for x in 0..PATCHES_PER_FACE {
                let start = DVec2::new(f64::from(x), f64::from(y)) / f64::from(PATCHES_PER_FACE);
                let end = start + 1.0 / f64::from(PATCHES_PER_FACE);
                let first = source_pixel(start, side).floor().as_uvec2() + origin;
                let last = source_pixel(end, side).ceil().as_uvec2() + origin;
                let has_water = (first.y..=last.y).any(|y| {
                    let row = (y * mask.width()) as usize;
                    mask.as_raw()[row + first.x as usize..=row + last.x as usize]
                        .iter()
                        .any(|&value| value >= MIN_WATER)
                });
                if has_water {
                    patches.push(build_patch(face, start, end, origin, side));
                }
            }
        }
    }
    let mut pixels = vec![0; mask.as_raw().len() * 4];
    for face in 0..6 {
        let origin = UVec2::new(face as u32 % 3, face as u32 / 3) * side;
        let texels = heights.face_texels(face, side);
        for y in 0..side {
            let atlas_start = ((origin.y + y) * mask.width() + origin.x) as usize;
            let source_start = (y * side) as usize;
            for x in 0..side as usize {
                let atlas_index = atlas_start + x;
                let coverage = mask.as_raw()[atlas_index];
                let depth = texels.map_or(45.0, |values| {
                    WATER_RADIUS - RADIUS - decode_height(f64::from(values[source_start + x]))
                });
                let start = 4 * atlas_index;
                pixels[start..start + 4].copy_from_slice(&atlas_pixel(coverage, depth));
            }
        }
    }
    let mut atlas = Image::new(
        Extent3d {
            width: mask.width(),
            height: mask.height(),
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    atlas.sampler = ImageSampler::linear();
    Some(OceanSurface {
        mask: atlas,
        wave_normals: load_wave_normals(assets),
        patches,
    })
}

fn atlas_pixel(coverage: u8, depth: f64) -> [u8; 4] {
    let legacy_depth =
        ((depth + DEPTH_OFFSET).clamp(0.0, DEPTH_RANGE) * (255.0 / DEPTH_RANGE)).round() as u8;
    // Retain the old channel for legacy inland water in older atlases. Ocean
    // depth uses quarter meters biased by 8192 m to keep steep shores filterable.
    let depth_code = (depth * 4.0 + 32_768.0).round().clamp(0.0, 65_535.0) as u16;
    let [high, low] = depth_code.to_be_bytes();
    [coverage, legacy_depth, high, low]
}

fn load_wave_normals(assets: &Path) -> Image {
    let path = assets.join(WAVE_NORMALS_PATH);
    let sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 4,
        ..ImageSamplerDescriptor::linear()
    });
    let image = fs::read(&path)
        .map_err(|err| err.to_string())
        .and_then(|bytes| {
            Image::from_buffer(
                &bytes,
                ImageType::Format(ImageFormat::Ktx2),
                CompressedImageFormats::empty(),
                false,
                sampler.clone(),
                RenderAssetUsages::RENDER_WORLD,
            )
            .map_err(|err| err.to_string())
        });
    match image {
        Ok(image) => image,
        Err(error) => {
            warn!(
                "Could not load ocean wave normals {}: {error}",
                path.display()
            );
            let mut flat = Image::new(
                Extent3d::default(),
                TextureDimension::D2,
                vec![128; 4],
                TextureFormat::Rgba8Unorm,
                RenderAssetUsages::RENDER_WORLD,
            );
            flat.sampler = sampler;
            flat
        }
    }
}

fn source_pixel(uv: DVec2, side: u32) -> DVec2 {
    height_source_uv(uv, DVec2::splat(f64::from(side))) * f64::from(side - 1)
}

fn build_patch(
    face: usize,
    start: DVec2,
    end: DVec2,
    origin: UVec2,
    side: u32,
) -> (Mesh, OceanPatch) {
    let stride = (SEGMENTS + 1) as usize;
    let mut positions = Vec::with_capacity(stride * stride);
    let mut uvs = Vec::with_capacity(stride * stride);
    let mut indices = Vec::with_capacity((SEGMENTS * SEGMENTS * 6) as usize);
    let mid = cube_direction(face, (start + end) * 0.5);
    let mut angular_radius: f64 = 0.0;
    let step = (end - start) / f64::from(SEGMENTS);

    for y in 0..=SEGMENTS {
        for x in 0..=SEGMENTS {
            let uv = start + step * DVec2::new(f64::from(x), f64::from(y));
            let direction = cube_direction(face, uv);
            angular_radius = angular_radius.max(mid.distance_squared(direction));
            positions.push((direction * WATER_RADIUS).as_vec3().to_array());
            uvs.push((origin.as_vec2() + source_pixel(uv, side).as_vec2()).to_array());
        }
    }

    for y in 0..SEGMENTS {
        for x in 0..SEGMENTS {
            let a = (y * (SEGMENTS + 1) + x) as u16;
            let b = a + 1;
            let c = a + stride as u16;
            let d = c + 1;
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U16(indices));
    // Pad the cap to cover the difference between triangle normals and the
    // radial directions at their vertices.
    let angle = 2.0 * (angular_radius.sqrt() * 0.5).asin() + 0.01;
    let (angle_sin, angle_cos) = angle.sin_cos();
    (
        mesh,
        OceanPatch {
            direction: mid,
            angle_cos,
            angle_sin,
        },
    )
}

pub(super) fn cull_patches(
    cameras: Query<(Entity, GridTransform), With<WorldCamera>>,
    frames: ReferenceFrames,
    mut patches: Query<(&OceanPatch, &mut Visibility)>,
    mut detail: Query<&mut Visibility, (With<OceanDetail>, Without<OceanPatch>)>,
) {
    let Ok((entity, camera)) = cameras.single() else {
        return;
    };
    let Some(frame) = frames.parent_frame(entity) else {
        return;
    };
    let eye = frame.grid_position_double(&camera.cell, &camera.transform);
    let eye_radius = eye.length();
    let detail_visibility = if eye_radius - WATER_RADIUS < f64::from(DETAIL_MAX_ALTITUDE) + 1.0 {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut visibility in &mut detail {
        visibility.set_if_neq(detail_visibility);
    }
    let eye_direction = eye.normalize_or_zero();
    let horizon_cos = ((WATER_RADIUS - 1.0) / eye_radius).min(1.0);
    let horizon_sin = (1.0 - horizon_cos * horizon_cos).sqrt();
    for (patch, mut visibility) in &mut patches {
        let limit = horizon_cos * patch.angle_cos - horizon_sin * patch.angle_sin;
        let visible = eye_direction.dot(patch.direction) >= limit;
        let desired = if visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        visibility.set_if_neq(desired);
    }
}

#[cfg(test)]
mod tests {
    use bevy::render::mesh::VertexAttributeValues;

    use super::*;

    #[test]
    fn toggling_a_fading_storm_preserves_its_strength_and_location() {
        let mut storm = StormControl::default();
        storm.toggle(3599.0, 3599.0, Vec3::X);
        let midway = 3599.0 + STORM_FADE_SECONDS * 0.5;
        assert_eq!(storm.strength_at(midway), 0.5);
        storm.toggle(midway, 0.5, Vec3::Y);
        assert_eq!(storm.center, Vec3::X);
        assert_eq!(storm.strength_at(midway), 0.5);
        assert_eq!(storm.shader_settings(3600.0).0.w, 0.5);
        let finished = midway + STORM_FADE_SECONDS;
        storm.finish_fade(finished);
        assert_eq!(storm.from, 0.0);
        storm.toggle(finished, 3.5, Vec3::Y);
        assert_eq!(storm.center, Vec3::Y);
        assert_eq!(storm.strength_at(finished + STORM_FADE_SECONDS), 1.0);
    }

    #[test]
    fn radial_datum_keeps_a_level_camera_at_the_same_height_across_a_rebase() {
        let direction = DVec3::new(-0.977, 0.213, 0.015).normalize();
        let tangent = direction.cross(DVec3::Y).normalize();
        let mut previous_origin = None;
        let mut rebased = false;
        for meter in -1100..1100 {
            let up = (direction + tangent * f64::from(meter) / WATER_RADIUS).normalize();
            let eye = up * (WATER_RADIUS + 12.0);
            let origin = (eye / 2000.0).round() * 2000.0;
            rebased |= previous_origin.is_some_and(|previous| previous != origin);
            previous_origin = Some(origin);
            let (point, datum) = ((eye - origin).as_vec3(), origin_radial(origin));
            let radial = datum.truncate();
            let radius = WATER_RADIUS as f32 + datum.w;
            let delta = (2.0 * radius * radial.dot(point) + point.length_squared())
                / (radius + (radius * radial + point).length());
            assert!((datum.w + delta - 12.0).abs() < 0.001);
        }
        assert!(rebased);
    }

    #[test]
    fn detail_mesh_covers_the_handoff_with_outward_facing_triangles() {
        let mesh = build_detail();
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("Expected detail grid positions");
        };
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("Expected indexed detail grid");
        };
        let outer = Vec3::from_array(*positions.last().unwrap()).length();
        let inradius = outer * (std::f32::consts::PI / DETAIL_SECTORS as f32).cos();
        assert!(inradius > DETAIL_RADIUS);
        assert!(positions.len() < 65_536);
        for triangle in indices.chunks_exact(3) {
            let point = |index| Vec3::from_array(positions[index as usize]);
            let a = point(triangle[0]);
            let b = point(triangle[1]);
            let c = point(triangle[2]);
            assert!((b - a).cross(c - a).z > 0.0);
        }
    }

    #[test]
    fn ocean_depth_filters_across_the_waterline_and_byte_boundaries() {
        let land = atlas_pixel(0, -91.5);
        let ocean = atlas_pixel(255, 348.25);
        assert_eq!((land[0], ocean[0]), (0, 255));
        for weight in [0.0, 0.19, 0.25, 0.5, 1.0] {
            let filter = |channel: usize| {
                (f32::from(land[channel]) * (1.0 - weight) + f32::from(ocean[channel]) * weight)
                    / 255.0
            };
            let decoded = (filter(2) * 65_280.0 + filter(3) * 255.0) * 0.25 - 8192.0;
            let expected = -91.5 * (1.0 - weight) + 348.25 * weight;
            assert!((decoded - expected).abs() < 0.002);
        }
    }

    #[test]
    fn shipped_wave_normals_load_as_linear_data_with_a_full_mip_chain() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let wave_normals = load_wave_normals(&assets);
        let descriptor = &wave_normals.texture_descriptor;
        assert_eq!(descriptor.format, TextureFormat::Rgba8Unorm);
        assert_eq!((descriptor.size.width, descriptor.size.height), (512, 512));
        assert_eq!(descriptor.mip_level_count, 10);
        let ImageSampler::Descriptor(sampler) = wave_normals.sampler else {
            panic!("Expected a repeating wave sampler");
        };
        assert_eq!(sampler.address_mode_u, ImageAddressMode::Repeat);
        assert_eq!(sampler.address_mode_v, ImageAddressMode::Repeat);
        assert_eq!(sampler.mipmap_filter, ImageFilterMode::Linear);
    }

    #[test]
    fn patch_triangles_face_outward_with_vertices_on_the_mars_water_datum() {
        // The corrected cube projection stretches vertices most near corners.
        let patch_size = 1.0 / f64::from(PATCHES_PER_FACE);
        for face in 0..6 {
            let (mesh, _) = build_patch(
                face,
                DVec2::ZERO,
                DVec2::splat(patch_size),
                UVec2::ZERO,
                1024,
            );
            let Some(VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                panic!("Expected ocean vertex positions");
            };
            let Some(Indices::U16(indices)) = mesh.indices() else {
                panic!("Expected indexed ocean triangles");
            };
            for triangle in indices.chunks_exact(3) {
                let point = |index| Vec3::from_array(positions[usize::from(index)]).as_dvec3();
                let a = point(triangle[0]);
                let b = point(triangle[1]);
                let c = point(triangle[2]);
                let normal = (b - a).cross(c - a);
                assert!(normal.dot(a) > 0.0);
                let tolerance = WATER_RADIUS * f64::from(f32::EPSILON);
                assert!([a, b, c]
                    .iter()
                    .all(|point| { (point.length() - WATER_RADIUS).abs() < tolerance }));
            }
        }
    }
}

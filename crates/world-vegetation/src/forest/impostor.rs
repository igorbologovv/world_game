//! Bake orthographic albedo and normal views of the canonical tree meshes.
use std::{
    f32::consts::TAU,
    fs::File,
    io::{self, BufReader, Read},
    path::Path,
    sync::Arc,
};

use bevy::{
    asset::RenderAssetUsages,
    mesh::VertexAttributeValues,
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
    },
};

use flate2::bufread::GzDecoder;

use super::{generate_tree, textures, MeshBuilder, Species, FOLIAGE_CUTOFF, VARIANTS};

const SIDE: usize = 96;
const HORIZON_VIEWS: usize = 16;
const ELEVATED_VIEWS: usize = 8;
const VIEWS: usize = HORIZON_VIEWS + ELEVATED_VIEWS + 1;
const TILES: usize = Species::Palm as usize * VARIANTS + Species::Palm.variants();
const INSET: f32 = 0.022;
const MIP_LEVELS: u32 = 5;
const MAGIC: &[u8; 8] = b"TREEIMP1";
const TEXTURE_BYTES: usize = mip_bytes();
pub(super) const ATLAS_PATH: &str = "models/vegetation/tree-impostors-v1.gz";

const fn mip_bytes() -> usize {
    let mut size = 0;
    let mut level = 0;
    while level < MIP_LEVELS {
        let side = SIDE >> level;
        size += side * side * TILES * VIEWS * 4;
        level += 1;
    }
    size
}

pub(super) fn tile(species: Species, variant: usize) -> usize {
    species as usize * VARIANTS + variant
}

#[derive(Clone, Default)]
pub(super) struct Portraits([Option<Arc<Source>>; TILES]);

impl Portraits {
    pub fn insert(&mut self, species: Species, variant: usize, source: Source) {
        self.0[tile(species, variant)] = Some(Arc::new(source));
    }

    fn fingerprint(&self) -> blake3::Hash {
        let mut hash = blake3::Hasher::new();
        for source in &self.0 {
            hash.update(&[u8::from(source.is_some())]);
            let Some(source) = source else { continue };
            for part in &source.parts {
                // The pack follows the actual decoded LOD0, texture and material
                // data used by the baker, including custom asset roots.
                fn update<const N: usize>(hash: &mut blake3::Hasher, values: &[[f32; N]]) {
                    hash.update(&(values.len() as u64).to_le_bytes());
                    let bytes: Vec<_> = values
                        .iter()
                        .flatten()
                        .flat_map(|v| v.to_le_bytes())
                        .collect();
                    hash.update(&bytes);
                }
                update(&mut hash, &part.mesh.positions);
                update(&mut hash, &part.mesh.normals);
                update(&mut hash, &part.mesh.uvs);
                update(&mut hash, &part.mesh.colors);
                hash.update(&(part.mesh.indices.len() as u64).to_le_bytes());
                let indices: Vec<_> = part
                    .mesh
                    .indices
                    .iter()
                    .flat_map(|i| i.to_le_bytes())
                    .collect();
                hash.update(&indices);
                let size = part.diffuse.texture_descriptor.size;
                hash.update(&size.width.to_le_bytes());
                hash.update(&size.height.to_le_bytes());
                let count = size.width as usize * size.height as usize * 4;
                hash.update(&part.diffuse.data.as_ref().expect("CPU image bytes")[..count]);
                update(&mut hash, &[part.tint.to_array()]);
                update(&mut hash, &[[part.cutoff, part.transmission]]);
                hash.update(&[u8::from(part.repeat), u8::from(part.double_sided)]);
            }
        }
        hash.finalize()
    }

    #[cfg(test)]
    pub fn contains(&self, species: Species, variant: usize) -> bool {
        self.0[tile(species, variant)].is_some()
    }

    #[cfg(test)]
    pub fn silhouette_score(
        &self,
        species: Species,
        variant: usize,
        meshes: [&Mesh; 2],
    ) -> (f32, f32) {
        let reference = self.0[tile(species, variant)].as_ref().unwrap();
        let parts = std::array::from_fn(|index| {
            let template = &reference.parts[index];
            Part {
                mesh: Part::geometry(meshes[index]).unwrap(),
                diffuse: Arc::clone(&template.diffuse),
                tint: template.tint,
                cutoff: template.cutoff,
                repeat: template.repeat,
                double_sided: template.double_sided,
                transmission: template.transmission,
            }
        });
        let model = Source {
            parts,
            frame: reference.frame,
        };
        silhouette_score(reference, &model)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Framing {
    pub center: f32,
    pub radius: f32,
}

pub(super) type Frames = [Framing; TILES];

pub(super) struct Atlas {
    pub colors: Image,
    pub normals: Image,
    pub frames: Frames,
}

pub(super) struct Source {
    parts: [Part; 2],
    frame: Framing,
}

pub(super) struct Part {
    mesh: MeshBuilder,
    diffuse: Arc<Image>,
    tint: Vec4,
    cutoff: f32,
    repeat: bool,
    double_sided: bool,
    transmission: f32,
}

impl Source {
    pub fn new(parts: [Part; 2]) -> Self {
        let positions = || parts.iter().flat_map(|part| &part.mesh.positions);
        let (min, max) = positions().fold((0.0_f32, 0.0_f32), |(lo, hi), p| {
            (lo.min(p[1]), hi.max(p[1]))
        });
        let center = (min + max) * 0.5;
        let radius = positions()
            .map(|&p| (Vec3::from_array(p) - Vec3::Y * center).length())
            .fold(0.0_f32, f32::max);
        Self {
            parts,
            frame: Framing {
                center,
                radius: radius / (1.0 - 2.0 * INSET),
            },
        }
    }

    fn procedural(species: Species, variant: usize, bark: Arc<Image>, diffuse: Arc<Image>) -> Self {
        let (trunk, canopy) = generate_tree(species, variant as u64, 0);
        Self::new([
            Part {
                mesh: trunk,
                diffuse: bark,
                tint: Vec4::ONE,
                cutoff: 0.0,
                repeat: true,
                double_sided: false,
                transmission: 0.0,
            },
            Part {
                mesh: canopy,
                diffuse,
                tint: Vec4::ONE,
                cutoff: FOLIAGE_CUTOFF,
                repeat: false,
                double_sided: true,
                transmission: 0.34,
            },
        ])
    }
}

impl Part {
    pub fn authored(
        mesh: &Mesh,
        diffuse: &Image,
        material: &StandardMaterial,
        repeat: bool,
    ) -> Option<Self> {
        if diffuse.texture_descriptor.format != TextureFormat::Rgba8UnormSrgb {
            return None;
        }
        Some(Self {
            mesh: Self::geometry(mesh)?,
            diffuse: Arc::new(diffuse.clone()),
            tint: material.base_color.to_linear().to_vec4(),
            cutoff: match material.alpha_mode {
                AlphaMode::Mask(cutoff) => cutoff,
                _ => 0.0,
            },
            repeat,
            double_sided: material.double_sided,
            transmission: material.diffuse_transmission,
        })
    }

    fn geometry(mesh: &Mesh) -> Option<MeshBuilder> {
        let VertexAttributeValues::Float32x3(positions) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)?
        else {
            return None;
        };
        let VertexAttributeValues::Float32x3(normals) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL)?
        else {
            return None;
        };
        let VertexAttributeValues::Float32x2(uvs) = mesh.attribute(Mesh::ATTRIBUTE_UV_0)? else {
            return None;
        };
        let colors = match mesh.attribute(Mesh::ATTRIBUTE_COLOR) {
            Some(VertexAttributeValues::Float32x4(colors)) => colors.clone(),
            _ => vec![[1.0; 4]; positions.len()],
        };
        Some(MeshBuilder {
            positions: positions.clone(),
            normals: normals.clone(),
            uvs: uvs.clone(),
            colors,
            indices: mesh.indices()?.iter().map(|index| index as u32).collect(),
            ..default()
        })
    }
}

/// Match the pack to the authored assets, then fall back to the same baker
/// for missing, modified, or older asset sets.
pub(super) fn load_or_bake(directory: &Path, sources: &Portraits) -> Atlas {
    load(&directory.join(ATLAS_PATH), sources).unwrap_or_else(|_| bake(sources))
}

pub(super) fn load(path: &Path, sources: &Portraits) -> io::Result<Atlas> {
    let reader = BufReader::new(File::open(path)?);
    let mut reader = GzDecoder::new(reader);
    let mut header = [0; 40];
    reader.read_exact(&mut header)?;
    if &header[..8] != MAGIC || &header[8..] != sources.fingerprint().as_bytes() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "tree impostor source mismatch",
        ));
    }
    let mut frames = [Framing {
        center: 0.0,
        radius: 0.0,
    }; TILES];
    for frame in &mut frames {
        let mut data = [0; 8];
        reader.read_exact(&mut data)?;
        frame.center = f32::from_le_bytes(data[..4].try_into().unwrap());
        frame.radius = f32::from_le_bytes(data[4..].try_into().unwrap());
        if !frame.center.is_finite() || !frame.radius.is_finite() || frame.radius <= 0.0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid tree framing",
            ));
        }
    }
    let mut colors = vec![0; TEXTURE_BYTES];
    let mut normals = vec![0; TEXTURE_BYTES];
    reader.read_exact(&mut colors)?;
    reader.read_exact(&mut normals)?;
    if reader.read(&mut [0])? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "oversized impostor pack",
        ));
    }
    Ok(Atlas {
        colors: array_image(colors, TextureFormat::Rgba8UnormSrgb),
        normals: array_image(normals, TextureFormat::Rgba8Unorm),
        frames,
    })
}

#[cfg(test)]
pub(super) fn bake_to(path: &Path, sources: &Portraits) -> io::Result<()> {
    use std::io::{BufWriter, Write};
    let atlas = bake(sources);
    let output = BufWriter::new(File::create(path)?);
    let mut writer = flate2::write::GzEncoder::new(output, flate2::Compression::default());
    writer.write_all(MAGIC)?;
    writer.write_all(sources.fingerprint().as_bytes())?;
    for frame in atlas.frames {
        writer.write_all(&frame.center.to_le_bytes())?;
        writer.write_all(&frame.radius.to_le_bytes())?;
    }
    writer.write_all(atlas.colors.data.as_ref().unwrap())?;
    writer.write_all(atlas.normals.data.as_ref().unwrap())?;
    writer.finish()?.flush()
}

pub(super) fn bake(authored: &Portraits) -> Atlas {
    let bark = Arc::new(textures::bark_texture());
    let sources: Vec<_> = Species::ALL
        .into_iter()
        .flat_map(|species| {
            let diffuse = Arc::new(textures::canopy_texture(species));
            let bark = Arc::clone(&bark);
            (0..species.variants()).map(move |variant| {
                authored.0[tile(species, variant)]
                    .clone()
                    .unwrap_or_else(|| {
                        Arc::new(Source::procedural(
                            species,
                            variant,
                            Arc::clone(&bark),
                            Arc::clone(&diffuse),
                        ))
                    })
            })
        })
        .collect();
    let frames = std::array::from_fn(|tile| sources[tile].frame);
    let width = SIDE * TILES;
    let mut colors = Vec::new();
    let mut normals = Vec::new();

    for view in 0..VIEWS {
        let mut albedo_layer = vec![[0; 4]; width * SIDE];
        let mut normal_layer = vec![[0; 4]; width * SIDE];
        for (tile, source) in sources.iter().enumerate() {
            let portrait = Portrait::render(source, view);
            for y in 0..SIDE {
                let start = y * width + tile * SIDE;
                let row = y * SIDE..(y + 1) * SIDE;
                albedo_layer[start..start + SIDE].copy_from_slice(&portrait.color[row.clone()]);
                normal_layer[start..start + SIDE].copy_from_slice(&portrait.normal[row]);
            }
        }
        let (mut albedo, mut normal, levels) =
            textures::impostor_mips(albedo_layer, normal_layer, width, SIDE, TILES);
        colors.append(&mut albedo);
        normals.append(&mut normal);
        debug_assert_eq!(levels, MIP_LEVELS);
    }
    Atlas {
        colors: array_image(colors, TextureFormat::Rgba8UnormSrgb),
        normals: array_image(normals, TextureFormat::Rgba8Unorm),
        frames,
    }
}

fn array_image(data: Vec<u8>, format: TextureFormat) -> Image {
    debug_assert_eq!(data.len(), TEXTURE_BYTES);
    let mut image = Image::new_fill(
        Extent3d {
            width: (SIDE * TILES) as u32,
            height: SIDE as u32,
            depth_or_array_layers: VIEWS as u32,
        },
        TextureDimension::D2,
        &[0; 4],
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..default()
    });
    image.texture_descriptor.mip_level_count = MIP_LEVELS;
    image.sampler = bevy::image::ImageSampler::linear();
    image
}

fn view_basis(view: usize) -> (Vec3, Vec3, Vec3) {
    if view == VIEWS - 1 {
        return (Vec3::X, Vec3::NEG_Z, Vec3::Y);
    }
    let (azimuth, count, elevation) = if view < HORIZON_VIEWS {
        (view, HORIZON_VIEWS, -4.0_f32)
    } else {
        (view - HORIZON_VIEWS, ELEVATED_VIEWS, 40.0)
    };
    let (sin, cos) = (TAU * azimuth as f32 / count as f32).sin_cos();
    let (sin_elevation, cos_elevation) = elevation.to_radians().sin_cos();
    let eye = Vec3::new(sin * cos_elevation, sin_elevation, cos * cos_elevation);
    let right = Vec3::new(cos, 0.0, -sin);
    (right, eye.cross(right), eye)
}

struct Portrait {
    color: Vec<[u8; 4]>,
    normal: Vec<[u8; 4]>,
    depth: Vec<f32>,
    right: Vec3,
    up: Vec3,
    eye: Vec3,
}

impl Portrait {
    fn render(source: &Source, view: usize) -> Self {
        let (right, up, eye) = view_basis(view);
        let mut portrait = Self {
            color: vec![[0; 4]; SIDE * SIDE],
            normal: vec![[0; 4]; SIDE * SIDE],
            depth: vec![f32::NEG_INFINITY; SIDE * SIDE],
            right,
            up,
            eye,
        };
        for part in &source.parts {
            portrait.draw(part, source.frame);
        }
        portrait
    }

    fn draw(&mut self, part: &Part, frame: Framing) {
        let mesh = &part.mesh;
        let vertices: Vec<_> = mesh
            .positions
            .iter()
            .map(|&p| {
                let point = Vec3::from_array(p) - Vec3::Y * frame.center;
                let pixel = Vec2::new(point.dot(self.right), -point.dot(self.up))
                    / (2.0 * frame.radius)
                    + 0.5;
                (pixel * SIDE as f32, point.dot(self.eye))
            })
            .collect();
        for triangle in mesh.indices.chunks_exact(3) {
            let indices: [usize; 3] = std::array::from_fn(|i| triangle[i] as usize);
            let [a, b, c] = indices.map(|i| vertices[i].0);
            let determinant = (b - a).perp_dot(c - a);
            let back_face = determinant > 0.0;
            if determinant.abs() < 0.001 || (back_face && !part.double_sided) {
                continue;
            }
            let min = a
                .min(b)
                .min(c)
                .floor()
                .clamp(Vec2::ZERO, Vec2::splat((SIDE - 1) as f32))
                .as_uvec2();
            let max = a
                .max(b)
                .max(c)
                .ceil()
                .clamp(Vec2::ZERO, Vec2::splat((SIDE - 1) as f32))
                .as_uvec2();
            let depths = indices.map(|i| vertices[i].1);
            let texture_uv = indices.map(|i| Vec2::from_array(mesh.uvs[i]));
            let colors = indices.map(|i| Vec4::from_array(mesh.colors[i]));
            let normals = indices.map(|i| Vec3::from_array(mesh.normals[i]));
            for y in min.y..=max.y {
                for x in min.x..=max.x {
                    let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) - a;
                    let v = p.perp_dot(c - a) / determinant;
                    let w = (b - a).perp_dot(p) / determinant;
                    let weights = [1.0 - v - w, v, w];
                    if weights.into_iter().any(|weight| weight < 0.0) {
                        continue;
                    }
                    let index = y as usize * SIDE + x as usize;
                    let depth = blend(depths, weights);
                    if depth < self.depth[index] {
                        continue;
                    }
                    let texel = sample(&part.diffuse, blend(texture_uv, weights), part.repeat);
                    let tint = blend(colors, weights) * part.tint;
                    if f32::from(texel[3]) / 255.0 * tint.w < part.cutoff {
                        continue;
                    }
                    self.color[index] = if tint == Vec4::ONE {
                        [texel[0], texel[1], texel[2], 255]
                    } else {
                        let albedo = LinearRgba::from(Srgba::rgb_u8(texel[0], texel[1], texel[2]))
                            .to_vec3()
                            * tint.xyz();
                        let albedo = Srgba::from(LinearRgba::rgb(albedo.x, albedo.y, albedo.z));
                        [byte(albedo.red), byte(albedo.green), byte(albedo.blue), 255]
                    };
                    let normal = blend(normals, weights).normalize_or_zero()
                        * if back_face { -1.0 } else { 1.0 };
                    let [u, v] = encode_normal(normal);
                    self.normal[index] = [
                        u,
                        v,
                        byte(part.transmission),
                        byte(depth / frame.radius * 0.5 + 0.5),
                    ];
                    self.depth[index] = depth;
                }
            }
        }
    }
}

// Octahedral normals leave two channels for transmission and signed depth.
fn encode_normal(normal: Vec3) -> [u8; 2] {
    let normal = normal / normal.abs().element_sum().max(1e-6);
    let mut oct = normal.xy();
    if normal.z < 0.0 {
        let sign = Vec2::new(
            if oct.x < 0.0 { -1.0 } else { 1.0 },
            if oct.y < 0.0 { -1.0 } else { 1.0 },
        );
        oct = (Vec2::ONE - oct.yx().abs()) * sign;
    }
    [byte(oct.x * 0.5 + 0.5), byte(oct.y * 0.5 + 0.5)]
}

fn byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn blend<T: std::ops::Mul<f32, Output = T> + std::ops::Add<Output = T>>(
    values: [T; 3],
    weights: [f32; 3],
) -> T {
    let [a, b, c] = values;
    a * weights[0] + b * weights[1] + c * weights[2]
}

fn sample(image: &Image, uv: Vec2, repeat: bool) -> [u8; 4] {
    let uv = if repeat { uv - uv.floor() } else { uv };
    let size = image.texture_descriptor.size;
    let x = (uv.x.clamp(0.0, 1.0) * (size.width - 1) as f32).round() as usize;
    let y = (uv.y.clamp(0.0, 1.0) * (size.height - 1) as f32).round() as usize;
    let i = (y * size.width as usize + x) * 4;
    image.data.as_ref().expect("CPU image bytes")[i..i + 4]
        .try_into()
        .unwrap()
}

#[cfg(test)]
fn silhouette_score(reference: &Source, model: &Source) -> (f32, f32) {
    let scores =
        (0..VIEWS).map(|view| {
            let reference = Portrait::render(reference, view);
            let model = Portrait::render(model, view);
            let (shared, combined) = reference.color.iter().zip(&model.color).fold(
                (0, 0),
                |(shared, combined), (a, b)| {
                    (
                        shared + u32::from(a[3] != 0 && b[3] != 0),
                        combined + u32::from(a[3] != 0 || b[3] != 0),
                    )
                },
            );
            shared as f32 / combined.max(1) as f32
        });
    let (sum, worst) = scores.fold((0.0_f32, 1.0_f32), |(sum, worst), score| {
        (sum + score, worst.min(score))
    });
    (sum / VIEWS as f32, worst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewpoints_share_framing_but_capture_different_silhouettes() {
        let bark = Arc::new(textures::bark_texture());
        for species in Species::ALL {
            let source = Source::procedural(
                species,
                0,
                Arc::clone(&bark),
                Arc::new(textures::canopy_texture(species)),
            );
            let silhouettes: Vec<Vec<_>> = (0..VIEWS)
                .map(|view| {
                    let portrait = Portrait::render(&source, view);
                    assert!(
                        portrait
                            .color
                            .iter()
                            .filter(|color| color[3] == 255)
                            .count()
                            > 50
                    );
                    assert!((0..SIDE).all(|i| {
                        [i, SIDE * (SIDE - 1) + i, i * SIDE, i * SIDE + SIDE - 1]
                            .into_iter()
                            .all(|pixel| portrait.color[pixel][3] == 0)
                    }));
                    portrait.color.iter().map(|color| color[3]).collect()
                })
                .collect();
            assert_ne!(silhouettes[0], silhouettes[2]);
            assert_ne!(silhouettes[0], silhouettes[VIEWS - 1]);
        }
    }
}

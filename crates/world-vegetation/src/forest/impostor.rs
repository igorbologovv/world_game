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

use super::{
    generate_tree, library, textures, MeshBuilder, Species, BROADLEAF_TRANSMISSION, FOLIAGE_CUTOFF,
    VARIANTS,
};

const SIDE: usize = 64;
const HORIZON_VIEWS: usize = 16;
const ELEVATED_VIEWS: usize = 8;
const VIEWS: usize = HORIZON_VIEWS + ELEVATED_VIEWS + 1;
const STOCK_TILES: usize = Species::Palm as usize * VARIANTS + Species::Palm.stock_variants();
const TILES: usize = STOCK_TILES + library::ASSETS.len();
const INSET: f32 = 0.022;
const MIP_LEVELS: u32 = 5;
const MAGIC: &[u8; 8] = b"TREEIMP2";
const TEXTURE_BYTES: usize = mip_bytes();
pub(super) const ATLAS_PATH: &str = "models/vegetation/tree-impostors-v2.gz";

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
    library::index(species, variant).map_or(species as usize * VARIANTS + variant, |index| {
        STOCK_TILES + index
    })
}

#[derive(Clone)]
pub(super) struct Portraits([Option<Arc<Source>>; TILES]);

impl Default for Portraits {
    fn default() -> Self {
        Self(std::array::from_fn(|_| None))
    }
}

impl Portraits {
    /// Bake shared point templates from the same material/mesh source
    /// used by the existing impostor baker. Missing authored assets
    /// deliberately keep their original mesh fallback until loaded.
    pub(super) fn point_variant(&self, species: Species, variant: usize) -> usize {
        if self.0[tile(species,variant)].is_some() {variant} else {variant % species.stock_variants()}
    }
    pub(super) fn point_samples(&self, species: Species, variant: usize) -> Option<Vec<super::point_forest::Sample>> {
        if let Some(source) = &self.0[tile(species, variant)] {
            return Some(source.point_samples());
        }
        if variant >= species.stock_variants() { return None; }
        let source = Source::procedural(species, variant,
            Arc::new(textures::bark_texture()),
            Arc::new(textures::canopy_texture(species)));
        Some(source.point_samples())
    }

    pub fn insert(&mut self, species: Species, variant: usize, source: Source) {
        self.0[tile(species, variant)] = Some(Arc::new(source));
    }

    fn fingerprint(&self) -> blake3::Hash {
        let mut hash = blake3::Hasher::new();
        hash.update(b"linear-albedo-tangent-normals-v1");
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
                if let Some(mapped) = &part.mapped_normals {
                    let size = mapped.texture.texture_descriptor.size;
                    hash.update(&size.width.to_le_bytes());
                    hash.update(&size.height.to_le_bytes());
                    let count = size.width as usize * size.height as usize * 4;
                    hash.update(&mapped.texture.data.as_ref().expect("CPU normal bytes")[..count]);
                    update(&mut hash, &mapped.tangents);
                }
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
    pub fn has_leaf_normal_map(&self, species: Species, variant: usize) -> bool {
        self.0[tile(species, variant)]
            .as_ref()
            .is_some_and(|source| source.parts[1].mapped_normals.is_some())
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
                mapped_normals: None,
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

impl Framing {
    pub fn from_parts(parts: [&MeshBuilder; 2]) -> Self {
        let positions = || parts.iter().flat_map(|part| &part.positions);
        let (min, max) = positions().fold((0.0_f32, 0.0_f32), |(lo, hi), p| {
            (lo.min(p[1]), hi.max(p[1]))
        });
        let center = (min + max) * 0.5;
        let radius = positions()
            .map(|&p| (Vec3::from_array(p) - Vec3::Y * center).length())
            .fold(0.0_f32, f32::max);
        Self {
            center,
            radius: radius / (1.0 - 2.0 * INSET),
        }
    }
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
    mapped_normals: Option<MappedNormals>,
    tint: Vec4,
    cutoff: f32,
    repeat: bool,
    double_sided: bool,
    transmission: f32,
}

struct MappedNormals {
    texture: Arc<Image>,
    tangents: Vec<[f32; 4]>,
}

impl Source {
    fn point_samples(&self) -> Vec<super::point_forest::Sample> {
        use super::point_forest::Sample;
        const MAX_POINTS: usize = 45_000;
        let area = |part: &Part, tri: &[u32]| {
            let [a,b,c] = [tri[0],tri[1],tri[2]].map(|i| Vec3::from_array(part.mesh.positions[i as usize]));
            (b-a).cross(c-a).length()*0.5
        };
        let total: f32 = self.parts.iter().map(|p| p.mesh.indices.chunks_exact(3).map(|t| area(p,t)).sum::<f32>()).sum();
        let spacing = (total / MAX_POINTS as f32).sqrt().max(0.055);
        let mut out = Vec::new();
        let mut rng = super::Random::new(0x243f6a8885a308d3);
        for (part_index, part) in self.parts.iter().enumerate() {
            for tri in part.mesh.indices.chunks_exact(3) {
                let indices = [tri[0],tri[1],tri[2]].map(|i| i as usize);
                let positions = indices.map(|i| Vec3::from_array(part.mesh.positions[i]));
                let normals = indices.map(|i| Vec3::from_array(part.mesh.normals[i]));
                let uvs = indices.map(|i| Vec2::from_array(part.mesh.uvs[i]));
                let colors = indices.map(|i| Vec4::from_array(part.mesh.colors[i]));
                let a = area(part,tri);
                let n = (a/(spacing*spacing)+rng.unit()).floor().min(4096.0) as usize;
                if n == 0 { continue; }
                let radius = (a/(n as f32*std::f32::consts::PI)).sqrt().mul_add(1.40,0.0).clamp(0.012,0.25);
                for _ in 0..n {
                    let u = rng.unit().sqrt();
                    let v = rng.unit();
                    let weights = [1.0-u,u*(1.0-v),u*v];
                    let uv = blend(uvs,weights);
                    let texel = sample(&part.diffuse,uv,part.repeat);
                    let tint = blend(colors,weights)*part.tint;
                    if f32::from(texel[3])/255.0*tint.w < part.cutoff { continue; }
                    let color = LinearRgba::from(Srgba::rgb_u8(texel[0],texel[1],texel[2])).to_vec3()*tint.xyz();
                    let mut normal = blend(normals,weights);
                    if let Some(mapped) = &part.mapped_normals {
                        let tangent = blend(indices.map(|i|Vec4::from_array(mapped.tangents[i])),weights);
                        let texel = sample(&mapped.texture,uv,part.repeat);
                        let n = Vec3::new(texel[0] as f32,texel[1] as f32,texel[2] as f32)*(2.0/255.0)-Vec3::ONE;
                        normal = tangent.xyz()*n.x + normal.cross(tangent.xyz())*tangent.w*n.y + normal*n.z;
                    }
                    out.push(Sample { position: blend(positions,weights), radius, color,
                        normal: normal.normalize_or_zero(), leaf: part_index != 0 });
                }
            }
        }
        out
    }

    pub fn new(parts: [Part; 2]) -> Self {
        let frame = Framing::from_parts([&parts[0].mesh, &parts[1].mesh]);
        Self { parts, frame }
    }

    pub fn frame(&self) -> Framing {
        self.frame
    }

    fn procedural(species: Species, variant: usize, bark: Arc<Image>, diffuse: Arc<Image>) -> Self {
        let (trunk, canopy) = generate_tree(species, variant as u64, 0);
        Self::new([
            Part {
                mesh: trunk,
                diffuse: bark,
                mapped_normals: None,
                tint: Vec4::ONE,
                cutoff: 0.0,
                repeat: true,
                double_sided: false,
                transmission: 0.0,
            },
            Part {
                mesh: canopy,
                diffuse,
                mapped_normals: None,
                tint: Vec4::ONE,
                cutoff: FOLIAGE_CUTOFF,
                repeat: false,
                double_sided: true,
                transmission: if species.broadleaf() {
                    BROADLEAF_TRANSMISSION
                } else {
                    0.34
                },
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
        normal_map: Option<&Image>,
    ) -> Option<Self> {
        Self::from_shared(
            mesh,
            Arc::new(diffuse.clone()),
            material,
            repeat,
            normal_map.cloned().map(Arc::new),
        )
    }

    pub fn from_shared(
        mesh: &Mesh,
        diffuse: Arc<Image>,
        material: &StandardMaterial,
        repeat: bool,
        normal_map: Option<Arc<Image>>,
    ) -> Option<Self> {
        if diffuse.texture_descriptor.format != TextureFormat::Rgba8UnormSrgb {
            return None;
        }
        Some(Self {
            mesh: Self::geometry(mesh)?,
            diffuse,
            mapped_normals: normal_map
                .filter(|map| map.texture_descriptor.format == TextureFormat::Rgba8Unorm)
                .and_then(|texture| match mesh.attribute(Mesh::ATTRIBUTE_TANGENT) {
                    Some(VertexAttributeValues::Float32x4(tangents)) => Some(MappedNormals {
                        texture,
                        tangents: tangents.clone(),
                    }),
                    _ => None,
                }),
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
pub(super) fn bake_to(path: &Path, coverage_path: &Path, sources: &Portraits) -> io::Result<()> {
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
    writer.finish()?.flush()?;

    let colors = atlas.colors.data.as_ref().unwrap();
    let zenith_begin = (VIEWS - 1) * (TEXTURE_BYTES / VIEWS);
    let zenith = &colors[zenith_begin..];
    let mut coverage = BufWriter::new(File::create(coverage_path)?);
    writeln!(coverage, "// Generated by the bake_tree_impostors test.\n[")?;
    for tile in STOCK_TILES..TILES {
        let occupied = (0..SIDE)
            .flat_map(|y| (0..SIDE).map(move |x| (y * SIDE * TILES + tile * SIDE + x) * 4))
            .filter(|&pixel| zenith[pixel + 3] as f32 / 255.0 >= 0.37)
            .count();
        let pixel_width = 2.0 * atlas.frames[tile].radius / SIDE as f32;
        writeln!(
            coverage,
            "    {:.4},",
            occupied as f32 * pixel_width * pixel_width
        )?;
    }
    writeln!(coverage, "]")?;
    coverage.flush()
}

pub(super) fn bake(authored: &Portraits) -> Atlas {
    let bark = Arc::new(textures::bark_texture());
    let stock = Species::ALL.into_iter().flat_map(|species| {
        let diffuse = Arc::new(textures::canopy_texture(species));
        let bark = Arc::clone(&bark);
        (0..species.stock_variants()).map(move |variant| {
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
    });
    let added = library::ASSETS.iter().enumerate().map(|(index, entry)| {
        authored.0[STOCK_TILES + index].clone().unwrap_or_else(|| {
            Arc::new(Source::procedural(
                entry.species,
                index % entry.species.stock_variants(),
                Arc::clone(&bark),
                Arc::new(textures::canopy_texture(entry.species)),
            ))
        })
    });
    let sources: Vec<_> = stock.chain(added).collect();
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
            let tangents = part
                .mapped_normals
                .as_ref()
                .map(|map| indices.map(|i| Vec4::from_array(map.tangents[i])));
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
                    let uv = blend(texture_uv, weights);
                    let texel = sample(&part.diffuse, uv, part.repeat);
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
                    let mut normal = blend(normals, weights);
                    if let Some((mapped, tangents)) = part.mapped_normals.as_ref().zip(tangents) {
                        // Use the same interpolated MikkTSpace basis as the mesh shader.
                        let tangent = blend(tangents, weights);
                        let bitangent = normal.cross(tangent.xyz()) * tangent.w;
                        let texel = sample(&mapped.texture, uv, part.repeat);
                        let local = Vec3::new(texel[0] as f32, texel[1] as f32, texel[2] as f32)
                            * (2.0 / 255.0)
                            - Vec3::ONE;
                        normal = tangent.xyz() * local.x + bitangent * local.y + normal * local.z;
                    }
                    let normal = if back_face { -normal } else { normal };
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
pub(super) fn encode_normal(normal: Vec3) -> [u8; 2] {
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

pub(super) fn decode_normal([x, y]: [u8; 2]) -> Vec3 {
    let mut normal = Vec3::new(x as f32, y as f32, 0.0) * (2.0 / 255.0) - Vec3::new(1.0, 1.0, 0.0);
    normal.z = 1.0 - normal.x.abs() - normal.y.abs();
    let fold = (-normal.z).max(0.0);
    normal.x -= fold.copysign(normal.x);
    normal.y -= fold.copysign(normal.y);
    normal.normalize()
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

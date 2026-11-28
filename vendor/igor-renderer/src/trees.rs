use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3, Vec4};
use std::{error::Error, path::Path};

use crate::tree_texture::{TextureAtlas, TextureRect};

/// Fine-cell-local disk center and radius, with a reference to its material
/// coordinates. Packing uses 0.244 mm position and 0.015 mm radius increments.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Surfel {
    xy: u32,
    z_radius: u32,
    pub(crate) patch: u32,
}

impl Surfel {
    fn new(center: Vec3, radius: f32, patch: u32) -> Self {
        let p = (center * 4096.0).round().as_uvec3();
        let radius = (radius * 65536.0).ceil() as u32;
        Self {
            xy: p.x | p.y << 16,
            z_radius: p.z | radius << 16,
            patch,
        }
    }

    pub(crate) fn center_radius(&self) -> (Vec3, f32) {
        let center = Vec3::new(
            (self.xy & 0xffff) as f32,
            (self.xy >> 16) as f32,
            (self.z_radius & 0xffff) as f32,
        ) / 4096.0;
        (center, (self.z_radius >> 16) as f32 / 65536.0)
    }
}

pub(crate) fn pack_normal(normal: Vec3) -> u32 {
    let n = normal / normal.abs().element_sum();
    let xy = if n.z >= 0.0 {
        n.truncate()
    } else {
        Vec2::new(1.0 - n.y.abs(), 1.0 - n.x.abs()) * n.truncate().signum()
    };
    let [x, y] = (xy * 32767.0)
        .round()
        .to_array()
        .map(|n| n as i16 as u16 as u32);
    x | y << 16
}

fn unpack_normal(packed: u32) -> Vec3 {
    let x = (packed as i16) as f32 / 32767.0;
    let y = ((packed >> 16) as i16) as f32 / 32767.0;
    let z = 1.0 - x.abs() - y.abs();
    let fold = (-z).max(0.0);
    Vec3::new(x - fold.copysign(x), y - fold.copysign(y), z).normalize()
}

/// Coplanar disks share source-triangle bounds, UV gradients, and vertex
/// normals. UVs are evaluated at the ray hit, including foliage alpha masks.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Patch {
    pub(crate) plane: [f32; 4],
    anchor: [f32; 3],
    pub(crate) material: u32,
    grad_u: [f32; 3],
    normal0: u32,
    grad_v: [f32; 3],
    normal1: u32,
    uv01: [f32; 4],
    uv2_normal_uv0: [f32; 4],
    normal_uv12: [f32; 4],
    normal2: u32,
    tangent: u32,
    bitangent: u32,
    has_tangent: u32,
}

impl Patch {
    pub(crate) fn coordinates(&self, pos: Vec3) -> Vec2 {
        let p = pos - Vec3::from(self.anchor);
        Vec2::new(
            p.dot(Vec3::from(self.grad_u)),
            p.dot(Vec3::from(self.grad_v)),
        )
    }

    fn base_uv(&self, b: Vec2) -> Vec2 {
        Vec2::from_slice(&self.uv01)
            + Vec2::from_slice(&self.uv01[2..]) * b.x
            + Vec2::from_slice(&self.uv2_normal_uv0) * b.y
    }

    fn normal_uv(&self, b: Vec2) -> Vec2 {
        Vec2::from_slice(&self.uv2_normal_uv0[2..])
            + Vec2::from_slice(&self.normal_uv12) * b.x
            + Vec2::from_slice(&self.normal_uv12[2..]) * b.y
    }

    fn shading_normal(&self, b: Vec2, mapped: Option<Vec3>) -> Vec3 {
        let n = (unpack_normal(self.normal0) * (1.0 - b.x - b.y)
            + unpack_normal(self.normal1) * b.x
            + unpack_normal(self.normal2) * b.y)
            .normalize_or(Vec3::from_slice(&self.plane));
        if let Some(mapped) = mapped {
            let t = unpack_normal(self.tangent);
            let t = (t - n * n.dot(t)).normalize_or_zero();
            let cross = n.cross(t);
            let bitangent = cross * cross.dot(unpack_normal(self.bitangent)).signum();
            (t * mapped.x + bitangent * mapped.y + n * mapped.z).normalize_or(n)
        } else {
            n
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Material {
    factor: [f32; 4],
    base: TextureRect,
    normal: TextureRect,
    params: [f32; 4], // alpha cutoff, normal scale
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub normal_uv: [f32; 2],
    pub material: u32,
}
pub struct MeshModel {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
    pub lo: Vec3,
    pub hi: Vec3,
}

#[derive(Default)]
pub(crate) struct TreeGeometry {
    pub mesh_models: Vec<MeshModel>,
    pub surfels: Vec<Surfel>,
    pub patches: Vec<Patch>,
    pub materials: Vec<Material>,
    textures: TextureAtlas,
}

#[derive(Clone, Copy)]
struct Vertex {
    pos: Vec3,
    normal: Vec3,
    uv: Vec2,
    normal_uv: Vec2,
}

struct Primitive {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    material: u32,
    foliage: bool,
}

impl TreeGeometry {
    pub(crate) fn load_model(&mut self, path: &Path) -> Result<f32, Box<dyn Error>> {
        let gltf = gltf::Gltf::open(path)?;
        let blob = gltf.blob.as_deref().ok_or("missing GLB geometry buffer")?;
        let scene = gltf
            .default_scene()
            .or_else(|| gltf.scenes().next())
            .ok_or("missing tree scene")?;
        let mut primitives = Vec::new();
        for node in scene.nodes() {
            self.load_node(path, blob, node, Mat4::IDENTITY, &mut primitives)?;
        }
        self.add_model(primitives)
    }

    pub(crate) fn add_mesh(&mut self, parts: Vec<crate::MeshPart>) -> Result<f32, Box<dyn Error>> {
        let mut primitives = Vec::with_capacity(parts.len());
        for part in parts {
            let material = self.materials.len() as u32;
            let base = self.textures.add_pixels(
                part.color,
                [
                    gltf::texture::WrappingMode::Repeat.into(),
                    gltf::texture::WrappingMode::Repeat.into(),
                ],
            )?;
            self.materials.push(Material {
                factor: part.factor,
                base,
                normal: [0; 4],
                params: [part.alpha_cutoff.unwrap_or(-1.0), 1.0, 0.0, 0.0],
            });
            let vertices = part
                .positions
                .into_iter()
                .zip(part.normals)
                .zip(part.uvs)
                .map(|((position, normal), uv)| Vertex {
                    pos: position.into(),
                    normal: normal.into(),
                    uv: uv.into(),
                    normal_uv: Vec2::ZERO,
                })
                .collect();
            primitives.push(Primitive {
                vertices,
                indices: part.indices,
                material,
                foliage: part.alpha_cutoff.is_some(),
            });
        }
        self.add_model(primitives)
    }

    fn add_model(&mut self, primitives: Vec<Primitive>) -> Result<f32, Box<dyn Error>> {
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for v in primitives.iter().flat_map(|p| &p.vertices) {
            lo = lo.min(v.pos);
            hi = hi.max(v.pos);
        }

        if !lo.is_finite() || !hi.is_finite() || lo == hi {
            return Err("empty tree geometry".into());
        }

        // Preserve proportions and make room for every disk inside its owner cell.
        let scale = (15.52 / (hi - lo).max_element()).min(1.0);
        let center = (lo + hi) * 0.5;
        let root = Vec3::new(center.x, lo.y, center.z);
        // Keep the original indexed GLB geometry, with the same coordinate
        // normalization and material IDs as the surfel path.
        let mut mesh = MeshModel {
            vertices: Vec::new(),
            indices: Vec::new(),
            lo: Vec3::splat(f32::INFINITY),
            hi: Vec3::splat(f32::NEG_INFINITY),
        };
        for primitive in &primitives {
            let first = u32::try_from(mesh.vertices.len())?;
            for v in &primitive.vertices {
                let position = (v.pos - root) * scale + Vec3::new(8.0, 0.0, 8.0);
                mesh.lo = mesh.lo.min(position);
                mesh.hi = mesh.hi.max(position);
                mesh.vertices.push(MeshVertex {
                    position: position.to_array(),
                    normal: v.normal.to_array(),
                    uv: v.uv.to_array(),
                    normal_uv: v.normal_uv.to_array(),
                    material: primitive.material,
                });
            }
            mesh.indices
                .extend(primitive.indices.iter().map(|i| first + *i));
        }
        self.mesh_models.push(mesh);
        let mut scratch = Vec::new();
        for primitive in primitives {
            let edge_limit = if primitive.foliage { 0.28 } else { 0.22 };
            for triangle in primitive.indices.chunks_exact(3) {
                let v =
                    [triangle[0], triangle[1], triangle[2]].map(|i| primitive.vertices[i as usize]);
                let points = v.map(|v| (v.pos - root) * scale + Vec3::new(8.0, 0.0, 8.0));
                self.add_triangle(v, points, primitive.material, edge_limit, &mut scratch);
            }
        }
        Ok(scale)
    }

    fn load_node(
        &mut self,
        path: &Path,
        blob: &[u8],
        node: gltf::Node<'_>,
        parent: Mat4,
        primitives: &mut Vec<Primitive>,
    ) -> Result<(), Box<dyn Error>> {
        let transform = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        let normal_matrix = transform.inverse().transpose();
        if let Some(mesh) = node.mesh() {
            for primitive in mesh.primitives() {
                if primitive.mode() != gltf::mesh::Mode::Triangles {
                    return Err("tree primitives must be triangles".into());
                }
                let reader = primitive.reader(|buffer| match buffer.source() {
                    gltf::buffer::Source::Bin => Some(blob),
                    _ => None,
                });
                let gltf_mat = primitive.material();
                let pbr = gltf_mat.pbr_metallic_roughness();
                let base = pbr.base_color_texture();
                let normal = gltf_mat.normal_texture();
                let alpha_cutoff = if gltf_mat.alpha_mode() == gltf::material::AlphaMode::Opaque {
                    -1.0
                } else {
                    gltf_mat.alpha_cutoff().unwrap_or(0.5)
                };
                let read_uv = |set: Option<u32>| -> Result<Vec<Vec2>, Box<dyn Error>> {
                    match set {
                        Some(set) => Ok(reader
                            .read_tex_coords(set)
                            .ok_or("missing material UVs")?
                            .into_f32()
                            .map(Vec2::from)
                            .collect()),
                        None => Ok(Vec::new()),
                    }
                };
                let uv = read_uv(base.as_ref().map(|t| t.tex_coord()))?;
                let normal_uv = read_uv(normal.as_ref().map(|t| t.tex_coord()))?;
                let positions = reader.read_positions().ok_or("missing tree positions")?;
                let normals = reader.read_normals().ok_or("missing tree normals")?;
                if positions.len() != normals.len()
                    || (!uv.is_empty() && uv.len() != positions.len())
                    || (!normal_uv.is_empty() && normal_uv.len() != positions.len())
                {
                    return Err("tree attribute counts differ".into());
                }
                let vertices: Vec<_> = positions
                    .zip(normals)
                    .enumerate()
                    .map(|(i, (p, n))| Vertex {
                        pos: transform.transform_point3(Vec3::from(p)),
                        normal: normal_matrix
                            .transform_vector3(Vec3::from(n))
                            .normalize_or(Vec3::Y),
                        uv: uv.get(i).copied().unwrap_or_default(),
                        normal_uv: normal_uv.get(i).copied().unwrap_or_default(),
                    })
                    .collect();
                let indices: Vec<_> = match reader.read_indices() {
                    Some(indices) => indices.into_u32().collect(),
                    None => (0..vertices.len() as u32).collect(),
                };
                if indices.len() % 3 != 0 || indices.iter().any(|&i| i as usize >= vertices.len()) {
                    return Err("invalid tree triangle indices".into());
                }
                let mut material = Material {
                    factor: pbr.base_color_factor(),
                    base: [0; 4],
                    normal: [0; 4],
                    params: [alpha_cutoff, 1.0, 0.0, 0.0],
                };
                if let Some(base) = base {
                    material.base = self.textures.add(path, blob, base.texture())?;
                }
                if let Some(normal) = normal {
                    material.normal = self.textures.add(path, blob, normal.texture())?;
                    material.params[1] = normal.scale();
                }
                primitives.push(Primitive {
                    vertices,
                    indices,
                    material: self.materials.len() as u32,
                    foliage: alpha_cutoff >= 0.0,
                });
                self.materials.push(material);
            }
        }
        for child in node.children() {
            self.load_node(path, blob, child, transform, primitives)?;
        }
        Ok(())
    }

    fn add_triangle(
        &mut self,
        v: [Vertex; 3],
        p: [Vec3; 3],
        material: u32,
        edge_limit: f32,
        stack: &mut Vec<[Vec3; 3]>,
    ) {
        let e1 = p[1] - p[0];
        let e2 = p[2] - p[0];
        let cross = e1.cross(e2);
        let area = cross.length();
        if area <= 1e-10 {
            return;
        }
        let n = cross / area;
        let plane = if n.dot(v[0].normal + v[1].normal + v[2].normal) < 0.0 {
            -n
        } else {
            n
        };
        let uv1 = v[1].uv - v[0].uv;
        let uv2 = v[2].uv - v[0].uv;
        let nuv1 = v[1].normal_uv - v[0].normal_uv;
        let nuv2 = v[2].normal_uv - v[0].normal_uv;
        let sign = nuv1.perp_dot(nuv2).signum();
        let tangent = (e1 * nuv2.y - e2 * nuv1.y).normalize_or_zero() * sign;
        let bitangent = (e2 * nuv1.x - e1 * nuv2.x).normalize_or_zero() * sign;
        let has_tangent = tangent != Vec3::ZERO && bitangent != Vec3::ZERO;
        let patch_id = self.patches.len() as u32;
        self.patches.push(Patch {
            plane: plane.extend(plane.dot(p[0])).to_array(),
            anchor: p[0].to_array(),
            material,
            grad_u: (e2.cross(n) / area).to_array(),
            normal0: pack_normal(v[0].normal),
            grad_v: (n.cross(e1) / area).to_array(),
            normal1: pack_normal(v[1].normal),
            uv01: [v[0].uv.x, v[0].uv.y, uv1.x, uv1.y],
            uv2_normal_uv0: [uv2.x, uv2.y, v[0].normal_uv.x, v[0].normal_uv.y],
            normal_uv12: [nuv1.x, nuv1.y, nuv2.x, nuv2.y],
            normal2: pack_normal(v[2].normal),
            tangent: if has_tangent { pack_normal(tangent) } else { 0 },
            bitangent: if has_tangent {
                pack_normal(bitangent)
            } else {
                0
            },
            has_tangent: has_tangent as u32,
        });
        stack.push(p);
        while let Some(p) = stack.pop() {
            let [a, b, c] = p;
            let edges = [
                (b - a).length_squared(),
                (c - b).length_squared(),
                (a - c).length_squared(),
            ];
            let mut first = 0;
            for i in 1..3 {
                if edges[i] > edges[first] {
                    first = i;
                }
            }
            if edges[first] > edge_limit * edge_limit {
                let next = (first + 1) % 3;
                let midpoint = p[first].lerp(p[next], 0.5);
                let mut left = p;
                let mut right = p;
                left[first] = midpoint;
                right[next] = midpoint;
                stack.push(left);
                stack.push(right);
            } else {
                let center = (a + b + c) / 3.0;
                // A disk covers its subtriangle. Source bounds clip disk overlap
                // at the model edges, including along the smallest branch tips.
                let radius = (a - center)
                    .length()
                    .max((b - center).length())
                    .max((c - center).length());
                self.surfels
                    .push(Surfel::new(center, radius + 0.0006, patch_id));
            }
        }
    }

    pub(crate) fn alpha_visible(&self, patch: &Patch, b: Vec2) -> bool {
        let mat = &self.materials[patch.material as usize];
        if mat.params[0] < 0.0 {
            return true;
        }
        let alpha = if mat.base[0] == 0 {
            1.0
        } else {
            self.textures.sample(mat.base, patch.base_uv(b), false).w
        };
        alpha * mat.factor[3] >= mat.params[0]
    }

    pub(crate) fn surface(&self, patch: &Patch, b: Vec2) -> (Vec3, Vec3) {
        let mat = &self.materials[patch.material as usize];
        let base = if mat.base[0] == 0 {
            Vec4::ONE
        } else {
            self.textures.sample(mat.base, patch.base_uv(b), true)
        };
        let mapped = (patch.has_tangent != 0 && mat.normal[0] != 0).then(|| {
            let normal = self
                .textures
                .sample(mat.normal, patch.normal_uv(b), false)
                .truncate()
                * 2.0
                - Vec3::ONE;
            normal * Vec3::new(mat.params[1], mat.params[1], 1.0)
        });
        (
            (base * Vec4::from(mat.factor)).truncate(),
            patch.shading_normal(b, mapped),
        )
    }

    /// CPU material sampling for offline geometry LOD generation. Uses the
    /// same atlas, wrapping and alpha cutoff as the original mesh.
    pub fn sample_mesh_material(&self, id: u32, uv: Vec2) -> (Vec4, bool, bool) {
        let m = &self.materials[id as usize];
        let mut c = Vec4::from_array(m.factor);
        if m.base[0] != 0 {
            c *= self.textures.sample(m.base, uv, true);
        }
        let leaf = m.params[0] >= 0.0;
        (c, leaf, !leaf || c.w >= m.params[0])
    }

    pub fn upload_textures(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> [wgpu::TextureView; 2] {
        self.textures.upload(device, queue)
    }
}

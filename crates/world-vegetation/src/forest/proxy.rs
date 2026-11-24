//! Merged impostor and parent canopy meshes built from the same canonical sites.

// Bevy 0.14's ShaderType derive emits unused layout-check functions.
#![allow(dead_code)]
use bevy::{
    asset::RenderAssetUsages,
    camera::primitives::Aabb,
    math::DVec3,
    pbr::MaterialPipeline,
    prelude::*,
    reflect::TypePath,
    render::{
        mesh::MeshVertexBufferLayoutRef,
        render_resource::{
            AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
        },
    },
    shader::{ShaderDefVal, ShaderRef},
};

use super::{impostor, placement::Site, MeshBuilder, Species, BROADLEAF_TRANSMISSION};

pub(super) const CHILD_SLOTS: usize = 24;
#[derive(Clone, Copy, ShaderType)]
pub(super) struct ProxyUniform {
    // Leaf coverage, near readiness, near fade start/end.
    pub leaf: Vec4,
    // Forest fade start/end, arrival coverage, parent mode.
    pub forest: Vec4,
    pub impostor: Vec4,
    pub children: [Vec4; CHILD_SLOTS / 4],
}

#[derive(Clone, Asset, TypePath, AsBindGroup)]
pub(super) struct ProxyMaterial {
    #[uniform(0)]
    pub coverage: ProxyUniform,
    #[texture(1, dimension = "2d_array")]
    #[sampler(2)]
    pub atlas: Handle<Image>,
    #[texture(3, dimension = "2d_array")]
    pub normals: Handle<Image>,
}

impl Material for ProxyMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/vegetation_proxy.wesl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/vegetation_proxy.wesl".into()
    }
    fn specialize(
        _: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.vertex.buffers = vec![layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            Mesh::ATTRIBUTE_NORMAL.at_shader_location(1),
            Mesh::ATTRIBUTE_UV_0.at_shader_location(2),
            Mesh::ATTRIBUTE_COLOR.at_shader_location(3),
            Mesh::ATTRIBUTE_TANGENT.at_shader_location(4),
        ])?];
        let defs = [
            ShaderDefVal::Bool("STANDARD_MATERIAL_DIFFUSE_TRANSMISSION".into(), true),
            ShaderDefVal::UInt(
                "BROADLEAF_TRANSMISSION_BYTE".into(),
                (BROADLEAF_TRANSMISSION * 255.0).round() as u32,
            ),
        ];
        descriptor.vertex.shader_defs.extend(defs.clone());
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader_defs.extend(defs);
        }

        Ok(())
    }
}

#[derive(Default)]
pub(super) struct ProxyMesh {
    geometry: MeshBuilder,
    sites: Vec<[f32; 4]>,
    bounds: Option<(Vec3, Vec3)>,
}

pub(super) struct ProxyGeometry {
    pub mesh: Mesh,
    pub bounds: Aabb,
}

impl ProxyMesh {
    pub fn add(&mut self, site: Site, anchor: DVec3, slot: usize, frame: impostor::Framing) {
        let root = (site.position - anchor).as_vec3();
        let rotation = site.rotation();
        let up = rotation * Vec3::Y;
        let forward = rotation * Vec3::Z;
        let center = root + up * (site.scale * frame.center);
        let radius = frame.radius * site.scale;
        let extent = Vec3::splat(std::f32::consts::SQRT_2 * radius);
        let min = center - extent;
        let max = center + extent;
        self.bounds = Some(
            self.bounds
                .map_or((min, max), |(lo, hi)| (min.min(lo), max.max(hi))),
        );
        let start = self.geometry.positions.len() as u32;
        let tile = impostor::tile(site.species, site.variant) as f32;
        for (x, y) in [(0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)] {
            self.geometry.vertex(
                center,
                up,
                Vec2::new(
                    tile + 0.25 + x * 0.5,
                    y + if site.species.broadleaf() { 2.0 } else { 0.0 },
                ),
                forward.extend(radius).to_array(),
            );
        }
        self.geometry.triangle(start, start + 1, start + 2);
        self.geometry.triangle(start, start + 2, start + 3);
        self.sites.resize(
            self.geometry.positions.len(),
            root.extend(slot as f32).to_array(),
        );
    }

    pub fn finish(self) -> Option<ProxyGeometry> {
        let (min, max) = self.bounds?;
        let mut mesh = self.geometry.finish();
        mesh.asset_usage = RenderAssetUsages::RENDER_WORLD;
        mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, self.sites);
        Some(ProxyGeometry {
            mesh,
            bounds: Aabb::from_min_max(min, max),
        })
    }
}

pub(super) fn half_width(species: Species, variant: usize) -> f32 {
    if let Some(entry) = super::library::asset(species, variant) {
        return entry.width;
    }
    match (species, variant) {
        (Species::Oak, _) => 0.37,
        (Species::Pine, _) => 0.24,
        (Species::Acacia, _) => 0.50,
        (Species::Jungle, 1) => 0.65,
        (Species::Jungle, _) => 0.32,
        (Species::Palm, _) => 0.53,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use world_core::planet::RADIUS;

    #[test]
    fn one_site_feeds_both_batches_at_the_same_root() {
        let anchor = DVec3::Y * RADIUS;
        let site = Site {
            position: anchor + DVec3::Y * 300.0,
            species: Species::Oak,
            variant: 1,
            scale: 1.0,
            yaw: 2.0,
        };
        let mut leaf = ProxyMesh::default();
        let mut parent = ProxyMesh::default();
        let frame = impostor::Framing {
            center: 8.0,
            radius: 9.0,
        };
        leaf.add(site, anchor, 0, frame);
        parent.add(site, anchor, 5, frame);
        assert_eq!(leaf.sites[0], [0.0, 300.0, 0.0, 0.0]);
        assert_eq!(parent.sites[0], [0.0, 300.0, 0.0, 5.0]);
        assert_eq!(leaf.geometry.positions, parent.geometry.positions);
        assert_eq!(leaf.geometry.uvs, parent.geometry.uvs);
        assert_eq!(leaf.finish().unwrap().mesh.indices().unwrap().len(), 6);
        assert_eq!(parent.finish().unwrap().mesh.indices().unwrap().len(), 6);
    }
}

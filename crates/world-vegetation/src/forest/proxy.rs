//! Merged impostor and parent canopy meshes built from the same canonical sites.

// Bevy 0.14's ShaderType derive emits unused layout-check functions.
#![allow(dead_code)]
use bevy::{
    math::DVec3,
    pbr::MaterialPipeline,
    prelude::*,
    shader::ShaderRef,
    asset::RenderAssetUsages,
    reflect::TypePath,
    render::{
        mesh::MeshVertexBufferLayoutRef,

        render_resource::{
            AsBindGroup, RenderPipelineDescriptor, ShaderType,
            SpecializedMeshPipelineError,
        },
    },
};

use super::{impostor, placement::Site, MeshBuilder, Species};

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
    #[texture(1)]
    #[sampler(2)]
    pub atlas: Handle<Image>,
}

impl Material for ProxyMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/vegetation_proxy.wgsl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/vegetation_proxy.wgsl".into()
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

        Ok(())
    }
}

#[derive(Default)]
pub(super) struct ProxyMesh {
    geometry: MeshBuilder,
    sites: Vec<[f32; 4]>,
}

impl ProxyMesh {
    pub fn add(&mut self, site: Site, anchor: DVec3, slot: usize) {
        let root = (site.position - anchor).as_vec3();
        let height = site.height();
        let transform = Transform::from_translation(root)
            .with_rotation(site.rotation())
            .with_scale(Vec3::splat(height));
        let start = self.geometry.positions.len() as u32;
        let up = transform.up().into();
        let half_width = half_width(site.species) * height * 1.16;
        for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            let side = (x * 2.0 - 1.0) * half_width;
            let position = transform.transform_point(Vec3::Y * y * 1.07) + Vec3::X * side;
            let uv = impostor::uv(site.species, site.variant, Vec2::new(x, 1.0 - y));
            self.geometry
                .vertex(position, up, uv, [1.0, 1.0, 1.0, side]);
        }
        self.geometry.triangle(start, start + 1, start + 2);
        self.geometry.triangle(start, start + 2, start + 3);
        self.sites.resize(
            self.geometry.positions.len(),
            root.extend(slot as f32).to_array(),
        );
    }

    pub fn finish(self) -> Option<Mesh> {
        if self.sites.is_empty() {
            return None;
        }
        let mut mesh = self.geometry.finish();
        mesh.asset_usage = RenderAssetUsages::RENDER_WORLD;
        mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, self.sites);
        Some(mesh)
    }
}

pub(super) fn half_width(species: Species) -> f32 {
    match species {
        Species::Oak => 0.37,
        Species::Pine => 0.24,
        Species::Acacia => 0.50,
        Species::Jungle => 0.32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_site_feeds_both_batches_at_the_same_root() {
        let site = Site {
            position: DVec3::Y * 100_300.0,
            species: Species::Oak,
            variant: 1,
            scale: 1.0,
            yaw: 2.0,
        };
        let mut leaf = ProxyMesh::default();
        let mut parent = ProxyMesh::default();
        leaf.add(site, DVec3::Y * 100_000.0, 0);
        parent.add(site, DVec3::Y * 100_000.0, 5);
        assert_eq!(leaf.sites[0], [0.0, 300.0, 0.0, 0.0]);
        assert_eq!(parent.sites[0], [0.0, 300.0, 0.0, 5.0]);
        assert_eq!(leaf.geometry.positions, parent.geometry.positions);
        assert_eq!(leaf.geometry.uvs, parent.geometry.uvs);
        assert_eq!(leaf.finish().unwrap().indices().unwrap().len(), 6);
        assert_eq!(parent.finish().unwrap().indices().unwrap().len(), 6);
    }
}

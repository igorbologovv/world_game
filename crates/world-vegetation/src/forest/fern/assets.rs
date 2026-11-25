//! Load and validate the supplied fern GLBs once.
use bevy::{
    asset::RenderAssetUsages,
    asset::{LoadState, RecursiveDependencyLoadState},
    camera::primitives::{Aabb, MeshAabb},
    gltf::{Gltf, GltfMaterial, GltfMesh, GltfNode},
    mesh::{PrimitiveTopology, VertexAttributeValues},
    pbr::gltf::standard_material_from_gltf_material,
    prelude::*,
};

use crate::forest::{textures, MeshBuilder};

const DIRECTORY: &str = "models/vegetation/woodland_fern";

fn path(level: usize) -> String {
    format!("{DIRECTORY}/woodland_fern_lod{level}.glb")
}

#[derive(Resource)]
pub(super) enum FernAssets {
    Loading([Handle<Gltf>; 4]),
    Ready(Box<Models>),
    Failed,
}

impl FernAssets {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Loading(_) => "loading",
            Self::Ready(_) => "ready",
            Self::Failed => "asset error",
        }
    }
}

pub(super) struct Models {
    pub detail: [(Handle<Mesh>, Aabb); 3],
    pub detail_material: StandardMaterial,
    pub cards: MeshBuilder,
    pub card_material: StandardMaterial,
}

pub(super) fn request(mut commands: Commands, server: Res<AssetServer>) {
    commands.insert_resource(FernAssets::Loading(std::array::from_fn(|lod| {
        server.load(path(lod))
    })));
}

pub(super) fn finish(
    server: Res<AssetServer>,
    gltfs: Res<Assets<Gltf>>,
    gltf_meshes: Res<Assets<GltfMesh>>,
    gltf_nodes: Res<Assets<GltfNode>>,
    materials: Res<Assets<GltfMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut fern: ResMut<FernAssets>,
) {
    let FernAssets::Loading(files) = &*fern else {
        return;
    };
    if let Some((level, handle)) = files.iter().enumerate().find(|(_, handle)| {
        matches!(server.load_state(handle.id()), LoadState::Failed(_))
            || matches!(
                server.get_recursive_dependency_load_state(handle.id()),
                Some(RecursiveDependencyLoadState::Failed(_))
            )
    }) {
        warn!(
            "Woodland Fern failed to load {} ({:?} and dependencies {:?})",
            path(level),
            server.load_state(handle.id()),
            server.get_recursive_dependency_load_state(handle.id())
        );
        *fern = FernAssets::Failed;
        return;
    }
    if !files
        .iter()
        .all(|file| server.is_loaded_with_dependencies(file.id()))
    {
        return;
    }

    let loaded = (|| -> Result<Models, String> {
        let mut sources = Vec::with_capacity(4);
        for (level, handle) in files.iter().enumerate() {
            let gltf = gltfs
                .get(handle)
                .ok_or_else(|| format!("{} has no glTF asset", path(level)))?;
            let source = combine(level, gltf, &gltf_meshes, &gltf_nodes, &meshes, &materials)
                .map_err(|reason| format!("{} {reason}", path(level)))?;
            sources.push(source);
        }
        let (cards, mut card_material) = sources.pop().unwrap();
        let cutoff = match card_material.alpha_mode {
            AlphaMode::Mask(value) => value,
            _ => return Err("Woodland Fern LOD3 requires its alpha-mask material".into()),
        };
        let image = card_material
            .base_color_texture
            .as_ref()
            .filter(|handle| images.get(*handle).is_some())
            .ok_or("Woodland Fern LOD3 is missing its embedded color/alpha image")?;
        card_material.base_color_texture = Some(textures::import_mipmapped(
            &mut images,
            image,
            Some(cutoff),
            false,
        ));
        let detail_material = sources[0].1.clone();
        if !sources
            .iter()
            .all(|(_, mat)| same_surface(&detail_material, mat))
        {
            return Err(
                "Woodland Fern LOD0-2 have different PBR surfaces and cannot share a material"
                    .into(),
            );
        }
        let mut detail = Vec::with_capacity(3);
        for (geometry, _) in sources {
            let mut mesh = geometry.finish();
            mesh.asset_usage = RenderAssetUsages::RENDER_WORLD;
            let bounds = mesh.get_aabb().ok_or("Woodland Fern mesh has no bounds")?;
            detail.push((meshes.add(mesh), bounds));
        }
        Ok(Models {
            detail: detail.try_into().ok().expect("three detailed LODs"),
            detail_material,
            cards,
            card_material,
        })
    })();
    *fern = match loaded {
        Ok(models) => FernAssets::Ready(Box::new(models)),
        Err(error) => {
            warn!("Woodland Fern asset error: {error}");
            FernAssets::Failed
        }
    };
}

fn same_surface(a: &StandardMaterial, b: &StandardMaterial) -> bool {
    a.base_color == b.base_color
        && a.base_color_texture == b.base_color_texture
        && a.perceptual_roughness == b.perceptual_roughness
        && a.metallic == b.metallic
        && a.metallic_roughness_texture == b.metallic_roughness_texture
        && a.normal_map_texture == b.normal_map_texture
        && a.occlusion_texture == b.occlusion_texture
        && a.emissive == b.emissive
        && a.emissive_texture == b.emissive_texture
        && a.reflectance == b.reflectance
        && a.diffuse_transmission == b.diffuse_transmission
        && a.unlit == b.unlit
        && a.alpha_mode == b.alpha_mode
        && a.double_sided == b.double_sided
        && a.cull_mode == b.cull_mode
}

fn combine(
    level: usize,
    gltf: &Gltf,
    gltf_meshes: &Assets<GltfMesh>,
    gltf_nodes: &Assets<GltfNode>,
    meshes: &Assets<Mesh>,
    materials: &Assets<GltfMaterial>,
) -> Result<(MeshBuilder, StandardMaterial), &'static str> {
    let cards = level == 3;
    if gltf.meshes.len() != 2
        || gltf.nodes.iter().any(|node| {
            gltf_nodes
                .get(node)
                .is_none_or(|node| node.transform != Transform::IDENTITY)
        })
    {
        return Err("expected two meshes at their exported identity transforms");
    }
    let mut merged = MeshBuilder::default();
    let mut shared = None;
    for mesh in &gltf.meshes {
        let gltf_mesh = gltf_meshes.get(mesh).ok_or("missing its mesh metadata")?;
        let [primitive] = gltf_mesh.primitives.as_slice() else {
            return Err("expected one primitive per part");
        };
        let material = primitive
            .material
            .as_ref()
            .and_then(|mat| materials.get(mat))
            .map(standard_material_from_gltf_material)
            .ok_or("missing its PBR material")?;
        if !material.double_sided
            || material.cull_mode.is_some()
            || (!cards
                && (material.alpha_mode != AlphaMode::Opaque
                    || material.base_color_texture.is_some()))
        {
            return Err("has an incompatible vertex-color or double-sided material");
        }
        if let Some(previous) = shared.as_ref() {
            if !same_surface(previous, &material) {
                return Err("parts require different PBR materials");
            }
        }
        shared = Some(material);
        append(
            &mut merged,
            meshes.get(&primitive.mesh).ok_or("missing its primitive")?,
            cards,
        )?;
    }
    Ok((merged, shared.ok_or("has no material")?))
}

fn append(builder: &mut MeshBuilder, source: &Mesh, cards: bool) -> Result<(), &'static str> {
    let Some(VertexAttributeValues::Float32x3(pos)) = source.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return Err("missing float positions");
    };
    let Some(VertexAttributeValues::Float32x3(normal)) = source.attribute(Mesh::ATTRIBUTE_NORMAL)
    else {
        return Err("missing float normals");
    };
    let tint = match source.attribute(Mesh::ATTRIBUTE_COLOR) {
        Some(VertexAttributeValues::Float32x4(values)) if values.len() == pos.len() => Some(values),
        _ if cards => None,
        _ => return Err("missing RGBA vertex colors"),
    };
    let uv = match source.attribute(Mesh::ATTRIBUTE_UV_0) {
        Some(VertexAttributeValues::Float32x2(values)) if values.len() == pos.len() => Some(values),
        _ if !cards => None,
        _ => return Err("missing impostor texture coordinates"),
    };
    let indices = source.indices().ok_or("expected indexed fern triangles")?;
    if source.primitive_topology() != PrimitiveTopology::TriangleList
        || normal.len() != pos.len()
        || pos.is_empty()
        || indices.is_empty()
        || indices.len() % 3 != 0
        || indices.iter().any(|index| index >= pos.len())
        || pos
            .iter()
            .chain(normal)
            .any(|v| !Vec3::from_array(*v).is_finite())
    {
        return Err("has inconsistent indexed triangle data");
    }

    let base = builder.positions.len() as u32;
    builder.positions.extend(pos);
    builder.normals.extend(normal);
    builder
        .uvs
        .extend((0..pos.len()).map(|i| uv.map_or([0.0; 2], |uv| uv[i])));
    builder
        .colors
        .extend((0..pos.len()).map(|i| tint.map_or([1.0; 4], |tint| tint[i])));
    builder
        .indices
        .extend(indices.iter().map(|i| base + i as u32));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        asset::AssetPlugin,
        gltf::GltfPlugin,
        image::{CompressedImageFormats, ImageLoader, ImagePlugin},
        scene::ScenePlugin,
        world_serialization::WorldSerializationPlugin,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn all_four_shipped_exports_load_and_keep_their_triangles_and_cutout() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin {
                file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into(),
                ..default()
            },
            WorldSerializationPlugin,
            ScenePlugin,
            ImagePlugin::default(),
            GltfPlugin::default(),
        ))
        .register_asset_loader(ImageLoader::new(CompressedImageFormats::empty()))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .add_systems(Startup, request)
        .add_systems(Update, finish);
        app.finish();
        app.cleanup();

        let timeout = Instant::now() + Duration::from_secs(5);
        loop {
            app.update();
            match app.world().resource::<FernAssets>() {
                FernAssets::Ready(models) => {
                    let meshes = app.world().resource::<Assets<Mesh>>();
                    for ((mesh, bounds), count) in models.detail.iter().zip([33_600, 5_544, 1_280])
                    {
                        let mesh = meshes.get(mesh).unwrap();
                        assert_eq!(mesh.indices().unwrap().len() / 3, count);
                        assert!(mesh.contains_attribute(Mesh::ATTRIBUTE_COLOR));
                        assert!(bounds.min().y >= 0.0 && (0.70..0.75).contains(&bounds.max().y));
                    }
                    assert_eq!(models.cards.indices.len() / 3, 4);
                    assert_eq!(models.card_material.alpha_mode, AlphaMode::Mask(0.45));
                    let images = app.world().resource::<Assets<Image>>();
                    let image = images
                        .get(models.card_material.base_color_texture.as_ref().unwrap())
                        .unwrap();
                    assert!(image.texture_descriptor.mip_level_count > 1);
                    return;
                }
                FernAssets::Failed => panic!("the supplied fern asset set should be compatible"),
                FernAssets::Loading(_) => {
                    assert!(Instant::now() < timeout, "fern glTF assets timed out");
                    std::thread::sleep(Duration::from_millis(4));
                }
            }
        }
    }
}

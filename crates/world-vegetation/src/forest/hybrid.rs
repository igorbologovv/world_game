//! Bevy asset adapter for Igor's shared tree mesh prefixes.
use bevy::{
    asset::RenderAssetUsages,
    camera::primitives::Aabb,
    color::Alpha as _,
    image::{ImageAddressMode, ImageSampler},
    mesh::{Indices, MeshVertexAttribute, VertexAttributeValues},
    prelude::*,
    render::render_resource::{TextureFormat, VertexFormat},
    tasks::{block_on, poll_once, AsyncComputeTaskPool, Task},
};

use super::{
    hero_oak::HeroOakAssets, placement::Site, tree_assets::TreeAssets, FoliageMaterial,
    ForestModels, HybridMaterial, HybridWind, ModelLod, Species,
};

pub(super) const MORPH: MeshVertexAttribute =
    MeshVertexAttribute::new("Hybrid_Tree_Morph", 954_572_831, VertexFormat::Float32x4);

pub(super) struct Part {
    pub meshes: [Handle<Mesh>; 3],
    pub material: Handle<HybridMaterial>,
    pub bounds: Aabb,
    pub triangles: [usize; 3],
}

pub(super) struct Model {
    pub wood: Option<Part>,
    pub leaves: Part,
    pub bounds: Aabb,
}

type BuiltPart = ([Mesh; 3], Aabb, Handle<HybridMaterial>);

#[derive(Resource, Default)]
pub(super) struct HybridModels {
    started: bool,
    pending: Vec<(Species, usize, Task<(Option<BuiltPart>, BuiltPart)>)>,
    models: [Vec<Option<Model>>; Species::ALL.len()],
    geometry_bytes: usize,
    texture_bytes: usize,
}

impl HybridModels {
    pub fn ready(&self) -> bool {
        self.started && self.pending.is_empty()
    }

    pub fn model(&self, site: Site) -> &Model {
        let family = &self.models[site.species as usize];
        family[site.variant]
            .as_ref()
            .or_else(|| family[site.variant % site.species.stock_variants()].as_ref())
            .expect("procedural trees always have a hybrid model")
    }
}

pub(super) fn prepare_models(
    mut source: ResMut<ForestModels>,
    mut hybrid: ResMut<HybridModels>,
    hero_loading: Res<HeroOakAssets>,
    tree_loading: Res<TreeAssets>,
    standard: Res<Assets<StandardMaterial>>,
    foliage_materials: Res<Assets<FoliageMaterial>>,
    mut materials: ResMut<Assets<HybridMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    images: Res<Assets<Image>>,
) {
    if hybrid.ready() || hero_loading.is_pending() || tree_loading.is_pending() {
        return;
    }
    if !hybrid.started {
        let pool = AsyncComputeTaskPool::get();
        let hybrid = &mut *hybrid;
        let mut textures = std::collections::HashSet::new();
        let mut enqueue = |species, variant, lod: &ModelLod, bark: &Handle<StandardMaterial>| {
            let wood_material = standard.get(bark).expect("resident bark material").clone();
            let leaves = foliage_materials
                .get(&lod.foliage_material)
                .expect("resident leaf material");
            for base in [&wood_material, &leaves.base] {
                textures.extend(
                    [
                        &base.base_color_texture,
                        &base.normal_map_texture,
                        &base.metallic_roughness_texture,
                        &base.occlusion_texture,
                        &base.emissive_texture,
                    ]
                    .into_iter()
                    .filter_map(|handle| handle.as_ref().map(Handle::id)),
                );
            }
            let alpha = Alpha::from_material(&leaves.base, &images);
            let foliage = materials.add(HybridMaterial {
                base: leaves.base.clone(),
                extension: HybridWind {
                    crown_frame: leaves.extension.crown_frame,
                },
            });
            let wood = lod.wood.as_ref().map(|part| {
                let handle = materials.add(HybridMaterial {
                    base: wood_material,
                    extension: default(),
                });
                (meshes.get(&part.mesh).unwrap().clone(), part.bounds, handle)
            });
            let leaves = meshes.get(&lod.foliage).unwrap().clone();
            let foliage_bounds = lod.foliage_bounds;
            let bounds = union(foliage_bounds, wood.as_ref().map(|part| part.1));
            hybrid.pending.push((
                species,
                variant,
                pool.spawn(async move {
                    let wood = wood.map(|(mesh, own_bounds, material)| {
                        (mesh_levels(mesh, bounds, None), own_bounds, material)
                    });
                    (
                        wood,
                        (
                            mesh_levels(leaves, bounds, alpha.as_ref()),
                            foliage_bounds,
                            foliage,
                        ),
                    )
                }),
            ));
        };
        for species in Species::ALL {
            let family = &source.variants[species as usize];
            for (index, model) in family.iter().enumerate() {
                if species == Species::Oak && index == 0 {
                    if let Some(hero) = &source.hero_oak {
                        enqueue(species, index, &hero.lods[0], &hero.bark);
                        continue;
                    }
                }
                if let Some(lod) = model.lods.first() {
                    enqueue(
                        species,
                        index,
                        lod,
                        model.bark.as_ref().unwrap_or(&source.bark_material),
                    );
                }
            }
            hybrid.models[species as usize].resize_with(family.len(), || None);
        }
        hybrid.texture_bytes = textures
            .into_iter()
            .filter_map(|id| images.get(id))
            .map(texture_bytes)
            .sum();
        hybrid.started = true;
    }
    for _ in 0..8 {
        let Some((index, (wood, leaves))) = hybrid
            .pending
            .iter_mut()
            .enumerate()
            .find_map(|(i, (_, _, task))| block_on(poll_once(task)).map(|built| (i, built)))
        else {
            break;
        };
        let (species, variant, _) = hybrid.pending.swap_remove(index);
        let mut upload = |(levels, bounds, material): BuiltPart| {
            let triangles = levels
                .each_ref()
                .map(|mesh| mesh.indices().unwrap().len() / 3);
            let meshes = levels.map(|mesh| {
                hybrid.geometry_bytes += mesh
                    .attributes()
                    .map(|(_, data)| data.get_bytes().len())
                    .sum::<usize>()
                    + mesh.indices().unwrap().len()
                        * if mesh.count_vertices() <= 65_536 {
                            2
                        } else {
                            4
                        };
                meshes.add(mesh)
            });
            Part {
                meshes,
                material,
                bounds,
                triangles,
            }
        };
        let wood = wood.map(&mut upload);
        let leaves = upload(leaves);
        let bounds = union(leaves.bounds, wood.as_ref().map(|part| part.bounds));
        hybrid.models[species as usize][variant] = Some(Model {
            wood,
            leaves,
            bounds,
        });
    }
    if hybrid.ready() {
        let models = hybrid.models.iter().flatten().flatten();
        let count = models.clone().count();
        let mut triangles = [0; 3];
        for model in models {
            for part in model.wood.iter().chain(std::iter::once(&model.leaves)) {
                for (total, count) in triangles.iter_mut().zip(part.triangles) {
                    *total += count;
                }
            }
        }
        info!(
            "Hybrid forest loaded {count} models, shared triangles {triangles:?}; GPU geometry {:.2} MiB, textures {:.2} MiB",
            hybrid.geometry_bytes as f64 / 1_048_576.0,
            hybrid.texture_bytes as f64 / 1_048_576.0
        );
        // The GPU and CPU only need the three shared output meshes from this point.
        source.variants.clear();
        source.hero_oak = None;
    }
}

// Count logical GPU payloads even when Bevy has released their CPU image data.
fn texture_bytes(image: &Image) -> usize {
    let desc = &image.texture_descriptor;
    let Some(block_bytes) = desc.format.block_copy_size(None) else {
        return 0;
    };
    let (block_width, block_height) = desc.format.block_dimensions();
    (0..desc.mip_level_count)
        .filter_map(|level| desc.mip_level_size(level))
        .map(|size| {
            size.width.div_ceil(block_width) as usize
                * size.height.div_ceil(block_height) as usize
                * size.depth_or_array_layers as usize
                * block_bytes as usize
        })
        .sum()
}

fn union(first: Aabb, second: Option<Aabb>) -> Aabb {
    second.map_or(first, |second| {
        Aabb::from_min_max(
            first.min().min(second.min()).into(),
            first.max().max(second.max()).into(),
        )
    })
}

struct Alpha {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    wrap: [ImageAddressMode; 2],
    cutoff: f32,
    factor: f32,
    uv_transform: bevy::math::Affine2,
}

impl Alpha {
    fn from_material(material: &StandardMaterial, images: &Assets<Image>) -> Option<Self> {
        let AlphaMode::Mask(cutoff) = material.alpha_mode else {
            return None;
        };
        let image = images.get(material.base_color_texture.as_ref()?)?;
        if !matches!(
            image.texture_descriptor.format,
            TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb
        ) {
            return None;
        }
        let size = image.texture_descriptor.size;
        let (width, height) = (size.width as usize, size.height as usize);
        let rgba = image.data.as_ref()?.get(..width * height * 4)?;
        let pixels = rgba.chunks_exact(4).map(|pixel| pixel[3]).collect();
        let wrap = match &image.sampler {
            ImageSampler::Descriptor(sampler) => [sampler.address_mode_u, sampler.address_mode_v],
            ImageSampler::Default => [ImageAddressMode::ClampToEdge; 2],
        };
        Some(Self {
            pixels,
            width,
            height,
            wrap,
            cutoff,
            factor: material.base_color.alpha(),
            uv_transform: material.uv_transform,
        })
    }

    fn visible(&self, uv: [f32; 2]) -> bool {
        let uv = self.uv_transform.transform_point2(Vec2::from_array(uv));
        let coord = |v: f32, wrap, size: usize| {
            let v = match wrap {
                ImageAddressMode::Repeat => v.rem_euclid(1.0),
                ImageAddressMode::MirrorRepeat => 1.0 - (v.rem_euclid(2.0) - 1.0).abs(),
                _ => v.clamp(0.0, 1.0),
            };
            ((v * size as f32) as usize).min(size - 1)
        };
        let x = coord(uv.x, self.wrap[0], self.width);
        let y = coord(uv.y, self.wrap[1], self.height);
        f32::from(self.pixels[y * self.width + x]) * (self.factor / 255.0) >= self.cutoff
    }
}

fn mesh_levels(source: Mesh, bounds: Aabb, alpha: Option<&Alpha>) -> [Mesh; 3] {
    let Some(VertexAttributeValues::Float32x3(positions)) =
        source.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        unreachable!("forest meshes use Float32x3 positions")
    };
    let Some(VertexAttributeValues::Float32x2(uvs)) = source.attribute(Mesh::ATTRIBUTE_UV_0) else {
        unreachable!("forest meshes use Float32x2 UVs")
    };
    let original: Vec<_> = source
        .indices()
        .unwrap()
        .iter()
        .map(|index| index as u32)
        .collect();
    let sample = |uv| alpha.is_none_or(|alpha| alpha.visible(uv));
    let levels = igor_renderer::build(
        positions,
        uvs,
        &original,
        (bounds.min().to_array(), bounds.max().to_array()),
        alpha.map(|_| &sample as &dyn Fn([f32; 2]) -> bool),
    );
    std::array::from_fn(|stage| {
        let mut remap = vec![u32::MAX; positions.len()];
        let mut used = Vec::new();
        let mut indices = Vec::with_capacity(levels.counts[stage]);
        for &index in &levels.indices[..levels.counts[stage]] {
            let index = index as usize;
            if remap[index] == u32::MAX {
                remap[index] = used.len() as u32;
                used.push(index);
            }
            indices.push(remap[index]);
        }
        let mut mesh = Mesh::new(source.primitive_topology(), RenderAssetUsages::RENDER_WORLD);
        for (attribute, values) in source.attributes() {
            if attribute.id == Mesh::ATTRIBUTE_POSITION.id {
                continue;
            }
            let values = match values {
                VertexAttributeValues::Float32x2(v) => {
                    VertexAttributeValues::Float32x2(used.iter().map(|&i| v[i]).collect())
                }
                VertexAttributeValues::Float32x3(v) => {
                    VertexAttributeValues::Float32x3(used.iter().map(|&i| v[i]).collect())
                }
                VertexAttributeValues::Float32x4(v) => {
                    VertexAttributeValues::Float32x4(used.iter().map(|&i| v[i]).collect())
                }
                _ => continue,
            };
            mesh.insert_attribute(*attribute, values);
        }
        let position = |i: usize, stage| match stage {
            0 => positions[i],
            1 => levels.vertices[i].middle,
            _ => levels.vertices[i].far,
        };
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            used.iter().map(|&i| position(i, stage)).collect::<Vec<_>>(),
        );
        if stage < 2 {
            let morph: Vec<_> = used
                .iter()
                .map(|&i| {
                    let next = position(i, stage + 1);
                    let retire = levels.vertices[i].retire_at - stage as f32;
                    [next[0], next[1], next[2], retire]
                })
                .collect();
            mesh.insert_attribute(MORPH, morph);
        }
        mesh.insert_indices(if used.len() <= 65_536 {
            Indices::U16(indices.into_iter().map(|index| index as u16).collect())
        } else {
            Indices::U32(indices)
        });
        mesh
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::primitives::MeshAabb;

    #[test]
    fn stock_tree_proxies_keep_full_mesh_and_share_transition_endpoints() {
        fn positions(mesh: &Mesh) -> &[[f32; 3]] {
            match mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() {
                VertexAttributeValues::Float32x3(values) => values,
                _ => unreachable!(),
            }
        }

        for species in Species::ALL {
            let (wood, leaves) = super::super::generate_tree(species, 0);
            let wood = wood.finish();
            let leaves = leaves.finish_canopy(0.0);
            let bounds = union(wood.get_aabb().unwrap(), leaves.get_aabb());
            let image = super::super::textures::canopy_texture(species);
            let mut images = Assets::default();
            let material = StandardMaterial {
                base_color_texture: Some(images.add(image)),
                alpha_mode: AlphaMode::Mask(super::super::FOLIAGE_CUTOFF),
                ..default()
            };
            let alpha = Alpha::from_material(&material, &images).unwrap();
            let mut triangles = [0; 3];
            for (source, alpha) in [(wood, None), (leaves, Some(&alpha))] {
                let original_count = source.indices().unwrap().len();
                let levels = mesh_levels(source, bounds, alpha);
                assert_eq!(levels[0].indices().unwrap().len(), original_count);
                for (total, mesh) in triangles.iter_mut().zip(&levels) {
                    *total += mesh.indices().unwrap().len() / 3;
                }
                for pair in levels.windows(2) {
                    let [from, to] = pair else { unreachable!() };
                    let VertexAttributeValues::Float32x4(morph) = from.attribute(MORPH).unwrap()
                    else {
                        unreachable!()
                    };
                    let next = positions(to);
                    assert!(morph.len() >= next.len());
                    assert!(morph.iter().zip(next).all(|(from, to)| from[..3] == to[..]));
                    let prefix = from
                        .indices()
                        .unwrap()
                        .iter()
                        .take(to.indices().unwrap().len());
                    assert!(prefix.eq(to.indices().unwrap().iter()));
                }
            }
            assert!(triangles[0] >= triangles[1] && triangles[1] >= triangles[2]);
            assert!(triangles[0] > triangles[2], "{species:?}: {triangles:?}");
        }
    }
}

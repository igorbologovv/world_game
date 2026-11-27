//! Input/output adapter for Bevy. The original loader's add_triangle and
//! PointScene template_data are the only geometry and LOD builders.
use crate::{TreeGeometry,splat::{PointScene,Range,Point,LOCAL_WIDTHS}};
use std::{collections::HashMap, mem::{size_of, size_of_val}};
pub const RAW:u32=0x8000_0000;
/// Template cell widths of the original full hierarchy. Zero represents raw surfels.
pub const BASELINE_LOD_WIDTHS: &[f32] = LOCAL_WIDTHS;

/// Requested planetary template density. Larger modes retain every baseline width.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LodLevels {
    /// The original hierarchy, including its legacy environment overrides.
    #[default]
    Levels22,
    /// The full baseline with 18 logarithmically spaced intermediate levels.
    Levels40,
    /// The full baseline with 42 logarithmically spaced intermediate levels.
    Levels64,
}

impl LodLevels {
    /// Number of widths, including the raw tier.
    pub fn count(self) -> usize {
        match self {
            Self::Levels22 => BASELINE_LOD_WIDTHS.len(),
            Self::Levels40 => 40,
            Self::Levels64 => 64,
        }
    }

    fn fallback(self) -> Option<Self> {
        match self {
            Self::Levels64 => Some(Self::Levels40),
            Self::Levels40 => Some(Self::Levels22),
            Self::Levels22 => None,
        }
    }
}
pub struct InputImage{pub width:u32,pub height:u32,pub rgba:Vec<u8>}
pub struct InputPart{
    pub positions:Vec<[f32;3]>,pub normals:Vec<[f32;3]>,pub uvs:Vec<[f32;2]>,pub indices:Vec<u32>,
    pub colors:Vec<[f32;4]>,pub tint:[f32;4],pub diffuse:InputImage,pub normal_map:Option<InputImage>,
    pub repeat:bool,pub cutoff:f32,pub foliage:bool,
}
pub struct InputTemplate{pub tile:usize,pub parts:Vec<InputPart>}
pub struct Transform{pub tile:usize,pub root:[f32;3],pub scale:f32}
#[derive(Clone,Copy)]
pub struct Summary{pub position:[f32;3],pub color:[f32;3],pub normal:[f32;3],pub radius:f32,pub coverage:f32}
pub struct TemplateInfo{pub ranges:Box<[Range]>,pub radii:Box<[f32]>,pub center:[f32;3],pub bound:f32,pub summary:Summary}
pub struct Template{pub transform:Transform,pub info:TemplateInfo}
pub use crate::render_api::{AtlasTile,AtlasUpload as AtlasData};
pub struct Pack{
    pub raw:Vec<[u32;4]>,pub lod:Vec<[u32;4]>,pub patches:Vec<[u32;32]>,pub materials:Vec<[u32;16]>,
    pub atlas:AtlasData,pub templates:HashMap<usize,Template>,pub levels:Vec<usize>,
    /// Cell widths indexed by the template ranges and active level identifiers.
    pub widths: Vec<f32>,
}

/// Size in bytes of each GPU storage buffer for a template pack.
#[derive(Debug, PartialEq, Eq)]
pub struct BufferSizes {
    /// Raw textured surfels.
    pub raw: u64,
    /// Aggregated surfels across all local levels.
    pub lod: u64,
    /// Source triangle records.
    pub patches: u64,
    /// Source material records.
    pub materials: u64,
}

fn storage_bytes<T>(len: usize) -> u64 {
    len.max(1) as u64 * size_of::<T>() as u64
}

fn check_storage<T>(name: &str, len: usize, limit: u64) -> Result<(), String> {
    let bytes = storage_bytes::<T>(len);
    if bytes > limit {
        return Err(format!("{name} buffer needs {bytes} bytes, device limit is {limit} bytes"));
    }
    Ok(())
}

impl Pack{
    /// Bake a pack with the original level settings.
    pub fn build(inputs:Vec<InputTemplate>,max_storage:u64,max_layers:u32)->Result<Self,String>{
        Self::build_with_lods(inputs, LodLevels::default(), max_storage, max_layers)
    }

    /// Bake a pack with the requested density, reducing only the LOD density
    /// when its point buffer would exceed the device's effective storage limit.
    pub fn build_with_lods(inputs:Vec<InputTemplate>,requested:LodLevels,max_storage:u64,max_layers:u32)->Result<Self,String>{
        let mut geometry=TreeGeometry::planetary_empty();
        let mut transforms=Vec::new();
        for input in inputs{
            let tile=input.tile;
            let transform=geometry.add_planet_template(input).map_err(|e|format!("template {tile}: {e}"))?;
            transforms.push(transform);
            check_storage::<[u32;32]>("source patch",geometry.patches.len(),max_storage)?;
            check_storage::<[u32;16]>("material",geometry.materials.len(),max_storage)?;
            check_storage::<Point>("raw point",geometry.surfels.len(),max_storage)?;
        }
        // Empty buffers still need one bindable record. The high bit in a range
        // offset also reserves separate address spaces for raw and LOD points.
        check_storage::<[u32;32]>("source patch",geometry.patches.len(),max_storage)?;
        check_storage::<[u32;16]>("material",geometry.materials.len(),max_storage)?;
        check_storage::<Point>("raw point",geometry.surfels.len(),max_storage)?;
        check_storage::<Point>("LOD",0,max_storage)?;
        let point_limit = (max_storage / size_of::<Point>() as u64).min(u64::from(RAW) - 1) as usize;
        if geometry.surfels.len() > point_limit {
            return Err(format!("raw point buffer exceeds the {point_limit}-record range limit"));
        }
        let atlas=geometry.atlas_upload();
        if atlas.layers>max_layers{return Err(format!("texture atlas needs {} layers, device limit is {max_layers}", atlas.layers));}
        let mut mode = requested;
        let (raw,lod,infos,widths,levels) = loop {
            match PointScene::planet_templates(&geometry, mode.count(), point_limit) {
                Ok(data) => break data,
                Err(error) => {
                    let Some(fallback) = mode.fallback() else { return Err(error) };
                    tracing::warn!(levels = mode.count(), retry_levels = fallback.count(), %error,
                        "Point forest LOD pack exceeds capacity, retrying with fewer levels");
                    mode = fallback;
                }
            }
        };
        let templates=transforms.into_iter().zip(infos).map(|(transform,info)|(transform.tile,Template{transform,info})).collect();
        Ok(Self{raw:raw.iter().map(|p|[p.xy,p.zr,p.rgba,p.normal]).collect(),
            lod:lod.iter().map(|p|[p.xy,p.zr,p.rgba,p.normal]).collect(),
            patches:geometry.patches.iter().map(|p|bytemuck::cast(*p)).collect(),
            materials:geometry.materials.iter().map(|p|bytemuck::cast(*p)).collect(),atlas,templates,levels,widths})
    }

    /// Exact storage buffer sizes, including one record for any empty buffer.
    pub fn buffer_sizes(&self) -> BufferSizes {
        BufferSizes {
            raw: storage_bytes::<[u32;4]>(self.raw.len()),
            lod: storage_bytes::<[u32;4]>(self.lod.len()),
            patches: storage_bytes::<[u32;32]>(self.patches.len()),
            materials: storage_bytes::<[u32;16]>(self.materials.len()),
        }
    }

    /// Logical RGBA8 atlas allocation shared by the color and normal views.
    pub fn atlas_bytes(&self) -> u64 {
        u64::from(self.atlas.side).pow(2) * u64::from(self.atlas.layers) * 4
    }

    /// Persistent CPU bytes in the pack arrays and template payloads. Includes
    /// reserved vector capacity, excluding hash table overhead and bake scratch.
    pub fn cpu_pack_bytes(&self) -> usize {
        fn capacity_bytes<T>(items: &Vec<T>) -> usize { items.capacity() * size_of::<T>() }
        capacity_bytes(&self.raw) + capacity_bytes(&self.lod) + capacity_bytes(&self.patches)
            + capacity_bytes(&self.materials) + capacity_bytes(&self.widths) + capacity_bytes(&self.levels)
            + capacity_bytes(&self.atlas.tiles) + self.atlas.tiles.iter().map(|tile| capacity_bytes(&tile.rgba)).sum::<usize>()
            + self.templates.values().map(|template| {
                size_of::<(usize, Template)>() + size_of_val(&*template.info.ranges) + size_of_val(&*template.info.radii)
            }).sum::<usize>()
    }
}

/// Original compute source with the existing spherical host/depth adaptations.
pub fn compute_shader()->String{crate::planetary_shader::shader()}

pub use crate::splat::planetary_selection::{
    Camera, LeafKey, Placement, PlanetHierarchy, PlanetScene, PlanetSelection, PlanetShadow,
    SelectionExecutor,
};

pub fn ground_shadow_shader()->String{crate::planetary_shader::ground_shadow_shader()}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> Vec<InputTemplate> {
        vec![InputTemplate {
            tile: 0,
            parts: vec![InputPart {
                positions: vec![[-2.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 4.0, 0.0], [-2.0, 4.0, 0.0]],
                normals: vec![[0.0, 0.0, 1.0]; 4],
                uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                indices: vec![0, 1, 2, 0, 2, 3],
                colors: vec![],
                tint: [1.0; 4],
                diffuse: InputImage { width: 1, height: 1, rgba: vec![40, 100, 60, 255] },
                normal_map: None,
                repeat: false,
                cutoff: 0.5,
                foliage: true,
            }],
        }]
    }

    fn bake(mode: LodLevels, limit: u64) -> Pack {
        Pack::build_with_lods(input(), mode, limit, 1).unwrap()
    }

    fn lod_points(pack: &Pack, width: f32) -> &[[u32; 4]] {
        let level = crate::splat::width_index(&pack.widths, width).unwrap();
        let range = pack.templates[&0].info.ranges[level];
        &pack.lod[range.offset as usize..(range.offset + range.count) as usize]
    }

    #[test]
    fn dense_bakes_retain_baseline_widths_records_and_shadows() {
        let baseline = bake(LodLevels::Levels22, u64::MAX);
        let forty = bake(LodLevels::Levels40, u64::MAX);
        let sixty_four = bake(LodLevels::Levels64, u64::MAX);
        assert_eq!(baseline.widths, BASELINE_LOD_WIDTHS);
        for (pack, mode) in [(&forty, LodLevels::Levels40), (&sixty_four, LodLevels::Levels64)] {
            assert_eq!(pack.widths.len(), mode.count());
            assert_eq!(pack.levels.len(), mode.count());
            assert!(pack.widths.windows(2).all(|pair| pair[0] < pair[1]));
            assert_eq!(pack.raw, baseline.raw);
            for &width in &baseline.widths[1..] {
                assert_eq!(lod_points(pack, width), lod_points(&baseline, width));
            }
            assert!(pack.templates[&0].info.ranges[1..].windows(2)
                .all(|levels| levels[0].count >= levels[1].count));
        }
        assert!(forty.widths.iter().all(|width| sixty_four.widths.contains(width)));
        assert_eq!(&sixty_four.widths[sixty_four.widths.len() - 3..], &[4.0, 8.0, 16.0]);
        let rebuilt = bake(LodLevels::Levels64, u64::MAX);
        assert_eq!(rebuilt.lod, sixty_four.lod);
        assert_eq!(rebuilt.templates[&0].info.ranges, sixty_four.templates[&0].info.ranges);

        let site = Placement { position: [0.0, 1000.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: 1.0, model: 0, phase: 0.0 };
        let shadow = |pack| PlanetShadow::bake(pack, vec![site], [0.0, 1001.0, 0.0], [0.0, 1.0, 0.0]).unwrap();
        let baseline_shadow = shadow(&baseline);
        assert!(baseline_shadow.pixels.iter().any(|&byte| byte != 0));
        assert_eq!(baseline_shadow.pixels, shadow(&sixty_four).pixels);
    }

    #[test]
    fn dense_bake_falls_back_and_baseline_rejects_undersized_device() {
        let original = bake(LodLevels::Levels22, u64::MAX);
        let forty = bake(LodLevels::Levels40, u64::MAX);
        let full = bake(LodLevels::Levels64, u64::MAX);
        let baseline_limit = original.buffer_sizes().lod;
        let forty_limit = forty.buffer_sizes().lod;
        assert!(baseline_limit < forty_limit && forty_limit < full.buffer_sizes().lod);
        let fallback = bake(LodLevels::Levels64, forty_limit);
        assert_eq!(fallback.widths, forty.widths);
        assert_eq!(fallback.lod, forty.lod);
        let fallback = bake(LodLevels::Levels64, baseline_limit);
        assert_eq!(fallback.widths, original.widths);
        assert_eq!(fallback.buffer_sizes(), original.buffer_sizes());
        let error = Pack::build_with_lods(input(), LodLevels::Levels22, baseline_limit - 16, 1).err().unwrap();
        assert!(error.contains("LOD buffer needs"));
    }
}

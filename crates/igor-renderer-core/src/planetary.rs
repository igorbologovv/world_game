//! Input/output adapter for Bevy. The original loader's add_triangle and
//! PointScene template_data are the only geometry and LOD builders.
use crate::{TreeGeometry,splat::{PointScene,Range}};
use std::collections::HashMap;
pub const RAW:u32=0x8000_0000;
pub const LEVELS:usize=22;
pub struct InputImage{pub width:u32,pub height:u32,pub rgba:Vec<u8>}
pub struct InputPart{
    pub positions:Vec<[f32;3]>,pub normals:Vec<[f32;3]>,pub uvs:Vec<[f32;2]>,pub indices:Vec<u32>,
    pub tint:[f32;4],pub diffuse:InputImage,pub normal_map:Option<InputImage>,
    pub repeat:bool,pub cutoff:f32,pub foliage:bool,
}
pub struct InputTemplate{pub tile:usize,pub parts:Vec<InputPart>}
pub struct Transform{pub tile:usize,pub root:[f32;3],pub scale:f32}
pub struct TemplateInfo{pub ranges:[Range;LEVELS],pub radii:[f32;LEVELS],pub center:[f32;3]}
pub struct Template{pub transform:Transform,pub info:TemplateInfo}
pub struct AtlasTile{pub x:u32,pub y:u32,pub layer:u32,pub width:u32,pub height:u32,pub rgba:Vec<u8>}
pub struct AtlasData{pub side:u32,pub layers:u32,pub tiles:Vec<AtlasTile>}
pub struct Pack{
    pub raw:Vec<[u32;4]>,pub lod:Vec<[u32;4]>,pub patches:Vec<[u32;32]>,pub materials:Vec<[u32;16]>,
    pub atlas:AtlasData,pub templates:HashMap<usize,Template>,
}
impl Pack{
    pub fn build(inputs:Vec<InputTemplate>,max_storage:u64,max_layers:u32)->Result<Self,String>{
        let mut geometry=TreeGeometry::planetary_empty();
        let mut transforms=Vec::new();
        for input in inputs{
            let tile=input.tile;
            let transform=geometry.add_planet_template(input).map_err(|e|format!("template {tile}: {e}"))?;
            transforms.push(transform);
            if (geometry.patches.len() as u64)*128>max_storage{return Err("original source patch buffer exceeds device storage limit".into());}
        }
        let (raw,lod,infos)=PointScene::planet_templates(&geometry);
        if [raw.len() as u64*16,lod.len() as u64*16,geometry.materials.len() as u64*64].iter().any(|&x|x>max_storage){
            return Err("original raw/LOD buffer exceeds device storage limit".into());
        }
        let atlas=geometry.atlas_data();
        if atlas.layers>max_layers{return Err("original texture atlas exceeds device layer limit".into());}
        let templates=transforms.into_iter().zip(infos).map(|(transform,info)|(transform.tile,Template{transform,info})).collect();
        Ok(Self{raw:raw.iter().map(|p|[p.xy,p.zr,p.rgba,p.normal]).collect(),
            lod:lod.iter().map(|p|[p.xy,p.zr,p.rgba,p.normal]).collect(),
            patches:geometry.patches.iter().map(|p|bytemuck::cast(*p)).collect(),
            materials:geometry.materials.iter().map(|p|bytemuck::cast(*p)).collect(),atlas,templates})
    }
}

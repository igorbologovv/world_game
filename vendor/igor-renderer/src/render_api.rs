//! Backend-independent ABI and original shaders. No window, device or queue
//! is created by this module. The demo and external hosts share these sources.
use bytemuck::{Pod,Zeroable};

pub const COMPUTE_SHADER:&str=include_str!("compute_splat.wgsl");
pub const PRESENT_SHADER:&str=include_str!("compute_present.wgsl");
pub const GPU_LOD_SHADER:&str=include_str!("gpu_lod.wgsl");
pub const TEMPORAL_SHADER:&str=include_str!("temporal.wgsl");
pub const LEGACY_SPLAT_SHADER:&str=include_str!("splat.wgsl");
pub const DEFAULT_POINT_BUDGET:u64=3_500_000;
pub const LANES:u32=64;
pub const JOB_WIDTH:u32=1024;
pub const RAW_RANGE_BIT:u32=0x8000_0000;
pub const EMPTY_PIXEL:u32=0xffff_ffff;

#[repr(C)]
#[derive(Clone,Copy,Debug,Pod,Zeroable)]
pub struct FrameUniform{
    pub origin:[f32;4],pub forward:[f32;4],pub right:[f32;4],pub up:[f32;4],
    pub screen:[f32;4],pub extent:[f32;4],pub hud:[[u32;4];8],pub params:[u32;4],
    pub effects:[f32;4],pub shadow_transform:[f32;4],
}

/// Depth, then owner IDs, then optional Hi-Z and instance flags.
/// See compute_splat.wgsl for the positive-view-depth encoding.
pub fn stats_word_offset(w:u32,h:u32)->u64 {
    let mut n=u64::from(w)*u64::from(h)*2;
    for s in [4u32,16,64,256]{n+=u64::from(w.div_ceil(s))*u64::from(h.div_ceil(s));}
    n
}
pub fn screen_bytes(w:u32,h:u32,max_instances:u64)->u64{(stats_word_offset(w,h)+4+max_instances)*4}

#[repr(C)]
#[derive(Clone,Copy,Debug,Pod,Zeroable)]
pub struct Job{pub instance_id:u32,pub first:u32,pub count:u32,pub padding:u32}
pub fn build_jobs(selection:&crate::splat::Selection)->Vec<Job>{
    let mut jobs=Vec::with_capacity((selection.points/64) as usize+selection.instances.len());
    for d in &selection.draws{
        for instance_id in d.start..d.end{
            for first in (0..d.point_count).step_by(LANES as usize){
                jobs.push(Job{instance_id,first,count:(d.point_count-first).min(LANES),padding:0});
            }
        }
    }
    jobs
}

/// Upload descriptors for the original atlas including its wrap gutters.
pub struct AtlasTile{
    pub origin:[u32;3],pub size:[u32;2],pub rgba:Vec<u8>,
}
pub struct AtlasUpload{
    pub side:u32,pub layers:u32,pub tiles:Vec<AtlasTile>,
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn stable_shader_abi(){
        assert_eq!(std::mem::size_of::<FrameUniform>(),272);
        assert_eq!(std::mem::size_of::<Job>(),16);
        assert_eq!(std::mem::size_of::<crate::splat::Point>(),16);
        assert_eq!(std::mem::size_of::<crate::splat::Instance>(),48);
        assert_eq!(std::mem::size_of::<crate::Patch>(),128);
        assert_eq!(std::mem::size_of::<crate::Material>(),64);
        assert_eq!(screen_bytes(5,3,7),180);
    }
}

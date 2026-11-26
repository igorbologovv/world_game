//! Host-specific shader assembly derived from the unchanged standalone WGSL.
//! This is an exact extraction of the previously running Bevy adapter.
//! The line splices are tied to the source fingerprint; edits to the common
//! shader must review the integration boundary rather than silently drift.
pub fn shader()->String{
    let source=crate::render_api::COMPUTE_SHADER;
    let fingerprint=source.bytes().fold(0xcbf29ce484222325u64,|h,b|(h^u64::from(b)).wrapping_mul(0x100000001b3));
    assert_eq!(fingerprint,8111712895578801785u64,"review planetary shader splices after changing the shared shader");
    let mut lines=source.split_inclusive('\n');
    let mut out=String::with_capacity(source.len());
    out.push_str(r####"// Port of Igor_renderer 9d50c07 compute_splat.wgsl. Planetary quaternion, exposure and Bevy composite adapters only; optional Hi-Z/TAA omitted.
"####);
    for _ in 0..1 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 1..4 {let _=lines.next().expect("host-adapted shader line");}
    out.push_str(r####"    origin:vec4<f32>,forward:vec4<f32>,right:vec4<f32>,up:vec4<f32>,screen:vec4<f32>,extent:vec4<f32>,
    params:vec4<u32>,effects:vec4<f32>,sun:vec4<f32>,sunlight:vec4<f32>,depth_coeff:vec4<f32>,viewport:vec4<u32>,
    shadow_x:vec4<f32>,shadow_z:vec4<f32>,shadow_height:vec4<f32>,
"####);
    for _ in 4..6 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 6..7 {let _=lines.next().expect("host-adapted shader line");}
    out.push_str(r####"struct Instance {origin_scale:vec4<f32>,rotation:vec4<f32>,range:vec4<u32>}
"####);
    for _ in 7..27 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 27..35 {let _=lines.next().expect("host-adapted shader line");}
    out.push_str(r####"fn rotate(p:vec3<f32>,q:vec4<f32>)->vec3<f32>{let t=2.0*cross(q.xyz,p);return p+q.w*t+cross(q.xyz,t);}
"####);
    for _ in 35..73 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 73..94 {let _=lines.next().expect("host-adapted shader line");}
    for _ in 94..95 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 95..98 {let _=lines.next().expect("host-adapted shader line");}
    out.push_str(r####"    let sun=normalize(cam.sun.xyz);
    return c*(0.10+0.24*max(dot(n,sun),0.0)*planet_visibility(world));
"####);
    for _ in 98..99 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 99..100 {let _=lines.next().expect("host-adapted shader line");}
    for _ in 100..105 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 105..108 {let _=lines.next().expect("host-adapted shader line");}
    out.push_str(r####"@group(0) @binding(11) var<storage,read_write> projection_cache:array<vec2<u32>>;
@group(0) @binding(12) var canopy_shadow:texture_2d<f32>;
"####);
    for _ in 108..126 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 126..159 {let _=lines.next().expect("host-adapted shader line");}
    for _ in 159..164 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 164..228 {let _=lines.next().expect("host-adapted shader line");}
    for _ in 228..232 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 232..234 {let _=lines.next().expect("host-adapted shader line");}
    out.push_str(r####"    if (render_flags()&16u)==0u||inst.range.z==1u{return vec3<f32>(0.0);}
    let world=inst.origin_scale.xyz+rotate(local,inst.rotation)*inst.origin_scale.w;
"####);
    for _ in 234..240 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 240..241 {let _=lines.next().expect("host-adapted shader line");}
    out.push_str(r####"    let phase=bitcast<f32>(inst.range.w);
"####);
    for _ in 241..259 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 259..261 {let _=lines.next().expect("host-adapted shader line");}
    out.push_str(r####"    let index=(inst.range.x&0x7fffffffu)+j.first+lane;
    let world_cluster=inst.range.z==1u;
    let detailed=!world_cluster&&(inst.range.x&0x80000000u)!=0u;
"####);
    for _ in 261..262 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 262..263 {let _=lines.next().expect("host-adapted shader line");}
    out.push_str(r####"    if world_cluster{pt=Point(0u,4096u<<16u,inst.range.x,inst.range.y);}
    else if detailed{pt=raw_points[index];}else{pt=lod_points[index];}
"####);
    for _ in 263..279 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 279..280 {let _=lines.next().expect("host-adapted shader line");}
    out.push_str(r####"    let c=camera_vector(inst.origin_scale.xyz+rotate(local,inst.rotation)*inst.origin_scale.w+wind_offset(inst,local,pt,detailed)-cam.origin.xyz);
"####);
    for _ in 280..300 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 300..302 {let _=lines.next().expect("host-adapted shader line");}
    out.push_str(r####"    let a=camera_vector(rotate(a_local,inst.rotation)*inst.origin_scale.w);
    let b=camera_vector(rotate(b_local,inst.rotation)*inst.origin_scale.w);
"####);
    for _ in 302..385 {out.push_str(lines.next().expect("shared shader line"));}
    out.push_str(r####"fn in_dither(pixel:vec2<u32>,upper:u32)->bool{
    if upper>=12u{return true;}
    let absolute=vec2<f32>(pixel+cam.viewport.xy);
    return fract(52.9829189*fract(dot(absolute,vec2<f32>(0.06711056,0.00583715))))<f32(upper)/12.0;
}
"####);
    for _ in 385..403 {out.push_str(lines.next().expect("shared shader line"));}
    out.push_str(r####"        if !in_dither(pixel,j.padding){return;}
"####);
    for _ in 403..419 {out.push_str(lines.next().expect("shared shader line"));}
    out.push_str(r####"            if !in_dither(pixel,j.padding){continue;}
"####);
    for _ in 419..437 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 437..438 {let _=lines.next().expect("host-adapted shader line");}
    out.push_str(r####"    atomicStore(&screen_words[2u*pixel_count()+i],0u);
"####);
    for _ in 438..444 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 444..445 {let _=lines.next().expect("host-adapted shader line");}
    for _ in 445..448 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 448..456 {let _=lines.next().expect("host-adapted shader line");}
    for _ in 456..460 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 460..461 {let _=lines.next().expect("host-adapted shader line");}
    for _ in 461..465 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 465..466 {let _=lines.next().expect("host-adapted shader line");}
    out.push_str(r####"    let world=p.inst.origin_scale.xyz+rotate(local,p.inst.rotation)*p.inst.origin_scale.w+wind_offset(p.inst,p.local_center,p.point,p.detailed);
"####);
    for _ in 466..467 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 467..468 {let _=lines.next().expect("host-adapted shader line");}
    out.push_str(r####"    var n=rotate(normal_at(p.point.normal),p.inst.rotation);
"####);
    for _ in 468..472 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 472..473 {let _=lines.next().expect("host-adapted shader line");}
    out.push_str(r####"        n=rotate(surface_normal(mat,surface,s.bary),p.inst.rotation);
"####);
    for _ in 473..480 {out.push_str(lines.next().expect("shared shader line"));}
    for _ in 480..508 {let _=lines.next().expect("host-adapted shader line");}
    out.push_str(r####"    let i=pixel_id(id.xy);let winner=atomicLoad(&screen_words[pixel_count()+i]);
    if winner==EMPTY{return;}
    let cached=projection_cache[winner];
    let p=project_point(jobs[winner/LANES],winner%LANES,cached);
    let s=sample_pixel(p,id.xy,0u);
    if !s.valid{atomicStore(&screen_words[pixel_count()+i],EMPTY);return;}
    let color=select(color_at(p,s),vec3<f32>(1.0,0.02,0.8),cam.params.z!=0u);
    atomicStore(&screen_words[2u*pixel_count()+i],pack4x8unorm(vec4<f32>(color,1.0)));
"####);
    for _ in 508..509 {out.push_str(lines.next().expect("shared shader line"));}
    assert!(lines.next().is_none());
    out.push_str(SHADOW_VISIBILITY);
    out
}
#[cfg(test)]
mod tests{
    #[test]fn planetary_shader_validates(){
        let source=super::shader();
        let module=naga::front::wgsl::parse_str(&source).unwrap_or_else(|e|panic!("{}",e.emit_to_string(&source)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(),naga::valid::Capabilities::all()).validate(&module).unwrap();
        let source=super::ground_shadow_shader();
        let module=naga::front::wgsl::parse_str(&source).unwrap_or_else(|e|panic!("{}",e.emit_to_string(&source)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(),naga::valid::Capabilities::all()).validate(&module).unwrap();
    }
}

const SHADOW_VISIBILITY:&str=r#"
fn planet_visibility(world:vec3<f32>)->f32{
    if cam.effects.z<=0.0{return 1.0;}
    let uv=vec2<f32>(dot(world,cam.shadow_x.xyz)+cam.shadow_x.w,dot(world,cam.shadow_z.xyz)+cam.shadow_z.w);
    if any(uv<vec2<f32>(0.0))||any(uv>vec2<f32>(1.0)){return 1.0;}
    let height=dot(world,cam.shadow_height.xyz)+cam.shadow_height.w;
    let opacity=textureSampleLevel(canopy_shadow,atlas_sampler,uv,0.0);
    // Original demo's above-receiver band weights, in band units.
    let above=smoothstep(vec4<f32>(0.125),vec4<f32>(1.125),vec4<f32>(0.5,1.5,2.5,3.5)-height);
    let edge=smoothstep(0.0,0.08,min(min(uv.x,uv.y),min(1.0-uv.x,1.0-uv.y)));
    let t=vec4<f32>(1.0)-opacity*above*cam.effects.z*edge;
    return max(0.12,t.x*t.y*t.z*t.w);
}
"#;
pub fn ground_shadow_shader()->String{
    let mut out=String::from(r#"
struct Camera {
    origin:vec4<f32>,forward:vec4<f32>,right:vec4<f32>,up:vec4<f32>,screen:vec4<f32>,extent:vec4<f32>,
    params:vec4<u32>,effects:vec4<f32>,sun:vec4<f32>,sunlight:vec4<f32>,depth_coeff:vec4<f32>,viewport:vec4<u32>,
    shadow_x:vec4<f32>,shadow_z:vec4<f32>,shadow_height:vec4<f32>,
}
@group(0) @binding(0) var<uniform> cam:Camera;
@group(0) @binding(1) var scene_depth:texture_depth_2d;
@group(0) @binding(2) var canopy_shadow:texture_2d<f32>;
@group(0) @binding(3) var atlas_sampler:sampler;
@fragment fn fragment(@builtin(position) pos:vec4<f32>)->@location(0) vec4<f32>{
    let depth=textureLoad(scene_depth,vec2<i32>(pos.xy),0);
    if depth<=0.0{discard;}
    let z=-(cam.depth_coeff.y-depth*cam.depth_coeff.w)/(depth*cam.depth_coeff.z-cam.depth_coeff.x);
    if z<=0.0{discard;}
    let pixel=pos.xy-vec2<f32>(cam.viewport.xy);
    let xy=(pixel-cam.screen.xy*0.5)*cam.screen.z*z;
    let world=cam.forward.xyz*z+cam.right.xyz*xy.x-cam.up.xyz*xy.y;
    let visibility=planet_visibility(world);
    // Opaque scene already contains ambient and existing terrain shading.
    // Approximate only the additional direct component.
    return vec4<f32>(vec3<f32>(mix(1.0,visibility,0.6)),1.0);
}
"#);
    out.push_str(SHADOW_VISIBILITY);out
}

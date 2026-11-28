struct Camera {
    origin:vec4<f32>, params:vec4<u32>, effects:vec4<f32>,
    shadow_transform:vec4<f32>, shadow_projection:vec4<f32>, sun:vec4<f32>,
    forest_band:vec4<f32>, clip_from_local:mat4x4<f32>,
}
struct Material { factor:vec4<f32>,base:vec4<u32>,normal:vec4<u32>,params:vec4<f32> }
@group(0) @binding(0) var<uniform> cam:Camera;
@group(0) @binding(1) var<storage,read> materials:array<Material>;
@group(0) @binding(2) var color_atlas:texture_2d_array<f32>;
@group(0) @binding(3) var normal_atlas:texture_2d_array<f32>;
@group(0) @binding(4) var atlas_sampler:sampler;
@group(0) @binding(5) var canopy_shadow:texture_2d<f32>;
fn render_flags()->u32{return cam.params.w;}
fn wrap_uv(uv:f32, mode:u32)->f32 {
    if mode == 1u { return clamp(uv, 0.0, 1.0); }
    if mode == 2u { return 1.0 - abs(fract(uv * 0.5) * 2.0 - 1.0); }
    return fract(uv);
}

fn atlas_uv(rect:vec4<u32>, uv:vec2<f32>)->vec2<f32> {
    let size = vec2<f32>(f32(rect.z & 65535u), f32(rect.z >> 16u));
    let origin = vec2<f32>(f32(rect.y & 65535u), f32(rect.y >> 16u));
    let wrapped = vec2<f32>(wrap_uv(uv.x, (rect.w >> 16u) & 3u), wrap_uv(uv.y, rect.w >> 18u));
    return (origin + wrapped * size) / vec2<f32>(textureDimensions(color_atlas));
}

fn clip_position(world:vec3<f32>)->vec4<f32>{
    return cam.clip_from_local * vec4<f32>(world,1.0);
}
fn sun_visibility(world:vec3<f32>)->f32{
    if (render_flags()&32u)==0u||cam.effects.z==0.0{return 1.0;}
    let height=world.y-cam.shadow_projection.z;
    let uv=(world.xz+height*cam.shadow_projection.xy+cam.shadow_transform.xy)*cam.shadow_transform.zw;
    if any(uv<vec2<f32>(0.0))||any(uv>vec2<f32>(1.0)){return 1.0;}
    let opacity=textureSampleLevel(canopy_shadow,atlas_sampler,uv,0.0);
    // Only canopy bands above the receiving surface attenuate sunlight.
    // Smooth band weights suppress seams and most same-height self-shadow.
    let above=smoothstep(vec4<f32>(0.125),vec4<f32>(1.125),vec4<f32>(0.5,1.5,2.5,3.5)-height*cam.shadow_projection.w);
    let t=vec4<f32>(1.0)-opacity*above*cam.effects.z;
    return max(0.12,t.x*t.y*t.z*t.w);
}
fn light(c:vec3<f32>,n:vec3<f32>,world:vec3<f32>)->vec3<f32>{
    let normal=select(n,-n,dot(n,cam.origin.xyz-world)<0.0);
    let daylight=smoothstep(-0.08,0.12,cam.sun.y);
    let diffuse=max(dot(normal,cam.sun.xyz),0.0)*sun_visibility(world);
    // Bevy applies aerial perspective to the shared depth.
    return c*(mix(0.025,0.36,daylight)+0.64*diffuse*daylight)*cam.effects.w;
}


struct MeshOut{
    @builtin(position) pos:vec4<f32>,
    @location(0) world:vec3<f32>,
    @location(1) normal:vec3<f32>,
    @location(2) uv:vec2<f32>,
    @location(3) normal_uv:vec2<f32>,
    @location(4) @interpolate(flat) material:u32,
    @location(5) rest:vec3<f32>,
    @location(6) @interpolate(flat) lod_range:vec2<f32>,
}
fn wind(world:vec3<f32>,local:vec3<f32>,origin:vec4<f32>,leaf:bool)->vec3<f32>{
    if (render_flags()&16u)==0u{return vec3<f32>(0.0);}
    let rel=world-cam.origin.xyz;let dist2=dot(rel,rel);
    if dist2>=19600.0{return vec3<f32>(0.0);}
    let fade=1.0-smoothstep(6400.0,19600.0,dist2);
    let h=max(0.0,local.y*origin.w);let bend=min(h*h*0.0015,0.24);
    let phase=dot(origin.xz,vec2<f32>(0.035,0.027));let t=cam.effects.x;
    let sway=sin(t*1.65+phase)*0.72+sin(t*0.61+phase*0.61)*0.28;
    let flutter=0.028*select(0.0,smoothstep(0.8,3.0,h),leaf)*sin(t*6.2+dot(world,vec3<f32>(2.1,1.7,1.3)));
    return vec3<f32>(bend*sway+flutter,flutter*0.28,bend*sway*0.48-flutter*0.7)*fade*cam.effects.y;
}
fn yaw(p:vec3<f32>,cs:vec2<f32>)->vec3<f32>{return vec3<f32>(cs.x*p.x+cs.y*p.z,p.y,-cs.y*p.x+cs.x*p.z);}
struct FragmentOut {
    @location(0) color:vec4<f32>,
    @builtin(sample_mask) mask:u32,
}
// Fixed ordered 8x8 threshold. No frame seed, identical in both shaders.
fn ordered(pixel:vec2<u32>)->f32{
    var rank=0u;
    for(var i=0u;i<3u;i=i+1u){
        let x=(pixel.x>>i)&1u;let y=(pixel.y>>i)&1u;
        rank=rank*4u+((x^y)*2u+y);
    }
    return (f32(rank)+0.5)/64.0;
}
fn sample_coverage(pos:vec2<f32>,range:vec2<f32>)->u32{
    // The host sample count drives coverage at both LOD boundaries.
    if cam.params.x==0u{return 0xffffffffu;}
    if range.y<=range.x{return 0u;}
    if range.x<=0.0&&range.y>=1.0{return 0xffffffffu;}
    let pixel=vec2<u32>(pos);
    let n=cam.params.x;
    let d=ordered(pixel);
    var bits=0u;
    for(var i=0u;i<n;i=i+1u){
        let r=(f32(i)+d)/f32(n);
        if r>=range.x&&r<range.y{bits=bits|(1u<<i);}
    }
    return bits;
}
fn forest_coverage(range:vec2<f32>,world:vec3<f32>)->vec2<f32>{
    let rel=world-cam.origin.xyz;
    let distance_squared=dot(rel,rel);
    if distance_squared<=cam.forest_band.z{return range;}
    let fade=1.0-smoothstep(cam.forest_band.x,cam.forest_band.y,sqrt(distance_squared));
    return range*fade;
}
fn fragment_out(color:vec4<f32>,bits:u32)->FragmentOut{
    var o:FragmentOut;o.color=color;o.mask=bits;return o;
}


@vertex fn mesh_vs(@location(0) p:vec3<f32>,@location(1) n:vec3<f32>,@location(2) uv:vec2<f32>,
    @location(3) normal_uv:vec2<f32>,@location(4) material:u32,@location(5) origin:vec4<f32>,@location(6) rotation:vec4<f32>)->MeshOut{
    let rest=origin.xyz+yaw(p,rotation.xy)*origin.w;
    let world=rest+wind(rest,p,origin,materials[material].params.x>=0.0);
    return MeshOut(clip_position(world),world,yaw(n,rotation.xy),uv,normal_uv,material,rest,rotation.zw);
}
@fragment fn mesh_fs(i:MeshOut)->FragmentOut{
    let mat=materials[i.material];
    var rgba=mat.factor;
    if mat.base.x!=0u{rgba*=textureSampleLevel(color_atlas,atlas_sampler,atlas_uv(mat.base,i.uv),i32(mat.base.w&65535u),0.0);}
    var n=normalize(i.normal);
    let dx=dpdx(i.rest);let dy=dpdy(i.rest);
    let ux=dpdx(i.normal_uv);let uy=dpdy(i.normal_uv);
    let det=ux.x*uy.y-ux.y*uy.x;
    if mat.normal.x!=0u&&abs(det)>1e-12{
        let t0=(dx*uy.y-dy*ux.y)/det;let b0=(dy*ux.x-dx*uy.x)/det;
        let tangent=t0-n*dot(n,t0);
        if dot(tangent,tangent)>1e-12{
            let t=normalize(tangent);let b=cross(n,t)*sign(dot(cross(n,t),b0));
            let mapped=textureSampleLevel(normal_atlas,atlas_sampler,atlas_uv(mat.normal,i.normal_uv),i32(mat.normal.w&65535u),0.0).rgb*2.0-1.0;
            n=normalize((t*mapped.x+b*mapped.y)*mat.params.y+n*mapped.z);
        }
    }
    if mat.params.x>=0.0&&rgba.a<mat.params.x{discard;}
    let bits=sample_coverage(i.pos.xy,forest_coverage(i.lod_range,i.world));if bits==0u{discard;}
    return fragment_out(vec4<f32>(light(rgba.rgb,n,i.world),1.0),bits);
}
// World-anchored hashed coverage: no camera position, frame index or
// screen pixel in the seed. Rotated lattice avoids axis-aligned patterns.
fn hash_cell(p:vec3<i32>)->f32{
    let v=bitcast<vec3<u32>>(p);
    var h=(v.x*0x9e3779b9u)^(v.y*0x85ebca6bu)^(v.z*0xc2b2ae35u);
    h=(h^(h>>16u))*0x7feb352du;
    h=(h^(h>>15u))*0x846ca68bu;
    h=h^(h>>16u);
    return f32(h>>8u)*(1.0/16777216.0);
}


struct ProxyOut {
    @builtin(position) clip:vec4<f32>,
    @location(0) color:vec4<f32>,@location(1) world:vec3<f32>,
    @location(2) @interpolate(flat) grain:f32,@location(3) @interpolate(flat) lod_range:vec2<f32>,
    @location(4) rest:vec3<f32>,
    @location(5) normal:vec3<f32>,
}
@vertex fn proxy_vs(@location(0) p:vec3<f32>,@location(1) color:vec4<f32>,@location(2) grain:f32, @location(3) normal:vec4<f32>,
    @location(5) translation:vec4<f32>,@location(6) rotation:vec4<f32>)->ProxyOut{
    let rest=yaw(p,rotation.xy)*translation.w+translation.xyz;
    var world=rest;
    if rotation.z>=0.0{world+=wind(rest,p,translation,true);}
    return ProxyOut(clip_position(world),color,world,grain*translation.w,rotation.zw,rest,yaw(normal.xyz,rotation.xy));
}
// Keep actual foliage coverage independent of the LOD thinning mask.
@fragment
fn proxy_fs(input:ProxyOut)->FragmentOut{
    let coverage=clamp(1.0-bitcast<f32>(cam.params.y)*(1.0-input.color.a),0.0,1.0);
    if coverage<0.9999{
        let p=input.rest/max(input.grain*bitcast<f32>(cam.params.z),0.01);
        let q=vec3<f32>(dot(p,vec3<f32>(0.8,0.0,0.6)),dot(p,vec3<f32>(-0.36,0.8,0.48)),dot(p,vec3<f32>(-0.48,-0.6,0.64)));
        if hash_cell(vec3<i32>(floor(q)))>=coverage{discard;}
    }
    var range=input.lod_range;
    if range.x<0.0{
        let t=clamp(range.y,0.0,1.0);
        var reveal=t;
        if t>0.0&&t<1.0{
            let p=input.rest/max(input.grain*0.7,0.05);
            let rank=0.7*input.color.a+0.3*hash_cell(vec3<i32>(floor(p))+vec3<i32>(17,43,79));
            reveal=clamp(t+(2.0*rank-1.0)*0.16*4.0*t*(1.0-t),0.0,1.0);
        }
        range=vec2<f32>(0.0,reveal);
    }
    let bits=sample_coverage(input.clip.xy,forest_coverage(range,input.world));
    if bits==0u{discard;}
    let n=normalize(input.normal);
    return fragment_out(vec4<f32>(light(input.color.rgb,n,input.world),1.0),bits);
}

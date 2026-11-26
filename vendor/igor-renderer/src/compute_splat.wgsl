struct Camera {
    origin:vec4<f32>, forward:vec4<f32>, right:vec4<f32>, up:vec4<f32>,
    screen:vec4<f32>, extent:vec4<f32>, hud:array<vec4<u32>,8>, params:vec4<u32>,
    effects:vec4<f32>, shadow_transform:vec4<f32>,
}
struct Point {xy:u32,zr:u32,rgba:u32,normal:u32}
struct Instance {origin_scale:vec4<f32>,offset:u32,turn:u32,pad:vec2<u32>,bounds:vec4<f32>}
@group(0) @binding(0) var<uniform> cam:Camera;
@group(0) @binding(1) var<storage,read> raw_points:array<Point>;
@group(0) @binding(2) var<storage,read> lod_points:array<Point>;
@group(0) @binding(3) var<storage,read> instances:array<Instance>;
struct SurfacePatch {
    plane: vec4<f32>,
    anchor: vec3<f32>, material: u32,
    grad_u: vec3<f32>, normal0: u32,
    grad_v: vec3<f32>, normal1: u32,
    uv01: vec4<f32>, uv2_normal_uv0: vec4<f32>, normal_uv12: vec4<f32>,
    normal2: u32, tangent: u32, bitangent: u32, has_tangent: u32,
}
struct Material { factor: vec4<f32>, base: vec4<u32>, normal: vec4<u32>, params: vec4<f32> }

@group(0) @binding(4) var<storage,read> surfaces:array<SurfacePatch>;
@group(0) @binding(5) var<storage,read> materials:array<Material>;
@group(0) @binding(6) var color_atlas:texture_2d_array<f32>;
@group(0) @binding(7) var normal_atlas:texture_2d_array<f32>;
@group(0) @binding(8) var atlas_sampler:sampler;

fn rotate(p:vec3<f32>,turn:u32)->vec3<f32>{
    switch turn {
        case 1u:{return vec3<f32>(p.z,p.y,-p.x);}
        case 2u:{return vec3<f32>(-p.x,p.y,-p.z);}
        case 3u:{return vec3<f32>(-p.z,p.y,p.x);}
        default:{return p;}
    }
}
fn normal_at(p:u32)->vec3<f32>{
    let xy=unpack2x16snorm(p);
    let z=1.0-abs(xy.x)-abs(xy.y);
    let fold=max(-z,0.0);
    return normalize(vec3<f32>(xy-select(vec2<f32>(fold),vec2<f32>(-fold),xy<vec2<f32>(0.0)),z));
}
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

fn base_color(mat:Material, surface:SurfacePatch, b:vec2<f32>)->vec4<f32> {
    if mat.base.x == 0u { return mat.factor; }
    let uv = surface.uv01.xy + surface.uv01.zw * b.x + surface.uv2_normal_uv0.xy * b.y;
    return mat.factor * textureSampleLevel(color_atlas, atlas_sampler, atlas_uv(mat.base, uv), i32(mat.base.w & 65535u), 0.0);
}

fn surface_normal(mat:Material, surface:SurfacePatch, b:vec2<f32>)->vec3<f32> {
    let n = normalize(normal_at(surface.normal0) * (1.0 - b.x - b.y)
         + normal_at(surface.normal1) * b.x + normal_at(surface.normal2) * b.y);
    if mat.normal.x == 0u || surface.has_tangent == 0u { return n; }
    let u = normal_at(surface.tangent);
    let t = normalize(u - n * dot(n, u));
    let cross_nt = cross(n, t);
    let bitangent = cross_nt * sign(dot(cross_nt, normal_at(surface.bitangent)));
    let uv = surface.uv2_normal_uv0.zw + surface.normal_uv12.xy * b.x + surface.normal_uv12.zw * b.y;
    let mapped = textureSampleLevel(normal_atlas, atlas_sampler, atlas_uv(mat.normal, uv), i32(mat.normal.w & 65535u), 0.0).rgb * 2.0 - 1.0;
    return normalize((t * mapped.x + bitangent * mapped.y) * mat.params.y + n * mapped.z);
}

fn clip_position(world:vec3<f32>)->vec4<f32>{
    let rel=world-cam.origin.xyz;
    let z=dot(rel,cam.forward.xyz);
    let near=0.05;
    let far=cam.extent.w;
    return vec4<f32>(dot(rel,cam.right.xyz)*2.0/(cam.screen.x*cam.screen.z),
                     dot(rel,cam.up.xyz)*2.0/(cam.screen.y*cam.screen.z),
                     (z-near)*far/(far-near),z);
}
fn sun_visibility(world:vec3<f32>)->f32{
    if (render_flags()&32u)==0u{return 1.0;}
    // Project along the same fixed sun direction as the startup field.
    let uv=(world.xz+world.y*vec2<f32>(0.625,0.5)+cam.shadow_transform.xy)*cam.shadow_transform.zw;
    if any(uv<vec2<f32>(0.0))||any(uv>vec2<f32>(1.0)){return 1.0;}
    let opacity=textureSampleLevel(canopy_shadow,atlas_sampler,uv,0.0);
    // Only canopy bands above the receiving surface attenuate sunlight.
    // Smooth band weights suppress seams and most same-height self-shadow.
    let above=smoothstep(vec4<f32>(0.5),vec4<f32>(4.5),vec4<f32>(2.0,6.0,10.0,14.0)-world.y);
    let t=vec4<f32>(1.0)-opacity*above*cam.effects.z;
    return max(0.12,t.x*t.y*t.z*t.w);
}
fn light(c:vec3<f32>,n:vec3<f32>,world:vec3<f32>)->vec3<f32>{
    let sun=normalize(vec3<f32>(-0.5,0.8,-0.4));
    let dist=length(world-cam.origin.xyz);
    return mix(c*(0.36+0.64*max(dot(n,sun),0.0)*sun_visibility(world)),vec3<f32>(0.51,0.66,0.78),1.0-exp(-dist*0.00045));
}

struct Job { instance_id:u32, first:u32, count:u32, padding:u32 }
@group(0) @binding(9) var<storage,read> jobs:array<Job>;
// Two independent atomic arrays in one binding: positive float depth bits,
// then winner ID. Separate passes make depth/owner publication race-free.
@group(0) @binding(10) var<storage,read_write> screen_words:array<atomic<u32>>;
@group(0) @binding(11) var output_image:texture_storage_2d<rgba8unorm,write>;
@group(0) @binding(12) var<storage,read_write> projection_cache:array<vec2<u32>>;
@group(0) @binding(13) var canopy_shadow:texture_2d<f32>;
const TINY_FLAG:u32=0x80000000u;
fn render_flags()->u32{return u32(cam.screen.w);}
fn cache_enabled()->bool{return (render_flags()&4u)!=0u;}
fn pack_xy(q:vec2<i32>)->u32{return u32(q.x)|(u32(q.y)<<16u);}
fn unpack_xy(q:u32)->vec2<i32>{return vec2<i32>(i32(q&65535u),i32((q>>16u)&32767u));}
const EMPTY:u32=0xffffffffu;
const NEAR:f32=0.05;
const JOB_WIDTH:u32=1024u;
const LANES:u32=64u;
struct Projected {
    c:vec3<f32>, a:vec3<f32>, b:vec3<f32>,
    local_center:vec3<f32>, local_a:vec3<f32>, local_b:vec3<f32>,
    inst:Instance, point:Point, detailed:bool, valid:bool,
    lo:vec2<i32>, hi:vec2<i32>, tiny:bool,
}
fn dimensions()->vec2<u32>{return vec2<u32>(cam.screen.xy);}
fn pixel_count()->u32{let s=dimensions();return s.x*s.y;}
fn pixel_id(p:vec2<u32>)->u32{return p.x+p.y*dimensions().x;}
// Packed max-depth pyramid: 4,16,64,256 pixel tiles. EMPTY is larger
// than every finite positive depth, so holes can never occlude geometry.
fn level_side(level:u32)->u32{return 1u<<(2u*level);}
fn level_dims(level:u32)->vec2<u32>{let s=level_side(level);return (dimensions()+vec2<u32>(s-1u))/s;}
fn level_offset(level:u32)->u32{
    if level==0u{return 0u;}
    var offset=2u*pixel_count();
    for(var k=1u;k<level;k++){let d=level_dims(k);offset+=d.x*d.y;}
    return offset;
}
fn stats_offset()->u32{return level_offset(5u);}
fn flag_offset()->u32{return stats_offset()+4u;}
fn flag_at(i:u32)->u32{return atomicLoad(&screen_words[flag_offset()+i]);}
fn hiz_read(level:u32,p:vec2<u32>)->u32{
    return atomicLoad(&screen_words[level_offset(level)+p.x+p.y*level_dims(level).x]);
}
fn build_hiz(level:u32,id:vec2<u32>){
    let out_dims=level_dims(level);
    if any(id>=out_dims){return;}
    let in_dims=level_dims(level-1u);
    var farthest=0u;
    for(var y=0u;y<4u;y++){for(var x=0u;x<4u;x++){
        let q=id*4u+vec2<u32>(x,y);
        if all(q<in_dims){farthest=max(farthest,hiz_read(level-1u,q));}
    }}
    atomicStore(&screen_words[level_offset(level)+id.x+id.y*out_dims.x],farthest);
}
@compute @workgroup_size(8,8,1) fn hiz1_cs(@builtin(global_invocation_id) id:vec3<u32>){build_hiz(1u,id.xy);}
@compute @workgroup_size(8,8,1) fn hiz2_cs(@builtin(global_invocation_id) id:vec3<u32>){build_hiz(2u,id.xy);}
@compute @workgroup_size(8,8,1) fn hiz3_cs(@builtin(global_invocation_id) id:vec3<u32>){build_hiz(3u,id.xy);}
@compute @workgroup_size(8,8,1) fn hiz4_cs(@builtin(global_invocation_id) id:vec3<u32>){build_hiz(4u,id.xy);}
fn instance_id(g:vec3<u32>,lane:u32)->u32{return (g.x+g.y*JOB_WIDTH)*LANES+lane;}

fn camera_vector(v:vec3<f32>)->vec3<f32>{
    return vec3<f32>(dot(v,cam.right.xyz),-dot(v,cam.up.xyz),dot(v,cam.forward.xyz));
}
fn jitter()->vec2<f32>{return vec2<f32>(cam.right.w,cam.up.w);}
fn screen_pos(v:vec3<f32>)->vec2<f32>{return cam.screen.xy*0.5+jitter()+v.xy/(v.z*cam.screen.z);}
@compute @workgroup_size(64,1,1)
fn classify_cs(@builtin(workgroup_id) g:vec3<u32>,@builtin(local_invocation_index) lane:u32){
    let i=instance_id(g,lane);
    if i>=cam.params.z{return;}
    let inst=instances[i];
    let c=camera_vector(inst.bounds.xyz-cam.origin.xyz);
    let is_near=c.z-inst.bounds.w<=bitcast<f32>(cam.params.w);
    atomicStore(&screen_words[flag_offset()+i],select(0u,1u,is_near));
    if is_near {
        atomicAdd(&screen_words[stats_offset()],1u);
        atomicAdd(&screen_words[stats_offset()+1u],inst.pad.y);
    }
}
// Conservative ratio interval for the projected enclosing box.
fn ratio_bounds(x:f32,r:f32,z:f32)->vec2<f32>{
    let a=(x-r)/(z-r);let b=(x-r)/(z+r);
    let c=(x+r)/(z-r);let d=(x+r)/(z+r);
    return vec2<f32>(min(min(a,b),min(c,d)),max(max(a,b),max(c,d)));
}
fn is_occluded(inst:Instance)->bool {
    let c=camera_vector(inst.bounds.xyz-cam.origin.xyz);
    let r=inst.bounds.w+select(0.0,0.35*cam.effects.y,(render_flags()&16u)!=0u);
    // Near clipping or uncertain bounds always keep the instance.
    if r<=0.0||c.z-r<=NEAR{return false;}
    let x=ratio_bounds(c.x,r,c.z);
    let y=ratio_bounds(c.y,r,c.z);
    let lo_f=cam.screen.xy*0.5+vec2<f32>(x.x,y.x)/cam.screen.z-1.0;
    let hi_f=cam.screen.xy*0.5+vec2<f32>(x.y,y.y)/cam.screen.z+1.0;
    let lo=vec2<u32>(clamp(floor(lo_f),vec2<f32>(0.0),cam.screen.xy));
    let hi=vec2<u32>(clamp(ceil(hi_f),vec2<f32>(0.0),cam.screen.xy));
    if any(hi<=lo){return false;}
    let min_depth=c.z-r-max(0.02,c.z*0.0001);
    if min_depth<=NEAR{return false;}
    let key=bitcast<u32>(min_depth);
    // Pick the finest level that covers this rectangle in <=64 tiles.
    // Retry at finer levels when cheap; a tile including any hole fails.
    var level=0u;
    for(var k=0u;k<4u;k++){
        let step=level_side(level);
        let a=lo/step;let b=(hi-vec2<u32>(1u))/step;
        let size=b-a+vec2<u32>(1u);
        if size.x*size.y<=64u{break;}
        level+=1u;
    }
    let step=level_side(level);
    let a=lo/step;let b=(hi-vec2<u32>(1u))/step;
    let size=b-a+vec2<u32>(1u);
    if size.x*size.y>64u{return false;}
    for(var yy=a.y;yy<=b.y;yy++){for(var xx=a.x;xx<=b.x;xx++){
        if hiz_read(level,vec2<u32>(xx,yy))>=key{return false;}
    }}
    return true;
}
@compute @workgroup_size(64,1,1)
fn cull_cs(@builtin(workgroup_id) g:vec3<u32>,@builtin(local_invocation_index) lane:u32){
    let i=instance_id(g,lane);
    if i>=cam.params.z||flag_at(i)!=0u{return;}
    let inst=instances[i];
    if is_occluded(inst){
        atomicStore(&screen_words[flag_offset()+i],2u);
        atomicAdd(&screen_words[stats_offset()+2u],1u);
        atomicAdd(&screen_words[stats_offset()+3u],inst.pad.y);
    }
}
// Coherent crown sway plus small spatially correlated leaf flutter.
// Translate each disk in world space while preserving source UVs and
// patch clipping in its original local coordinates.
fn wind_offset(inst:Instance,local:vec3<f32>,pt:Point,detailed:bool)->vec3<f32>{
    if (render_flags()&16u)==0u||inst.pad.x==1u{return vec3<f32>(0.0);}
    let world=inst.origin_scale.xyz+rotate(local,inst.turn)*inst.origin_scale.w;
    let rel=world-cam.origin.xyz;
    let dist2=dot(rel,rel);
    if dist2>=19600.0{return vec3<f32>(0.0);}
    let fade=1.0-smoothstep(6400.0,19600.0,dist2);
    let h=max(0.0,local.y*inst.origin_scale.w);
    let bend=min(h*h*0.0015,0.24);
    let phase=dot(inst.origin_scale.xz,vec2<f32>(0.035,0.027));
    let t=cam.effects.x;
    let sway=sin(t*1.65+phase)*0.72+sin(t*0.61+phase*0.61)*0.28;
    var leaf=smoothstep(0.8,3.0,h);
    if detailed{
        let surface=surfaces[pt.rgba];
        if materials[surface.material].params.x<0.0{leaf=0.0;}
    }
    // Flutter is correlated in space, not random per frame or point ID.
    let ripple=sin(t*6.2+dot(world,vec3<f32>(2.1,1.7,1.3)));
    let flutter=0.028*leaf*ripple;
    return vec3<f32>(bend*sway+flutter,flutter*0.28,bend*sway*0.48-flutter*0.7)*fade*cam.effects.y;
}
fn tangent_for(n:vec3<f32>)->vec3<f32>{
    return normalize(cross(n,select(vec3<f32>(0.0,1.0,0.0),vec3<f32>(1.0,0.0,0.0),abs(n.y)>0.9)));
}
// cached.x = EMPTY: fresh projection; bit31: accepted tiny; otherwise packed bbox.
fn project_point(j:Job,lane:u32,cached:vec2<u32>)->Projected {
    let inst=instances[j.instance_id];
    let index=(inst.offset&0x7fffffffu)+j.first+lane;
    let detailed=(inst.offset&0x80000000u)!=0u;
    var pt:Point;
    if detailed{pt=raw_points[index];}else{pt=lod_points[index];}
    var local=vec3<f32>(f32(pt.xy&65535u),f32(pt.xy>>16u),f32(pt.zr&65535u))/4096.0;
    var n=vec3<f32>(0.0,1.0,0.0);
    var radius=f32(pt.zr>>16u)/4096.0;
    if detailed {
        let surface=surfaces[pt.rgba];
        n=surface.plane.xyz;
        local+=n*(surface.plane.w-dot(n,local));
    }else{
        radius*=sqrt(unpack4x8unorm(pt.rgba).a);
    }
    var p=Projected(vec3<f32>(0.0),vec3<f32>(0.0),vec3<f32>(0.0),local,vec3<f32>(0.0),vec3<f32>(0.0),inst,pt,detailed,false,vec2<i32>(0),vec2<i32>(0),false);
    if cached.x!=EMPTY && (cached.x&TINY_FLAG)!=0u {
        // The depth pass has already projected and alpha-tested this center.
        p.c.z=bitcast<f32>(cached.y);p.lo=unpack_xy(cached.x);p.hi=p.lo+1;
        p.tiny=true;p.valid=true;return p;
    }
    let c=camera_vector(inst.origin_scale.xyz+rotate(local,inst.turn)*inst.origin_scale.w+wind_offset(inst,local,pt,detailed)-cam.origin.xyz);
    p.c=c;
    let wr=radius*abs(inst.origin_scale.w);
    if radius<=0.0||c.z+wr<=NEAR{return p;}
    if cached.x==EMPTY && (render_flags()&2u)!=0u && c.z-wr>NEAR {
        // Conservative sphere projection bound. No normal decode, tangents,
        // corner projection or homography for the single-pixel case.
        let shift=abs(c.xy)/c.z;
        let half_span=wr*(vec2<f32>(1.0)+shift)/((c.z-wr)*cam.screen.z);
        if max(half_span.x,half_span.y)<=0.5{
            let q=vec2<i32>(floor(screen_pos(c)));
            if all(q>=vec2<i32>(0))&&all(q<vec2<i32>(dimensions())){
                p.lo=q;p.hi=q+1;p.valid=true;p.tiny=true;
            }
            return p;
        }
    }
    if !detailed{n=normal_at(pt.normal);}
    let tangent=tangent_for(n);
    let a_local=tangent*radius;
    let b_local=cross(n,tangent)*radius;
    let a=camera_vector(rotate(a_local,inst.turn)*inst.origin_scale.w);
    let b=camera_vector(rotate(b_local,inst.turn)*inst.origin_scale.w);
    p.a=a;p.b=b;p.local_a=a_local;p.local_b=b_local;
    if cached.x!=EMPTY{
        p.lo=unpack_xy(cached.x);p.hi=unpack_xy(cached.y);p.valid=true;
        return p;
    }
    if c.z+abs(a.z)+abs(b.z)<=NEAR{return p;}
    // Project the bounding square of the fixed 3D disk, clipping its edges
    // against the near plane before division. No generated triangles.
    var corners=array<vec3<f32>,4>(c-a-b,c+a-b,c+a+b,c-a+b);
    var mn=vec2<f32>(1e20);
    var mx=vec2<f32>(-1e20);
    for(var k=0u;k<4u;k++){
        let v=corners[k];
        let w=corners[(k+1u)%4u];
        if v.z>=NEAR {let q=screen_pos(v);mn=min(mn,q);mx=max(mx,q);}
        if (v.z>=NEAR)!=(w.z>=NEAR) {
            let q=screen_pos(mix(v,w,(NEAR-v.z)/(w.z-v.z)));
            mn=min(mn,q);mx=max(mx,q);
        }
    }
    if any(mx<vec2<f32>(0.0))||any(mn>=cam.screen.xy){return p;}
    // Subpixel point: deposit in one pixel. This is coverage approximation,
    // not a camera-facing disk. Larger splats retain perspective and depth.
    if c.z>=NEAR && max(mx.x-mn.x,mx.y-mn.y)<1.0 {
        let q=vec2<i32>(floor(screen_pos(c)));
        if all(q>=vec2<i32>(0))&&all(q<vec2<i32>(dimensions())){
            p.lo=q;p.hi=q+vec2<i32>(1);p.valid=true;p.tiny=true;
        }
        return p;
    }
    // Clip to framebuffer. Limit exceptional near-camera work per splat:
    // large footprints are sampled in a centered 96x96 window (documented).
    let low=max(floor(mn),vec2<f32>(0.0));
    let high=min(ceil(mx),cam.screen.xy);
    let mid=(low+high)*0.5;
    p.lo=vec2<i32>(max(low,floor(mid-48.0)));
    p.hi=vec2<i32>(min(high,ceil(mid+48.0)));
    p.valid=all(p.hi>p.lo);
    return p;
}
struct Sample {depth:f32,uv:vec2<f32>,bary:vec2<f32>,valid:bool}
// phase: 0 = resolve, 1 = depth/prepass, 2 = owner
fn sample_pixel(p:Projected,pixel:vec2<u32>,phase:u32)->Sample {
    var s=Sample(0.0,vec2<f32>(0.0),vec2<f32>(0.0),false);
    if !p.valid{return s;}
    var uv=vec2<f32>(0.0);
    if !p.tiny {
        // Invert the projected disk's 2D homography at this pixel.
        // This is a coverage test of a known projected primitive, NOT a
        // spatial search, ray traversal or ray/scene intersection.
        let q=(vec2<f32>(pixel)+vec2<f32>(0.5)-cam.screen.xy*0.5-jitter())*cam.screen.z;
        let a=p.a.xy-q*p.a.z;
        let b=p.b.xy-q*p.b.z;
        let rhs=q*p.c.z-p.c.xy;
        let det=a.x*b.y-a.y*b.x;
        if abs(det)<1e-12{return s;}
        uv=vec2<f32>(rhs.x*b.y-rhs.y*b.x,a.x*rhs.y-a.y*rhs.x)/det;
        if dot(uv,uv)>1.0{return s;}
    }
    let depth=p.c.z+p.a.z*uv.x+p.b.z*uv.y;
    if depth<NEAR||depth>cam.extent.w{return s;}
    // Depth only decreases during depth passes. Reject a candidate behind
    // a published alpha-tested depth; a concurrent closer write is safe.
    // In the owner pass the final depth is immutable and equality is
    // mandatory. Do this before touching patch barycentrics/material/atlas.
    if (render_flags()&1u)!=0u && phase!=0u{
        let known=atomicLoad(&screen_words[pixel_id(pixel)]);
        let key=bitcast<u32>(depth);
        if phase==1u && key>known{return s;}
        if phase==2u && key!=known{return s;}
    }
    var bary=vec2<f32>(0.0);
    if p.detailed {
        let surface=surfaces[p.point.rgba];
        let pos=p.local_center+p.local_a*uv.x+p.local_b*uv.y;
        let rel=pos-surface.anchor;
        bary=vec2<f32>(dot(rel,surface.grad_u),dot(rel,surface.grad_v));
        if min(bary.x,bary.y)<-0.00005||bary.x+bary.y>1.00005{return s;}
        let mat=materials[surface.material];
        if mat.params.x>=0.0 && base_color(mat,surface,bary).a<mat.params.x{return s;}
    }
    return Sample(depth,uv,bary,true);
}
fn paint(j:Job,lane:u32,id:u32,owner_pass:bool){
    if lane>=j.count{return;}
    var cached=vec2<u32>(EMPTY);
    if owner_pass && cache_enabled(){
        cached=projection_cache[id];
        if cached.x==EMPTY{return;}
        if (cached.x&TINY_FLAG)!=0u{
            let i=pixel_id(vec2<u32>(unpack_xy(cached.x)));
            if atomicLoad(&screen_words[i])==cached.y{atomicMin(&screen_words[pixel_count()+i],id);}
            return;
        }
    }
    let p=project_point(j,lane,cached);
    if !owner_pass && cache_enabled(){projection_cache[id]=vec2<u32>(EMPTY);}
    if !p.valid{return;}
    let n=pixel_count();
    if p.tiny{
        let pixel=vec2<u32>(p.lo);
        let s=sample_pixel(p,pixel,select(1u,2u,owner_pass));
        if !s.valid{return;}
        let key=bitcast<u32>(s.depth);
        let i=pixel_id(pixel);
        if owner_pass{
            if atomicLoad(&screen_words[i])==key{atomicMin(&screen_words[n+i],id);}
        }else{
            if cache_enabled(){projection_cache[id]=vec2<u32>(pack_xy(p.lo)|TINY_FLAG,key);}
            atomicMin(&screen_words[i],key);
        }
        return;
    }
    if !owner_pass && cache_enabled(){projection_cache[id]=vec2<u32>(pack_xy(p.lo),pack_xy(p.hi));}
    for(var y=p.lo.y;y<p.hi.y;y++){
        for(var x=p.lo.x;x<p.hi.x;x++){
            let pixel=vec2<u32>(u32(x),u32(y));
            let s=sample_pixel(p,pixel,select(1u,2u,owner_pass));
            if !s.valid{continue;}
            let key=bitcast<u32>(s.depth);
            let i=pixel_id(pixel);
            if owner_pass {
                if atomicLoad(&screen_words[i])==key {atomicMin(&screen_words[n+i],id);}
            }else{
                atomicMin(&screen_words[i],key);
            }
        }
    }
}
@compute @workgroup_size(8,8,1)
fn clear_cs(@builtin(global_invocation_id) id:vec3<u32>){
    if any(id.xy>=dimensions()){return;}
    let i=pixel_id(id.xy);
    atomicStore(&screen_words[i],EMPTY);
    atomicStore(&screen_words[pixel_count()+i],EMPTY);
    if i==0u {for(var k=0u;k<4u;k++){atomicStore(&screen_words[stats_offset()+k],0u);}}
}
@compute @workgroup_size(64,1,1)
fn depth_cs(@builtin(workgroup_id) g:vec3<u32>,@builtin(local_invocation_index) lane:u32){
    let j=g.x+g.y*JOB_WIDTH;
    if j>=cam.params.x{return;}
    let job=jobs[j];
    if cam.params.y!=0u && flag_at(job.instance_id)!=0u{return;}
    paint(job,lane,j*LANES+lane,false);
}
@compute @workgroup_size(64,1,1)
fn prepass_cs(@builtin(workgroup_id) g:vec3<u32>,@builtin(local_invocation_index) lane:u32){
    let j=g.x+g.y*JOB_WIDTH;
    if j>=cam.params.x{return;}
    let job=jobs[j];
    if flag_at(job.instance_id)!=1u{return;}
    paint(job,lane,j*LANES+lane,false);
}
@compute @workgroup_size(64,1,1)
fn owner_cs(@builtin(workgroup_id) g:vec3<u32>,@builtin(local_invocation_index) lane:u32){
    let j=g.x+g.y*JOB_WIDTH;
    if j>=cam.params.x{return;}
    let job=jobs[j];
    if cam.params.y!=0u && flag_at(job.instance_id)==2u{return;}
    paint(job,lane,j*LANES+lane,true);
}
fn color_at(p:Projected,s:Sample)->vec3<f32>{
    let local=p.local_center+p.local_a*s.uv.x+p.local_b*s.uv.y;
    let world=p.inst.origin_scale.xyz+rotate(local,p.inst.turn)*p.inst.origin_scale.w+wind_offset(p.inst,p.local_center,p.point,p.detailed);
    var color=unpack4x8unorm(p.point.rgba).rgb;
    var n=rotate(normal_at(p.point.normal),p.inst.turn);
    if p.detailed{
        let surface=surfaces[p.point.rgba];
        let mat=materials[surface.material];
        color=base_color(mat,surface,s.bary).rgb;
        n=rotate(surface_normal(mat,surface,s.bary),p.inst.turn);
        if dot(n,cam.origin.xyz-world)<0.0 {n=-n;}
    }
    return light(color,n,world);
}
@compute @workgroup_size(8,8,1)
fn resolve_cs(@builtin(global_invocation_id) id:vec3<u32>){
    if any(id.xy>=dimensions()){return;}
    let i=pixel_id(id.xy);
    let winner=atomicLoad(&screen_words[pixel_count()+i]);
    var z=1e30;
    var color=vec3<f32>(0.0);
    if winner!=EMPTY {
        var cached=vec2<u32>(EMPTY);
        if cache_enabled(){cached=projection_cache[winner];}
        let p=project_point(jobs[winner/LANES],winner%LANES,cached);
        let s=sample_pixel(p,id.xy,0u);
        if s.valid{color=color_at(p,s);z=s.depth;}
    }
    // Background plane has a closed-form screen projection; no geometry
    // traversal, no rays through the forest, no ground triangles.
    let q=(vec2<f32>(id.xy)+0.5-cam.screen.xy*0.5-jitter())*cam.screen.z;
    let basis=cam.forward.xyz+cam.right.xyz*q.x-cam.up.xyz*q.y;
    if z==1e30 {
        color=mix(vec3<f32>(0.65,0.76,0.87),vec3<f32>(0.23,0.47,0.75),max(normalize(basis).y,0.0));
    }
    if abs(basis.y)>1e-8 {
        let ground_z=-cam.origin.y/basis.y;
        let world=cam.origin.xyz+basis*ground_z;
        if ground_z>=NEAR&&ground_z<z&&world.x>=0.0&&world.z>=0.0&&world.x<=cam.extent.x&&world.z<=cam.extent.z {
            color=light(vec3<f32>(0.16,0.24,0.09),vec3<f32>(0.0,1.0,0.0),world);
            z=ground_z;
        }
    }
    if (render_flags()&8u)!=0u{atomicStore(&screen_words[i],bitcast<u32>(select(z,0.0,z==1e30)));}
    textureStore(output_image,vec2<i32>(id.xy),vec4<f32>(color,1.0));
}

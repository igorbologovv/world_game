struct Camera {
    origin:vec4<f32>, forward:vec4<f32>, right:vec4<f32>, up:vec4<f32>,
    screen:vec4<f32>, extent:vec4<f32>, hud:array<vec4<u32>,8>,
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
fn light(c:vec3<f32>,n:vec3<f32>,dist:f32)->vec3<f32>{
    let sun=normalize(vec3<f32>(-0.5,0.8,-0.4));
    return mix(c*(0.36+0.64*max(dot(n,sun),0.0)),vec3<f32>(0.51,0.66,0.78),1.0-exp(-dist*0.00045));
}
fn corner(i:u32)->vec2<f32>{
    switch i {
        case 0u:{return vec2<f32>(-1.0,-1.0);}
        case 1u:{return vec2<f32>(1.0,-1.0);}
        case 2u:{return vec2<f32>(-1.0,1.0);}
        case 3u:{return vec2<f32>(-1.0,1.0);}
        case 4u:{return vec2<f32>(1.0,-1.0);}
        default:{return vec2<f32>(1.0,1.0);}
    }
}
struct SplatOut {
    @builtin(position) pos:vec4<f32>,
    @location(0) uv:vec2<f32>,
    @location(1) @interpolate(flat) rgb:vec3<f32>,
    @location(2) local_pos:vec3<f32>,
    @location(3) world_pos:vec3<f32>,
    @location(4) @interpolate(flat) surface_id:u32,
    @location(5) @interpolate(flat) turn:u32,
}
fn tangent_for(n:vec3<f32>)->vec3<f32>{
    return normalize(cross(n,select(vec3<f32>(0.0,1.0,0.0),vec3<f32>(1.0,0.0,0.0),abs(n.y)>0.9)));
}
@vertex fn splat_vs(@builtin(vertex_index) v:u32,@builtin(instance_index) i:u32)->SplatOut {
    let inst=instances[i];
    var point:Point;
    let offset=inst.offset&0x7fffffffu;
    let detailed=(inst.offset&0x80000000u)!=0u;
    if detailed {point=raw_points[offset+v/6u];}
    else{point=lod_points[offset+v/6u];}
    let p=vec3<f32>(f32(point.xy&65535u),f32(point.xy>>16u),f32(point.zr&65535u))/4096.0;
    let uv=corner(v%6u);
    let radius=f32(point.zr>>16u)/4096.0;
    if detailed {
        // True surface-oriented disk. Project vertices directly; hardware
        // interpolates local coordinates and depth perspectively.
        let surface=surfaces[point.rgba];
        let n=surface.plane.xyz;
        let t=tangent_for(n);
        let bt=cross(n,t);
        let center=p+n*(surface.plane.w-dot(n,p));
        let local=center+radius*(t*uv.x+bt*uv.y);
        let world=inst.origin_scale.xyz+rotate(local,inst.turn)*inst.origin_scale.w;
        return SplatOut(clip_position(world),uv,vec3<f32>(0.0),local,world,point.rgba,inst.turn);
    }
    let world=inst.origin_scale.xyz+rotate(p,inst.turn)*inst.origin_scale.w;
    let pos=clip_position(world);
    if pos.w<=0.05 {
        return SplatOut(vec4<f32>(2.0,2.0,2.0,1.0),uv,vec3<f32>(0.0),p,world,0xffffffffu,0u);
    }
    let rgba=unpack4x8unorm(point.rgba);
    let n=rotate(normal_at(point.normal),inst.turn);
    let world_radius=radius*inst.origin_scale.w*sqrt(rgba.a);
    let pixel_radius=clamp(world_radius/(pos.w*cam.screen.z),0.55,128.0);
    var projected=pos;
    if inst.pad.x!=0u || pixel_radius<1.0 {
        // World-level metadata and subpixel points use a screen footprint.
        projected.x+=uv.x*pixel_radius*2.0/cam.screen.x*pos.w;
        projected.y+=uv.y*pixel_radius*2.0/cam.screen.y*pos.w;
    }else{
        // Coarse local cells approximate a surface, not a facing circle.
        // Blend toward the view normal slightly to avoid edge-on holes.
        let view_n=normalize(cam.origin.xyz-world);
        let facing=select(n,-n,dot(n,view_n)<0.0);
        let plane_n=normalize(mix(facing,view_n,0.30));
        let t=tangent_for(plane_n);
        let bt=cross(plane_n,t);
        let r=pixel_radius*pos.w*cam.screen.z;
        projected=clip_position(world+r*(t*uv.x+bt*uv.y));
    }
    return SplatOut(projected,uv,light(rgba.rgb,n,length(world-cam.origin.xyz)),p,world,0xffffffffu,0u);
}
@fragment fn splat_fs(i:SplatOut)->@location(0) vec4<f32>{
    if dot(i.uv,i.uv)>1.0{discard;}
    if i.surface_id==0xffffffffu {return vec4<f32>(i.rgb,1.0);}
    let surface=surfaces[i.surface_id];
    let rel=i.local_pos-surface.anchor;
    let b=vec2<f32>(dot(rel,surface.grad_u),dot(rel,surface.grad_v));
    if min(b.x,b.y)<-0.00005 || b.x+b.y>1.00005 {discard;}
    let mat=materials[surface.material];
    let rgba=base_color(mat,surface,b);
    if mat.params.x>=0.0 && rgba.a<mat.params.x {discard;}
    var n=rotate(surface_normal(mat,surface,b),i.turn);
    if dot(n,cam.origin.xyz-i.world_pos)<0.0 {n=-n;}
    return vec4<f32>(light(rgba.rgb,n,length(i.world_pos-cam.origin.xyz)),1.0);
}
@vertex fn fullscreen_vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32>{
    return vec4<f32>(f32((i<<1u)&2u)*2.0-1.0,f32(i&2u)*2.0-1.0,0.0,1.0);
}
@fragment fn sky_fs(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32>{
    let y=clamp(cam.forward.y+(cam.screen.y*0.5-p.y)*cam.screen.z,0.0,1.0);
    return vec4<f32>(mix(vec3<f32>(0.65,0.76,0.87),vec3<f32>(0.23,0.47,0.75),y),1.0);
}
struct GroundOut {@builtin(position) pos:vec4<f32>,@location(0) world:vec3<f32>}
@vertex fn ground_vs(@builtin(vertex_index) i:u32)->GroundOut{
    let uv=(corner(i)+vec2<f32>(1.0))*0.5;
    let world=vec3<f32>(uv.x*cam.extent.x,0.0,uv.y*cam.extent.z);
    return GroundOut(clip_position(world),world);
}
@fragment fn ground_fs(i:GroundOut)->@location(0) vec4<f32>{
    return vec4<f32>(light(vec3<f32>(0.16,0.24,0.09),vec3<f32>(0.0,1.0,0.0),length(i.world-cam.origin.xyz)),1.0);
}
@fragment fn hud_fs(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32>{
    let hud_scale=select(1,2,cam.screen.x>=186.0&&cam.screen.y>=52.0);
    let hud_pixel=vec2<i32>(p.xy)/hud_scale;
    if hud_pixel.x<5||hud_pixel.y<5||hud_pixel.x>=93||hud_pixel.y>=26{discard;}
    let q=hud_pixel-vec2<i32>(9,8);
    if q.x>=0&&q.y>=0 {
        let digit=q.x/5;let line=q.y/8;let gx=q.x%5;let gy=q.y%8;
        if digit<16&&line<2&&gx<4&&gy<6 {
            let i=u32(line*16+digit);
            let glyph=cam.hud[i/4u][i%4u];
            let bit=u32(23-gy*4-gx);
            if ((glyph>>bit)&1u)!=0u {return vec4<f32>(0.88,0.94,0.90,1.0);}
        }
    }
    return vec4<f32>(0.014,0.022,0.019,1.0);
}

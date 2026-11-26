// Direct projection of oriented 3D point disks into Bevy's camera.
// Reverse-Z keys: positive finite f32 bits have the same ordering as depth.
struct Frame { clip:mat4x4<f32>, viewport:vec4<u32>, params:vec4<u32>, sun_time:vec4<f32>, sunlight:vec4<f32> }
struct Tree { origin_scale:vec4<f32>, rotation:vec4<f32>, range:vec4<u32> }
@group(0) @binding(0) var<uniform> cam:Frame;
@group(0) @binding(1) var<storage,read> points:array<vec4<u32>>;
@group(0) @binding(2) var<storage,read> trees:array<Tree>;
@group(0) @binding(3) var<storage,read> jobs:array<vec4<u32>>;
@group(0) @binding(4) var<storage,read_write> screen:array<atomic<u32>>;
@group(0) @binding(5) var<storage,read_write> projection_cache:array<vec2<u32>>;
const TINY_FLAG:u32=0x80000000u;
const INVALID:u32=0xffffffffu;
fn pack_xy(v:vec2<i32>)->u32{return u32(v.x)|(u32(v.y)<<16u);}
fn unpack_xy(v:u32)->vec2<i32>{return vec2<i32>(i32(v&65535u),i32((v>>16u)&32767u));}
fn in_dither(pixel:vec2<u32>,upper:u32)->bool{
    if upper>=12u{return true;}
    let absolute=vec2<f32>(pixel+cam.viewport.xy);
    let threshold=fract(52.9829189*fract(dot(absolute,vec2<f32>(0.06711056,0.00583715))));
    return threshold<f32(upper)/12.0;
}
const LANES:u32=64u;
fn dims()->vec2<u32>{return cam.viewport.zw;}
fn pixels()->u32{return dims().x*dims().y;}
fn index(q:vec2<u32>)->u32{return q.x+q.y*dims().x;}
fn signed16(v:u32)->f32{return f32(bitcast<i32>((v&65535u)<<16u)>>16u)/256.0;}
fn point_pos(p:vec4<u32>)->vec3<f32>{return vec3<f32>(signed16(p.x),signed16(p.x>>16u),signed16(p.y));}
fn normal(p:vec4<u32>)->vec3<f32>{
    let xy=vec2<f32>(f32(p.w&255u),f32((p.w>>8u)&255u))/127.5-1.0;
    let z=1.0-abs(xy.x)-abs(xy.y);
    let fold=max(-z,0.0);
    return normalize(vec3<f32>(xy-select(vec2<f32>(fold),vec2<f32>(-fold),xy<vec2<f32>(0.0)),z));
}
fn rotate(v:vec3<f32>,q:vec4<f32>)->vec3<f32>{let t=2.0*cross(q.xyz,v);return v+q.w*t+cross(q.xyz,t);}
fn wind(p:vec3<f32>,tree:Tree,leaf:bool)->vec3<f32>{
    let h=max(p.y*tree.origin_scale.w,0.0);
    let t=cam.sun_time.w;let phase=bitcast<f32>(tree.range.z);
    let sway=min(h*h*0.0015,0.24)*(0.72*sin(t*1.65+phase)+0.28*sin(t*0.61+phase*0.61));
    let flutter=select(0.0,0.028*sin(t*6.2+dot(p,vec3<f32>(2.1,1.7,1.3))),leaf)*smoothstep(0.8,3.0,h);
    let fade=1.0-smoothstep(80.0,140.0,length(tree.origin_scale.xyz));
    return vec3<f32>(sway+flutter,flutter*0.28,sway*0.48-flutter*0.7)*fade;
}
fn project(q:vec4<f32>)->vec2<f32>{return (q.xy/q.w*vec2<f32>(0.5,-0.5)+0.5)*vec2<f32>(dims());}
struct Projected { c:vec4<f32>,a:vec4<f32>,b:vec4<f32>,lo:vec2<i32>,hi:vec2<i32>,tiny:bool,valid:bool }
fn geometry(p:vec4<u32>,tree:Tree,cached:vec2<u32>)->Projected{
    let local=point_pos(p);
    let n=normal(p);
    let radius=f32(p.y>>16u)/4096.0*tree.origin_scale.w;
    let center=tree.origin_scale.xyz+rotate(local*tree.origin_scale.w+wind(local,tree,(p.w&65536u)!=0u),tree.rotation);
    let world_n=rotate(n,tree.rotation);
    let c=cam.clip*vec4<f32>(center,1.0);
    var result=Projected(c,vec4<f32>(0.0),vec4<f32>(0.0),vec2<i32>(0),vec2<i32>(0),false,false);
    if c.w<=0.05||c.z<=0.0||c.z>c.w{return result;}
    // Conservative sphere bound for tiny footprints.
    if cached.x==INVALID && c.w>radius+0.05 {
        let f=length(cam.clip[0].xyz)+length(cam.clip[1].xyz)+length(cam.clip[2].xyz);
        let span=radius*f*max(f32(dims().x),f32(dims().y))/(c.w-radius);
        if span<0.7 {
            let q=vec2<i32>(floor(project(c)));
            if all(q>=vec2<i32>(0))&&all(q<vec2<i32>(dims())){result.lo=q;result.hi=q+1;result.tiny=true;result.valid=true;}
            return result;
        }
    }
    let tangent=normalize(cross(world_n,select(vec3<f32>(0.0,1.0,0.0),vec3<f32>(1.0,0.0,0.0),abs(world_n.y)>0.9)));
    let a=cam.clip*vec4<f32>(tangent*radius,0.0);
    let b=cam.clip*vec4<f32>(cross(world_n,tangent)*radius,0.0);
    result.a=a;result.b=b;
    if cached.x!=INVALID {
        result.lo=unpack_xy(cached.x);result.hi=unpack_xy(cached.y);result.valid=true;return result;
    }
    var mn=vec2<f32>(1e20);var mx=vec2<f32>(-1e20);
    var corners=array<vec4<f32>,4>(c-a-b,c+a-b,c+a+b,c-a+b);
    for(var k=0u;k<4u;k++){
        let v=corners[k];let w=corners[(k+1u)%4u];
        if v.w>=0.05{let q=project(v);mn=min(mn,q);mx=max(mx,q);}
        if (v.w>=0.05)!=(w.w>=0.05){let q=project(mix(v,w,(0.05-v.w)/(w.w-v.w)));mn=min(mn,q);mx=max(mx,q);}
    }
    if any(mx<vec2<f32>(0.0))||any(mn>=vec2<f32>(dims())){return result;}
    if max(mx.x-mn.x,mx.y-mn.y)<1.0{
        let q=vec2<i32>(floor(project(c)));
        if all(q>=vec2<i32>(0))&&all(q<vec2<i32>(dims())){result.lo=q;result.hi=q+1;result.tiny=true;result.valid=true;}
        return result;
    }
    let lo=clamp(floor(mn),vec2<f32>(0.0),vec2<f32>(dims()));
    let hi=clamp(ceil(mx),vec2<f32>(0.0),vec2<f32>(dims()));
    let mid=(lo+hi)*0.5;
    result.lo=vec2<i32>(max(lo,floor(mid-48.0)));
    result.hi=vec2<i32>(min(hi,ceil(mid+48.0)));
    result.valid=all(result.hi>result.lo);return result;
}
fn coverage(p:Projected,pixel:vec2<u32>)->u32{
    var q=p.c;
    if !p.tiny {
        let ndc=(vec2<f32>(pixel)+0.5)/vec2<f32>(dims())*vec2<f32>(2.0,-2.0)+vec2<f32>(-1.0,1.0);
        let a=p.a.xy-ndc*p.a.w;let b=p.b.xy-ndc*p.b.w;
        let rhs=ndc*p.c.w-p.c.xy;
        let det=a.x*b.y-a.y*b.x;
        if abs(det)<1e-12{return 0u;}
        let uv=vec2<f32>(rhs.x*b.y-rhs.y*b.x,a.x*rhs.y-a.y*rhs.x)/det;
        if dot(uv,uv)>1.0{return 0u;}
        q+=p.a*uv.x+p.b*uv.y;
    }
    if q.w<=0.05||q.z<=0.0||q.z>q.w{return 0u;}
    return bitcast<u32>(q.z/q.w);
}
fn paint(g:vec3<u32>,lane:u32,owner:bool){
    let j=g.x+g.y*cam.params.y;
    if j>=cam.params.x{return;}
    let job=jobs[j];if lane>=job.z{return;}
    let slot=j*LANES+lane;
    let id=slot+1u;
    var cached=vec2<u32>(INVALID);
    if owner {
        cached=projection_cache[slot];
        if cached.x==INVALID{return;}
        if (cached.x&TINY_FLAG)!=0u{
            let i=index(vec2<u32>(unpack_xy(cached.x)));
            if atomicLoad(&screen[i])==cached.y{atomicMax(&screen[pixels()+i],id);}
            return;
        }
    }else{projection_cache[slot]=vec2<u32>(INVALID);}
    let tree=trees[job.x];let p=points[tree.range.x+job.y+lane];
    let projected=geometry(p,tree,cached);if !projected.valid{return;}
    if projected.tiny{
        let pixel=vec2<u32>(projected.lo);
        if !in_dither(pixel,job.w){return;}
        let key=coverage(projected,pixel);
        if key==0u{return;}
        let i=index(pixel);
        if owner {
            if atomicLoad(&screen[i])==key{atomicMax(&screen[pixels()+i],id);}
        }else{
            projection_cache[slot]=vec2<u32>(pack_xy(projected.lo)|TINY_FLAG,key);
            atomicMax(&screen[i],key);
        }
        return;
    }
    if !owner{projection_cache[slot]=vec2<u32>(pack_xy(projected.lo),pack_xy(projected.hi));}
    for(var y=projected.lo.y;y<projected.hi.y;y++){
        for(var x=projected.lo.x;x<projected.hi.x;x++){
            let pixel=vec2<u32>(u32(x),u32(y));
            if !in_dither(pixel,job.w){continue;}
            let i=index(pixel);let key=coverage(projected,pixel);
            if key==0u{continue;}
            let current=atomicLoad(&screen[i]);
            if owner{
                if current==key{atomicMax(&screen[pixels()+i],id);}
            }else if key>=current{atomicMax(&screen[i],key);}
        }
    }
}
@compute @workgroup_size(64)
fn depth_cs(@builtin(workgroup_id) g:vec3<u32>,@builtin(local_invocation_index) lane:u32){paint(g,lane,false);}
@compute @workgroup_size(64)
fn owner_cs(@builtin(workgroup_id) g:vec3<u32>,@builtin(local_invocation_index) lane:u32){paint(g,lane,true);}
@compute @workgroup_size(8,8)
fn resolve_cs(@builtin(global_invocation_id) id:vec3<u32>){
    if any(id.xy>=dims()){return;}
    let i=index(id.xy);let owner=atomicLoad(&screen[pixels()+i]);
    if owner==0u{return;}
    let slot=owner-1u;let job=jobs[slot/LANES];let tree=trees[job.x];
    let p=points[tree.range.x+job.y+slot%LANES];
    let n=rotate(normal(p),tree.rotation);
    let leaf=(p.w&65536u)!=0u;
    let sun=dot(n,cam.sun_time.xyz);
    let ndl=select(max(sun,0.0),abs(sun)*0.8,leaf);
    let h=point_pos(p).y*tree.origin_scale.w;
    // Cheap crown-interior shading; terrain already has its own canopy shadow field.
    let visibility=mix(0.48,1.0,smoothstep(0.1,0.9,h/max(bitcast<f32>(tree.range.w),0.1)));
    let color=select(unpack4x8unorm(p.z).rgb*(0.12+0.32*ndl*visibility),vec3<f32>(1.0,0.02,0.8),cam.params.z!=0u);
    atomicStore(&screen[2u*pixels()+i],pack4x8unorm(vec4<f32>(color,1.0)));
}

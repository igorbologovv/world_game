struct Camera {
    origin: vec4<f32>, forward: vec4<f32>, right: vec4<f32>, up: vec4<f32>, screen: vec4<f32>,
    levels: array<vec4<u32>,3>, flags: vec4<u32>, hud: array<vec4<u32>,8>,
    prev_origin: vec4<f32>, prev_forward: vec4<f32>, prev_right: vec4<f32>, prev_up: vec4<f32>, history: vec4<u32>,
}
struct Block { batch: u32, distance: u32 }
struct Batch { lo: vec3<f32>, offset: u32, hi: vec3<f32>, count: u32 }
struct Surfel { xy: u32, z_radius: u32, surface_id: u32 }
struct SurfacePatch {
    plane: vec4<f32>,
    anchor: vec3<f32>, material: u32,
    grad_u: vec3<f32>, normal0: u32,
    grad_v: vec3<f32>, normal1: u32,
    uv01: vec4<f32>, uv2_normal_uv0: vec4<f32>, normal_uv12: vec4<f32>,
    normal2: u32, tangent: u32, bitangent: u32, has_tangent: u32,
}
struct Material { factor: vec4<f32>, base: vec4<u32>, normal: vec4<u32>, params: vec4<f32> }
@group(0) @binding(0) var<uniform> cam: Camera;
@group(0) @binding(1) var<storage,read> blocks: array<Block>;
@group(0) @binding(2) var<storage,read> batches: array<Batch>;
@group(0) @binding(3) var<storage,read> points: array<Surfel>;
@group(0) @binding(4) var<storage,read> surfaces: array<SurfacePatch>;
@group(0) @binding(5) var<storage,read> materials: array<Material>;
@group(0) @binding(6) var<storage,read> bin_offsets: array<u32>;
@group(0) @binding(7) var<storage,read> bin_indices: array<u32>;
@group(0) @binding(8) var color_atlas: texture_2d_array<f32>;
@group(0) @binding(9) var normal_atlas: texture_2d_array<f32>;
@group(0) @binding(10) var atlas_sampler: sampler;
@group(0) @binding(11) var<storage,read> acceleration: array<u32>;

const CELL: f32 = 16.0;
const INF: f32 = 1e30;
const BIN_SIDE: i32 = 32;
const BIN_WIDTH: f32 = CELL / f32(BIN_SIDE);
const BINS_PER_TREE: u32 = u32(BIN_SIDE * BIN_SIDE * BIN_SIDE + 1);
const BIN_COUNT: u32 = BINS_PER_TREE - 1u;
const PROXY_SIDE: u32 = 32u;
const ACCEL_STRIDE: u32 = BIN_COUNT + 6u * PROXY_SIDE * PROXY_SIDE * 3u;

@vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32>{
    let x=f32((i<<1u)&2u);
    let y=f32(i&2u);
    return vec4<f32>(x*2.0-1.0,y*2.0-1.0,0.0,1.0);
}
fn slab(o:vec3<f32>,d:vec3<f32>,lo:vec3<f32>,hi:vec3<f32>)->vec2<f32>{
    var a=-INF;var b=INF;
    for(var k=0u;k<3u;k++){
        if abs(d[k])<1e-8 {
            if o[k]<lo[k] || o[k]>=hi[k] {return vec2<f32>(1.0,-1.0);}
        }else{
            let p=(lo[k]-o[k])/d[k];let q=(hi[k]-o[k])/d[k];
            a=max(a,min(p,q));b=min(b,max(p,q));
        }
    }
    return vec2<f32>(a,b);
}
fn block_at(c:vec3<i32>,level:u32,scale:i32)->Block{
    let info=cam.levels[level];
    let g=vec3<u32>(c/scale);
    return blocks[info.x+g.x+info.y*(g.y+info.z*g.z)];
}
fn cell_at(p:vec3<f32>,d:vec3<f32>,dims:vec3<i32>,width:f32)->vec3<i32>{
    let g=p/width;var c=vec3<i32>(floor(g));
    for(var k=0u;k<3u;k++){if d[k]<0.0 && g[k]==floor(g[k]){c[k]-=1;}}
    return clamp(c,vec3<i32>(0),dims-vec3<i32>(1));
}
struct Region {lo:vec3<i32>, hi:vec3<i32>}
fn empty_region(c:vec3<i32>,scale:i32,dist:u32,dims:vec3<i32>)->Region{
    let g=c/scale;let r=i32(max(dist,1u)-1u);
    return Region(max((g-vec3<i32>(r))*scale,vec3<i32>(0)),min((g+vec3<i32>(r+1))*scale,dims));
}
struct Step {t:f32, c:vec3<i32>}
fn advance(o:vec3<f32>,d:vec3<f32>,inv:vec3<f32>,t:f32,region:Region,dims:vec3<i32>,width:f32)->Step{
    var ts=vec3<f32>(INF);
    for(var k=0u;k<3u;k++){
        if abs(d[k])>=1e-8 {
            let edge=select(region.lo[k],region.hi[k],d[k]>0.0);
            ts[k]=(f32(edge)*width-o[k])*inv[k];
        }
    }
    let exit=min(ts.x,min(ts.y,ts.z));
    var next=clamp(cell_at(o+d*exit,d,dims,width),region.lo,region.hi-vec3<i32>(1));
    for(var k=0u;k<3u;k++){if ts[k]==exit{next[k]=select(region.lo[k]-1,region.hi[k],d[k]>0.0);}}
    return Step(max(t,exit),next);
}
fn lit(c:vec3<f32>,n:vec3<f32>,t:f32)->vec3<f32>{
    let sun=normalize(vec3<f32>(-0.5,0.8,-0.4));
    let light=0.36+0.64*max(dot(n,sun),0.0);
    return mix(c*light,vec3<f32>(0.51,0.66,0.78),1.0-exp(-t*0.00045));
}
fn sky(d:vec3<f32>)->vec3<f32>{
    return mix(vec3<f32>(0.65,0.76,0.87),vec3<f32>(0.23,0.47,0.75),max(d.y,0.0));
}
fn random(seed:u32)->f32 {
    var x = seed;
    x = (x ^ (x >> 16u)) * 0x7feb352du;
    x = (x ^ (x >> 15u)) * 0x846ca68bu;
    x = x ^ (x >> 16u);
    return f32(x >> 8u) / 16777216.0;
}

fn rotate(p:vec3<f32>, turn:u32)->vec3<f32> {
    switch turn {
        case 1u: { return vec3<f32>(p.z,p.y,-p.x); }
        case 2u: { return vec3<f32>(-p.x,p.y,-p.z); }
        case 3u: { return vec3<f32>(-p.z,p.y,p.x); }
        default: { return p; }
    }
}

fn jitter(lo:f32, hi:f32, seed:u32)->f32 {
    let left = min(lo, 2.3);
    let right = min(CELL - hi, 2.3);
    return random(seed) * (left + right) - left;
}

fn understory_axis(seed:u32)->f32 {
    let j = random(seed);
    return select(11.0 + (j - 0.5) * 2.0, 4.0 + j * 2.0, j < 0.5);
}

fn unpack_normal(packed:u32)->vec3<f32> {
    let xy = unpack2x16snorm(packed);
    let z = 1.0 - abs(xy.x) - abs(xy.y);
    let fold = max(-z, 0.0);
    return normalize(vec3<f32>(xy - select(vec2<f32>(fold), vec2<f32>(-fold), xy < vec2<f32>(0.0)), z));
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
    let n = normalize(unpack_normal(surface.normal0) * (1.0 - b.x - b.y)
         + unpack_normal(surface.normal1) * b.x + unpack_normal(surface.normal2) * b.y);
    if mat.normal.x == 0u || surface.has_tangent == 0u { return n; }
    let u = unpack_normal(surface.tangent);
    let t = normalize(u - n * dot(n, u));
    let cross_nt = cross(n, t);
    let bitangent = cross_nt * sign(dot(cross_nt, unpack_normal(surface.bitangent)));
    let uv = surface.uv2_normal_uv0.zw + surface.normal_uv12.xy * b.x + surface.normal_uv12.zw * b.y;
    let mapped = textureSampleLevel(normal_atlas, atlas_sampler, atlas_uv(mat.normal, uv), i32(mat.normal.w & 65535u), 0.0).rgb * 2.0 - 1.0;
    return normalize((t * mapped.x + bitangent * mapped.y) * mat.params.y + n * mapped.z);
}

struct TreeHit { t: f32, color: vec3<f32>, normal: vec3<f32> }

// Select the closest of six orthographic views. A depth history_hit is read
// directly from the template's flat proxy array. Off-axis silhouettes and
// visibility are approximations; full-detail mode bypasses this code.
fn proxy_hit(tree:u32,o:vec3<f32>,d:vec3<f32>,start:f32,end:f32)->TreeHit {
    let miss=TreeHit(INF,vec3<f32>(0.0),vec3<f32>(0.0));
    let ad=abs(d);
    var axis=0u;
    if ad.y>ad.x {axis=1u;}
    if ad.z>ad[axis] {axis=2u;}
    let a=(axis+1u)%3u;
    let b=(axis+2u)%3u;
    let face=axis*2u+select(1u,0u,d[axis]>0.0);
    // Project the ray onto the middle of its clipped segment.
    var pos=o+d*((start+end)*0.5);
    let base=tree*ACCEL_STRIDE+BIN_COUNT+face*PROXY_SIDE*PROXY_SIDE*3u;
    var depth=0.0;
    var rgba=0u;
    var normal=0u;
    // One reprojection refinement reduces parallax error for oblique rays.
    for(var i=0u;i<2u;i++){
        let uv=vec2<f32>(pos[a],pos[b])/CELL;
        if any(uv<vec2<f32>(0.0)) || any(uv>=vec2<f32>(1.0)){return miss;}
        let xy=vec2<u32>(uv*f32(PROXY_SIDE));
        let offset=base+(xy.x+xy.y*PROXY_SIDE)*3u;
        rgba=acceleration[offset+1u];
        if rgba==0u {return miss;}
        depth=(bitcast<f32>(acceleration[offset])-o[axis])/d[axis];
        normal=acceleration[offset+2u];
        if depth<start || depth>=end {return miss;}
        pos=o+d*depth;
    }
    let color=vec3<f32>(f32(rgba&255u),f32((rgba>>8u)&255u),f32((rgba>>16u)&255u))/255.0;
    return TreeHit(depth,color,unpack_normal(normal));
}

struct PreparedTree {
    batch: Batch,
    anchor: vec3<f32>,
    shift: vec3<f32>,
    scale: f32,
    tree: u32,
    turn: u32,
}
fn prepare_tree(instance:u32,world_cell:vec3<i32>,understory:bool)->PreparedTree {
    let tree=instance/4u;
    let turn=instance&3u;
    let batch=batches[tree];
    let key=u32(world_cell.x)+u32(world_cell.z)*65537u;
    let center=vec3<f32>(CELL*0.5,0.0,CELL*0.5);
    var anchor=center;
    var shift=vec3<f32>(0.0);
    var scale=1.0;
    if understory {
        anchor=vec3<f32>(understory_axis(key^0x715ed0b1u),0.0,understory_axis(key^0x48f52175u));
        scale=0.34+0.12*random(key^0x57d05d9bu);
    } else {
        shift=vec3<f32>(jitter(batch.lo.x,batch.hi.x,key^0x57a318d1u),0.0,jitter(batch.lo.z,batch.hi.z,key^0x6a814b21u));
    }
    return PreparedTree(batch,anchor,shift,scale,tree,turn);
}
fn prepared_span(prep:PreparedTree,origin:vec3<f32>,direction:vec3<f32>)->vec2<f32> {
    let center=vec3<f32>(CELL*0.5,0.0,CELL*0.5);
    let o=rotate((origin-prep.anchor)/prep.scale,prep.turn)+center-prep.shift;
    return slab(o,rotate(direction,prep.turn),prep.batch.lo,prep.batch.hi)*prep.scale;
}
fn trace_tree(instance:u32,world_cell:vec3<i32>,origin:vec3<f32>,direction:vec3<f32>,limit:f32,understory:bool,world_t:f32)->TreeHit {
    return trace_prepared(prepare_tree(instance,world_cell,understory),origin,direction,limit,world_t);
}
fn trace_prepared(prep:PreparedTree,origin:vec3<f32>,direction:vec3<f32>,limit:f32,world_t:f32)->TreeHit {
    let tree=prep.tree;
    let turn=prep.turn;
    let batch=prep.batch;
    let scale=prep.scale;
    let center=vec3<f32>(CELL*0.5,0.0,CELL*0.5);
    let o=rotate((origin-prep.anchor)/scale,turn)+center-prep.shift;
    let d=rotate(direction,turn);
    let inv=1.0/d;
    let span = slab(o, d, batch.lo, batch.hi);
    var t = max(span.x, 0.0);
    let end = min(span.y, limit / scale);
    let miss = TreeHit(INF, vec3<f32>(0.0), vec3<f32>(0.0));
    if t >= end { return miss; }
    // Screen-space footprint in template metres. L toggles exact reference.
    if cam.flags.y!=0u && (world_t+t*scale)*cam.screen.z/scale>=0.30 {
        let h=proxy_hit(tree,o,d,t,end);
        if h.t>=INF {return miss;}
        return TreeHit(h.t*scale,h.color,rotate(h.normal,(4u-turn)&3u));
    }

    let dims = vec3<i32>(BIN_SIDE);
    var c = cell_at(o + d * t, d, dims, BIN_WIDTH);
    for (var iter = 0i; iter < BIN_SIDE * 3; iter++) {
        if any(c < vec3<i32>(0)) || any(c >= dims) || t >= end { break; }
        let local_bin=u32(c.x+BIN_SIDE*(c.y+BIN_SIDE*c.z));
        let bin=tree*BINS_PER_TREE+local_bin;
        let first=bin_offsets[bin];
        let finish=bin_offsets[bin+1u];
        if cam.flags.x!=0u && first==finish {
            let r=i32(max(acceleration[tree*ACCEL_STRIDE+local_bin],1u)-1u);
            let region=Region(max(c-vec3<i32>(r),vec3<i32>(0)),min(c+vec3<i32>(r+1),dims));
            let jump=advance(o,d,inv,t,region,dims,BIN_WIDTH);
            c=jump.c;t=jump.t;continue;
        }
        let step=advance(o,d,inv,t,Region(c,c+vec3<i32>(1)),dims,BIN_WIDTH);
        var best = min(step.t, end);
        var best_id = 0xffffffffu;
        var best_b = vec2<f32>(0.0);
        var best_color = vec3<f32>(0.0);
        var backface = false;
        var last_surface = 0xffffffffu;
        for (var i = first; i < finish; i++) {
            let point = points[bin_indices[i]];
            if point.surface_id == last_surface { continue; }
            let p = vec3<f32>(f32(point.xy & 65535u), f32(point.xy >> 16u), f32(point.z_radius & 65535u)) / 4096.0;
            let radius = f32(point.z_radius >> 16u) / 65536.0;
            let delta = p - o;
            let along = dot(delta, d);
            let perp = delta - d * along;
            if along + radius < t || along - radius >= best || dot(perp, perp) > radius * radius { continue; }
            let surface = surfaces[point.surface_id];
            let denom = dot(d, surface.plane.xyz);
            if abs(denom) < 1e-7 { continue; }
            let u = (surface.plane.w - dot(o, surface.plane.xyz)) / denom;
            if u < t || u >= best { continue; }
            let pos = o + d * u;
            let diff = pos - p;
            if dot(diff, diff) > radius * radius { continue; }
            last_surface = point.surface_id;
            let rel = pos - surface.anchor;
            let b = vec2<f32>(dot(rel, surface.grad_u), dot(rel, surface.grad_v));
            if min(b.x, b.y) < -0.00005 || b.x + b.y > 1.00005 { continue; }
            let mat = materials[surface.material];
            let rgba = base_color(mat, surface, b);
            if mat.params.x >= 0.0 && rgba.a < mat.params.x { continue; }
            best = u;
            best_id = point.surface_id;
            best_b = b;
            best_color = rgba.rgb;
            backface = denom > 0.0;
        }
        if best_id != 0xffffffffu {
            let surface = surfaces[best_id];
            let n = surface_normal(materials[surface.material], surface, best_b);
            return TreeHit(best * scale, best_color, rotate(select(n, -n, backface), (4u - turn) & 3u));
        }
        c = step.c;
        t = step.t;
    }
    return miss;
}

struct RayState {
    c:vec3<i32>,
    t:f32,
    end:f32,
    ray_live:bool,
    result:vec4<f32>,
}
struct CellInfo { region:Region, id:u32 }
fn begin_ray(o:vec3<f32>,d:vec3<f32>)->RayState {
    let dims=vec3<i32>(cam.levels[0].yzw);
    let extent=vec3<f32>(dims)*CELL;
    var ground=INF;
    if d.y < -1e-8 {
        let g=-o.y/d.y;let p=o+d*g;
        if g>=0.0 && p.x>=0.0 && p.z>=0.0 && p.x<extent.x && p.z<extent.z {ground=g;}
    }
    var background=vec4<f32>(sky(d),0.0);
    if ground<INF {background=vec4<f32>(lit(vec3<f32>(0.16,0.24,0.09),vec3<f32>(0,1,0),ground),ground);}
    let span=slab(o,d,vec3<f32>(0.0),extent);
    let t=max(span.x,0.0);
    let end=min(span.y,ground);
    var c=vec3<i32>(0);
    if t<end {c=cell_at(o+d*t,d,dims,CELL);}
    return RayState(c,t,end,t<end,background);
}
fn get_cell(c:vec3<i32>,dims:vec3<i32>)->CellInfo {
    var region=Region(c,c+vec3<i32>(1));
    let fine=block_at(c,0u,1);
    if cam.flags.x!=0u && fine.batch==0u {
        let b2=block_at(c,2u,16);
        if b2.batch==0u {return CellInfo(empty_region(c,16,b2.distance,dims),0u);}
        let b1=block_at(c,1u,4);
        if b1.batch==0u {return CellInfo(empty_region(c,4,b1.distance,dims),0u);}
        region=empty_region(c,1,fine.distance,dims);
    }
    return CellInfo(region,fine.batch);
}
fn finish_step(state:RayState,next:Step,dims:vec3<i32>)->RayState {
    var s=state;
    s.c=next.c;s.t=next.t;
    s.ray_live=all(s.c>=vec3<i32>(0))&&all(s.c<dims)&&s.t<s.end;
    return s;
}
fn accept_hit(state:RayState,hit:TreeHit)->RayState {
    var s=state;
    let depth=s.t+hit.t;
    s.result=vec4<f32>(lit(hit.color,hit.normal,depth),depth);
    s.ray_live=false;
    return s;
}
fn scalar_continue(initial:RayState,o:vec3<f32>,d:vec3<f32>,inv:vec3<f32>)->vec4<f32>{
    var s=initial;
    let dims=vec3<i32>(cam.levels[0].yzw);
    for(var iter=0i;iter<dims.x+dims.y+dims.z+8;iter++){
        if !s.ray_live {break;}
        let info=get_cell(s.c,dims);
        let next=advance(o,d,inv,s.t,info.region,dims,CELL);
        if info.id!=0u {
            let local=(o-vec3<f32>(s.c)*CELL)+d*s.t;
            let limit=min(next.t,s.end)-s.t;
            var hit=trace_tree((info.id&65535u)-1u,s.c,local,d,limit,false,s.t);
            let under=trace_tree((info.id>>16u)-1u,s.c,local,d,min(limit,hit.t),true,s.t);
            if under.t<hit.t {hit=under;}
            if hit.t<INF {s=accept_hit(s,hit);break;}
        }
        s=finish_step(s,next,dims);
    }
    return s.result;
}
fn trace(o:vec3<f32>,d:vec3<f32>)->vec3<f32>{
    return scalar_continue(begin_ray(o,d),o,d,1.0/d).rgb;
}
@fragment fn fs(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32>{
    // Two lines of 4x6 bitmap glyphs. The panel stays in framebuffer space.
    let hud_scale = select(1, 2, cam.screen.x >= 186.0 && cam.screen.y >= 52.0);
    let hud_pixel = vec2<i32>(p.xy) / hud_scale;
    if hud_pixel.x >= 5 && hud_pixel.y >= 5 && hud_pixel.x < 93 && hud_pixel.y < 26 {
        let q = hud_pixel - vec2<i32>(9, 8);
        if q.x >= 0 && q.y >= 0 {
            let digit = q.x / 5;
            let line = q.y / 8;
            let gx = q.x % 5;
            let gy = q.y % 8;
            if digit < 16 && line < 2 && gx < 4 && gy < 6 {
                let i = u32(line * 16 + digit);
                let glyph = cam.hud[i / 4u][i % 4u];
                let bit = u32(23 - gy * 4 - gx);
                if ((glyph >> bit) & 1u) != 0u {
                    return vec4<f32>(0.88, 0.94, 0.90, 1.0);
                }
            }
        }
        return vec4<f32>(0.014, 0.022, 0.019, 1.0);
    }
    let x=(p.x-cam.screen.x*0.5)*cam.screen.z;
    let y=(cam.screen.y*0.5-p.y)*cam.screen.z;
    let d=normalize(cam.forward.xyz+cam.right.xyz*x+cam.up.xyz*y);
    return vec4<f32>(trace(cam.origin.xyz,d),1.0);
}

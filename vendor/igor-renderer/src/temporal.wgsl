struct Camera {
    origin:vec4<f32>, forward:vec4<f32>, right:vec4<f32>, up:vec4<f32>,
    screen:vec4<f32>, extent:vec4<f32>, hud:array<vec4<u32>,8>, params:vec4<u32>,
}
struct HistoryCamera{
    origin:vec4<f32>, forward:vec4<f32>, right:vec4<f32>, up:vec4<f32>,
    screen:vec4<f32>, valid:vec4<u32>,
}
@group(0) @binding(0) var<uniform> cam:Camera;
@group(0) @binding(1) var<uniform> old:HistoryCamera;
@group(0) @binding(2) var current:texture_2d<f32>;
@group(0) @binding(3) var history:texture_2d<f32>;
@group(0) @binding(4) var output_history:texture_storage_2d<rgba16float,write>;
@group(0) @binding(5) var<storage,read_write> depths:array<atomic<u32>>;
@compute @workgroup_size(8,8)
fn temporal_cs(@builtin(global_invocation_id) id:vec3<u32>){
    let size=vec2<u32>(cam.screen.xy);
    if any(id.xy>=size){return;}
    let p=vec2<i32>(id.xy);
    let z=bitcast<f32>(atomicLoad(&depths[id.x+id.y*size.x]));
    let color=textureLoad(current,p,0).rgb;
    var result=color;
    if old.valid.x!=0u{
        let q=(vec2<f32>(id.xy)+0.5-cam.screen.xy*0.5-vec2<f32>(cam.right.w,cam.up.w))*cam.screen.z;
        let dir=cam.forward.xyz+cam.right.xyz*q.x-cam.up.xyz*q.y;
        var rel=dir;
        if z>0.0{rel=cam.origin.xyz+dir*z-old.origin.xyz;}
        let prev_z=dot(rel,old.forward.xyz);
        if prev_z>0.05{
            let uv=old.screen.xy*0.5+vec2<f32>(old.right.w,old.up.w)+vec2<f32>(dot(rel,old.right.xyz),-dot(rel,old.up.xyz))/(prev_z*old.screen.z);
            let ij=vec2<i32>(floor(uv));
            if all(ij>=vec2<i32>(0))&&all(ij<vec2<i32>(size)){
                let h=textureLoad(history,ij,0);
                // Depth stored as log2(1+z) in half float for large worlds.
                let hz=exp2(h.a)-1.0;
                let compatible=select(h.a==0.0,h.a>0.0&&abs(hz-prev_z)<=max(0.25,prev_z*0.025),z>0.0);
                if compatible{
                    var lo=color;var hi=color;
                    for(var y=-1;y<=1;y++){
                        for(var x=-1;x<=1;x++){
                            let c=textureLoad(current,clamp(p+vec2<i32>(x,y),vec2<i32>(0),vec2<i32>(size)-1),0).rgb;
                            lo=min(lo,c);hi=max(hi,c);
                        }
                    }
                    // Bound stale history at disocclusions and LOD changes.
                    let hcolor=clamp(h.rgb,lo,hi);
                    let motion=length(uv-(vec2<f32>(id.xy)+0.5));
                    let weight=mix(0.88,0.50,clamp(motion/16.0,0.0,1.0));
                    result=mix(color,hcolor,weight);
                }
            }
        }
    }
    textureStore(output_history,p,vec4<f32>(result,log2(1.0+max(z,0.0))));
}

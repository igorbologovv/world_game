struct Camera {
    origin:vec4<f32>, forward:vec4<f32>, right:vec4<f32>, up:vec4<f32>,
    screen:vec4<f32>, extent:vec4<f32>, hud:array<vec4<u32>,8>, params:vec4<u32>,
}
struct Instance {origin_scale:vec4<f32>,offset:u32,turn:u32,pad:vec2<u32>,bounds:vec4<f32>}
struct Choice {ranges:vec4<u32>,metric:vec4<f32>}
struct Job {instance_id:u32,first:u32,count:u32,padding:u32}
@group(0) @binding(0) var<uniform> cam:Camera;
@group(0) @binding(1) var<storage,read_write> instances:array<Instance>;
@group(0) @binding(2) var<storage,read> choices:array<Choice>;
@group(0) @binding(3) var<storage,read_write> jobs:array<Job>;
@compute @workgroup_size(64)
fn select_cs(@builtin(workgroup_id) g:vec3<u32>,@builtin(local_invocation_index) lane:u32){
    let id=(g.x+g.y*1024u)*64u+lane;
    if id>=cam.params.z{return;}
    var inst=instances[id];
    let choice=choices[id];
    let reserved=inst.pad.y;
    // CPU has allocated an upper bound; GPU may only reduce it.
    // Use representative depth with a margin instead of the extreme
    // near edge of a conservative enclosing sphere.
    let z=dot(inst.bounds.xyz-cam.origin.xyz,cam.forward.xyz);
    let ppm=abs(inst.origin_scale.w)/(max(z-inst.bounds.w*0.25,0.05)*cam.screen.z);
    for(var k=0u;k<2u;k++){
        let n=choice.ranges[k*2u+1u];
        if n>0u&&n<inst.pad.y&&choice.metric[k]*ppm<=choice.metric.z*0.85{
            inst.offset=choice.ranges[k*2u];inst.pad.y=n;
        }
    }
    instances[id]=inst;
    let first=bitcast<u32>(choice.metric.w);
    let slots=(reserved+63u)/64u;
    for(var k=0u;k<slots;k++){
        let offset=k*64u;
        var count=0u;
        if offset<inst.pad.y{count=min(64u,inst.pad.y-offset);}
        jobs[first+k]=Job(id,offset,count,0u);
    }
}

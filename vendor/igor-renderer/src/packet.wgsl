
// This file is appended to trace.wgsl. 64 rays per workgroup.
// Keep scalar continuation as the reference and divergence fallback.
@group(0) @binding(12) var previous_frame: texture_2d<f32>;
@group(0) @binding(13) var output_frame: texture_storage_2d<rgba16float,write>;

var<workgroup> selected_lane:atomic<u32>;
var<workgroup> members:atomic<u32>;
var<workgroup> canopy_lo:atomic<u32>;
var<workgroup> canopy_hi:atomic<u32>;
var<workgroup> under_lo:atomic<u32>;
var<workgroup> under_hi:atomic<u32>;
var<workgroup> chosen_cell:vec3<i32>;
var<workgroup> shared_info:CellInfo;
var<workgroup> shared_canopy:PreparedTree;
var<workgroup> shared_under:PreparedTree;
var<workgroup> command:u32;
var<workgroup> all_done:u32;

fn direction_at(p:vec2<f32>,f:vec3<f32>,r:vec3<f32>,u:vec3<f32>)->vec3<f32>{
    let q=(p-cam.screen.xy*0.5)*cam.screen.z;
    return normalize(f+r*q.x-u*q.y);
}
fn project(v:vec3<f32>,f:vec3<f32>,r:vec3<f32>,u:vec3<f32>)->vec2<f32>{
    let z=dot(v,f);
    if z<=0.00001 {return vec2<f32>(-1e6);}
    return cam.screen.xy*0.5+vec2<f32>(dot(v,r),-dot(v,u))/(z*cam.screen.z);
}
fn inside(p:vec2<i32>)->bool {
    return all(p>=vec2<i32>(0))&&all(p<vec2<i32>(cam.screen.xy));
}
// Approximate temporal reuse on alternate pixels. Only independently traced
// previous hits are eligible (positive depth). Negative depths tag reused
// samples, so there is never an unbounded chain of reprojected samples.
// Reject large motions on CPU, off-screen projections and poor pixel matches.
fn temporal_sample(p:vec2<f32>,d:vec3<f32>)->vec4<f32>{
    let miss=vec4<f32>(0.0);
    let prev_f=cam.prev_forward.xyz;
    let prev_r=cam.prev_right.xyz;
    let prev_u=cam.prev_up.xyz;
    var center=vec2<i32>(floor(project(d,prev_f,prev_r,prev_u)));
    if !inside(center){return miss;}
    let rough=textureLoad(previous_frame,center,0);
    // Refine the backward projection for translation using the old depth.
    if rough.a>0.0 {
        let v=cam.origin.xyz+d*rough.a-cam.prev_origin.xyz;
        center=vec2<i32>(floor(project(v,prev_f,prev_r,prev_u)));
    }
    var best=0.16; // squared pixel reprojection error, <=0.4 pixel
    var result=miss;
    for(var y=-1i;y<=1i;y++){
        for(var x=-1i;x<=1i;x++){
            let q=center+vec2<i32>(x,y);
            if !inside(q){continue;}
            let old=textureLoad(previous_frame,q,0);
            if old.a<=0.0 {continue;}
            let old_d=direction_at(vec2<f32>(q)+vec2<f32>(0.5),prev_f,prev_r,prev_u);
            let world=cam.prev_origin.xyz+old_d*old.a;
            let rel=world-cam.origin.xyz;
            let depth=dot(rel,d);
            if depth<=0.0 {continue;}
            let screen_p=project(rel,cam.forward.xyz,cam.right.xyz,cam.up.xyz);
            let delta=screen_p-p;
            let error=dot(delta,delta);
            if error>=best {continue;}
            best=error;
            result=vec4<f32>(old.rgb,-depth);
        }
    }
    return result;
}

@compute @workgroup_size(8,8,1)
fn packet_cs(@builtin(global_invocation_id) gid:vec3<u32>,@builtin(local_invocation_index) lane:u32){
    let valid=gid.x<u32(cam.screen.x)&&gid.y<u32(cam.screen.y);
    let px=vec2<i32>(gid.xy);
    let pixel=vec2<f32>(gid.xy)+vec2<f32>(0.5);
    // Identical view, static scene, prior frame independently refreshed.
    // Uniform branch: all workgroup lanes return together.
    if cam.history.y==1u {
        if valid {textureStore(output_frame,px,textureLoad(previous_frame,px,0));}
        return;
    }
    let o=cam.origin.xyz;
    let d=direction_at(pixel,cam.forward.xyz,cam.right.xyz,cam.up.xyz);
    let inv=1.0/d;
    let dims=vec3<i32>(cam.levels[0].yzw);
    var s=begin_ray(o,d);
    if !valid {s.ray_live=false;}
    if valid && cam.history.x==1u && ((gid.x+gid.y+cam.history.z)&1u)==0u {
        let history_hit=temporal_sample(pixel,d);
        if history_hit.a<0.0 {s.result=history_hit;s.ray_live=false;}
    }

    if cam.flags.z!=0u {
        // Bounded coherent rounds. When fewer than 4 live lanes share the
        // elected cell, finish all remaining rays independently from their
        // CURRENT state, rather than restarting or serializing 64 ray paths.
        for(var round=0u;round<16u;round++){
            if lane==0u {
                atomicStore(&selected_lane,64u);
                atomicStore(&members,0u);
                atomicStore(&canopy_lo,0u);atomicStore(&canopy_hi,0u);
                atomicStore(&under_lo,0u);atomicStore(&under_hi,0u);
            }
            workgroupBarrier();
            if s.ray_live {atomicMin(&selected_lane,lane);}
            workgroupBarrier();
            let leader=atomicLoad(&selected_lane);
            if lane==0u {all_done=select(0u,1u,leader==64u);}
            if lane==leader {chosen_cell=s.c;}
            if workgroupUniformLoad(&all_done)==1u {break;}
            // uniform load includes synchronization; chosen_cell was written
            // before it. Count the rays sharing the leader's next cell.
            let in_packet=s.ray_live&&all(s.c==chosen_cell);
            if in_packet {atomicAdd(&members,1u);}
            workgroupBarrier();
            if lane==leader {
                command=select(0u,1u,atomicLoad(&members)>=4u);
                if command==1u {
                    shared_info=get_cell(chosen_cell,dims);
                    if shared_info.id!=0u {
                        shared_canopy=prepare_tree((shared_info.id&65535u)-1u,chosen_cell,false);
                        shared_under=prepare_tree((shared_info.id>>16u)-1u,chosen_cell,true);
                    }
                }
            }
            if workgroupUniformLoad(&command)==0u {break;}
            // Metadata and instance transforms are read/prepared once.
            // Per-ray AABB tests form two 64-bit masks; a batch with no
            // possible intersection is not traversed by any lane.
            var next=Step(s.t,s.c);
            var local=vec3<f32>(0.0);
            var limit=0.0;
            var test_canopy=false;
            var test_under=false;
            if in_packet {
                next=advance(o,d,inv,s.t,shared_info.region,dims,CELL);
                if shared_info.id!=0u {
                    local=(o-vec3<f32>(s.c)*CELL)+d*s.t;
                    limit=min(next.t,s.end)-s.t;
                    let a=prepared_span(shared_canopy,local,d);
                    let b=prepared_span(shared_under,local,d);
                    test_canopy=max(a.x,0.0)<min(a.y,limit);
                    test_under=max(b.x,0.0)<min(b.y,limit);
                    let bit=1u<<(lane&31u);
                    if test_canopy {
                        if lane<32u {atomicOr(&canopy_lo,bit);}else{atomicOr(&canopy_hi,bit);}
                    }
                    if test_under {
                        if lane<32u {atomicOr(&under_lo,bit);}else{atomicOr(&under_hi,bit);}
                    }
                }
            }
            workgroupBarrier();
            let canopy_mask=select(atomicLoad(&canopy_lo),atomicLoad(&canopy_hi),lane>=32u);
            let under_mask=select(atomicLoad(&under_lo),atomicLoad(&under_hi),lane>=32u);
            let bit=1u<<(lane&31u);
            if in_packet {
                var hit=TreeHit(INF,vec3<f32>(0.0),vec3<f32>(0.0));
                if (canopy_mask&bit)!=0u {
                    hit=trace_prepared(shared_canopy,local,d,limit,s.t);
                }
                if (under_mask&bit)!=0u {
                    let under=trace_prepared(shared_under,local,d,min(limit,hit.t),s.t);
                    if under.t<hit.t {hit=under;}
                }
                if hit.t<INF {s=accept_hit(s,hit);}else{s=finish_step(s,next,dims);}
            }
            // Nobody may overwrite shared metadata while another lane uses it.
            workgroupBarrier();
        }
    }
    if valid {
        let result=scalar_continue(s,o,d,inv);
        textureStore(output_frame,px,result);
    }
}

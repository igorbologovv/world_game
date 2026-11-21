#import bevy_pbr::forward_io::{Vertex, VertexOutput}
#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_normal_local_to_world}
#import bevy_pbr::mesh_view_bindings::globals
#import bevy_pbr::view_transformations::position_world_to_clip

@vertex
fn vertex(mesh: Vertex) -> VertexOutput {
    var output: VertexOutput;
    let world = get_world_from_local(mesh.instance_index);
    let weight = mesh.uv_b.x;
    let phase = mesh.uv_b.y;
    let time = globals.time;
    var position = mesh.position;
    if weight > 0.0 {
        let gust = 0.64 * sin(time * 1.3 + phase) + 0.36 * sin(time * 2.8 + phase * 1.6);
        let flutter = sin(time * 4.4 + phase * 2.1);
        position.x += weight * gust;
        position.z += weight * (gust * 0.43 + flutter * 0.27);
    }

    output.world_position = world * vec4<f32>(position, 1.0);
    output.position = position_world_to_clip(output.world_position.xyz);
    output.world_normal = mesh_normal_local_to_world(mesh.normal, mesh.instance_index);
    output.uv = mesh.uv;
    output.uv_b = mesh.uv_b;
    output.color = mesh.color;
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    output.instance_index = mesh.instance_index;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    output.visibility_range_dither = bevy_pbr::mesh_functions::get_visibility_range_dither_level(mesh.instance_index, world[3]);
#endif
    return output;
}

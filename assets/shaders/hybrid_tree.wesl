@if(VISIBILITY_RANGE_DITHER)
import bevy_pbr::render::mesh_functions::get_visibility_range_dither_level;
@if(VISIBILITY_RANGE_DITHER)
import bevy_pbr::render::pbr_functions::visibility_range_dither;
import super::hybrid_tree_motion::{hybrid_position, hybrid_phase, hybrid_wind, hybrid_color, hybrid_alpha_discard};
import super::tree_crown_lighting::shade_broadleaf;
import bevy_pbr::render::forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex};
import bevy_pbr::render::mesh_functions::{
    get_world_from_local, get_local_from_world,
    mesh_normal_local_to_world, mesh_tangent_local_to_world
};
import bevy_pbr::render::mesh_view_bindings::{globals, lights};
import bevy_pbr::render::view_transformations::position_world_to_clip;
import bevy_pbr::render::pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT;
import bevy_pbr::render::pbr_fragment::pbr_input_from_standard_material;
import bevy_pbr::render::pbr_functions::{
    alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing
};

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> crown_frame: vec2<f32>;

@vertex
fn vertex(
    input: Vertex,
    @if(HYBRID_MORPH) @location(12) morph: vec4<f32>,
) -> VertexOutput {
    let mesh = decompress_vertex(input, input.instance_index);
    let world = get_world_from_local(mesh.instance_index);
    var position = mesh.position;
    @if(HYBRID_MORPH)
    let phase = hybrid_phase(mesh.instance_index);
    @if(HYBRID_MORPH)
    position = hybrid_position(position, morph.xyz, phase);
    @if(HYBRID_WIND)
    position = hybrid_wind(position, mesh.uv_b, globals.time);

    var out: VertexOutput;
    out.world_position = world * vec4(position, 1.0);
    out.position = position_world_to_clip(out.world_position.xyz);
    out.world_normal = mesh_normal_local_to_world(mesh.normal, mesh.instance_index);
    out.uv = mesh.uv;
    @if(VERTEX_UVS_B)
    out.uv_b = mesh.uv_b;
    @if(VERTEX_COLORS && HYBRID_MORPH)
    out.color = hybrid_color(mesh.color, morph.w, phase);
    @if(VERTEX_COLORS && !HYBRID_MORPH)
    out.color = mesh.color;
    @if(VERTEX_TANGENTS)
    out.world_tangent = mesh_tangent_local_to_world(world, mesh.tangent, mesh.instance_index);
    out.instance_index = mesh.instance_index;
    @if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = get_visibility_range_dither_level(mesh.instance_index, world[3]);
    return out;
}

@fragment
fn fragment(input: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var in = input;
    @if(VERTEX_COLORS)
    in.color.a = select(in.color.a, 1.0, in.color.a < 0.0);
    var pbr = pbr_input_from_standard_material(in, is_front);
    pbr.material.base_color = alpha_discard(pbr.material.flags, pbr.material.alpha_cutoff, pbr.material.base_color);
    @if(VISIBILITY_RANGE_DITHER)
    visibility_range_dither(in.position, in.visibility_range_dither);
    @if(VERTEX_COLORS)
    hybrid_alpha_discard(in.world_position, in.instance_index, -input.color.a);
    var color = pbr.material.base_color;
    if (pbr.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        if crown_frame.y > 0.0 {
            let local_from_world = get_local_from_world(in.instance_index);
            let local = (local_from_world * in.world_position).xyz;
            let point = (local - vec3(0.0, crown_frame.x, 0.0)) / crown_frame.y;
            var sun = vec3(0.0, 1.0, 0.0);
            if lights.n_directional_lights > 0u {
                sun = (local_from_world * vec4(lights.directional_lights[0].direction_to_light, 0.0)).xyz;
            }
            shade_broadleaf(&pbr, point, sun, 1.0);
        }
        color = apply_pbr_lighting(pbr);
    }
    return FragmentOutput(main_pass_post_lighting_processing(pbr, color));
}

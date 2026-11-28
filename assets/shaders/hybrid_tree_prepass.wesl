@if(VISIBILITY_RANGE_DITHER)
import bevy_pbr::render::mesh_functions::get_visibility_range_dither_level;
@if(VISIBILITY_RANGE_DITHER)
import bevy_pbr::render::pbr_functions::visibility_range_dither;
import super::hybrid_tree_motion::{hybrid_position, hybrid_phase, hybrid_wind, hybrid_color, hybrid_alpha_discard};
import bevy_pbr::prepass::io::{Vertex, VertexOutput, decompress_vertex};
@if(PREPASS_FRAGMENT)
import bevy_pbr::prepass::io::FragmentOutput;
import bevy_pbr::render::mesh_functions::{
    get_world_from_local, get_previous_world_from_local,
    mesh_normal_local_to_world, mesh_tangent_local_to_world
};
import bevy_render::globals::Globals;
import bevy_pbr::render::view_transformations::position_world_to_clip;
import bevy_pbr::render::pbr_prepass_functions::{prepass_sample_color_and_alpha_discard};
@if(MOTION_VECTOR_PREPASS)
import bevy_pbr::render::pbr_prepass_functions::calculate_motion_vector;

@group(0) @binding(1) var<uniform> globals: Globals;

@vertex
fn vertex(
    input: Vertex,
    @if(HYBRID_MORPH) @location(12) morph: vec4<f32>,
    @if(HYBRID_WIND) @location(13) wind: vec2<f32>,
) -> VertexOutput {
    let mesh = decompress_vertex(input, input.instance_index);
    let world = get_world_from_local(mesh.instance_index);
    var position = mesh.position;
    @if(HYBRID_MORPH)
    let phase = hybrid_phase(mesh.instance_index);
    @if(HYBRID_MORPH)
    position = hybrid_position(position, morph.xyz, phase);
    @if(HYBRID_WIND)
    position = hybrid_wind(position, wind, globals.time);
    var out: VertexOutput;
    out.world_position = world * vec4(position, 1.0);
    out.position = position_world_to_clip(out.world_position.xyz);
    @if(UNCLIPPED_DEPTH_ORTHO_EMULATION) {
        out.unclipped_depth = out.position.z;
        out.position.z = min(out.position.z, 1.0);
    }
    @if(VERTEX_UVS_A)
    out.uv = mesh.uv;
    @if(VERTEX_UVS_B)
    out.uv_b = mesh.uv_b;
    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS)
    out.world_normal = mesh_normal_local_to_world(mesh.normal, mesh.instance_index);
    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_TANGENTS)
    out.world_tangent = mesh_tangent_local_to_world(world, mesh.tangent, mesh.instance_index);
    @if(VERTEX_COLORS && HYBRID_MORPH)
    out.color = hybrid_color(mesh.color, morph.w, phase);
    @if(VERTEX_COLORS && !HYBRID_MORPH)
    out.color = mesh.color;
    @if(MOTION_VECTOR_PREPASS)
    out.previous_world_position = get_previous_world_from_local(mesh.instance_index) * vec4(position, 1.0);
    out.instance_index = mesh.instance_index;
    @if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = get_visibility_range_dither_level(mesh.instance_index, world[3]);
    return out;
}

fn mask(in: VertexOutput) {
    prepass_sample_color_and_alpha_discard(in);
    @if(VISIBILITY_RANGE_DITHER)
    visibility_range_dither(in.position, in.visibility_range_dither);
    @if(VERTEX_COLORS)
    hybrid_alpha_discard(in.world_position, in.instance_index, -in.color.a);
}

@if(PREPASS_FRAGMENT)
@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    mask(in);
    var out: FragmentOutput;
    @if(UNCLIPPED_DEPTH_ORTHO_EMULATION)
    out.frag_depth = in.unclipped_depth;
    @if(NORMAL_PREPASS)
    out.normal = vec4(in.world_normal * 0.5 + vec3(0.5), 1.0);
    @if(MOTION_VECTOR_PREPASS)
    out.motion_vector = calculate_motion_vector(in.world_position, in.previous_world_position);
    return out;
}

@if(!PREPASS_FRAGMENT)
@fragment
fn fragment(in: VertexOutput) {
    mask(in);
}

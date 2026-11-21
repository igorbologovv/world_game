import bevy_pbr::render::forward_io::{Vertex, decompress_vertex};
import bevy_pbr::render::mesh_functions::get_world_from_local;
import bevy_pbr::render::view_transformations::position_world_to_clip;

struct WaterVertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) planet_position: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) @interpolate(flat) velocity: vec3<f32>,
    @location(4) depth_m: f32,
}

fn water_vertex(vertex_in: Vertex) -> WaterVertex {
    let mesh = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world = get_world_from_local(mesh.instance_index) * vec4<f32>(mesh.position, 1.0);
    var velocity = vec3<f32>(0.0);
    var depth_m = 0.0;
@if(VERTEX_COLORS) velocity = mesh.color.xyz;
    @if(VERTEX_COLORS) depth_m = mesh.color.w;
    return WaterVertex(
        position_world_to_clip(world.xyz), world.xyz, mesh.position, mesh.uv,
        velocity, depth_m,
    );
}

fn wave_slope(position: vec3<f32>, time: f32, direction: vec3<f32>, wavelength: f32, steepness: f32) -> vec3<f32> {
    let phase = (dot(position, direction) - time * sqrt(1.56 * wavelength)) * (6.2831853 / wavelength);
    return direction * (cos(phase) * steepness);
}

fn water_normal(up: vec3<f32>, slope: vec3<f32>) -> vec3<f32> {
    return normalize(up - (slope - up * dot(slope, up)));
}

fn water_light(water_color: vec3<f32>, up: vec3<f32>, normal: vec3<f32>, eye: vec3<f32>, roughness: f32) -> vec3<f32> {
    let sun = vec3<f32>(-0.57735, 0.57735, -0.57735);
    let reflected = reflect(-eye, normal);
    let grazing = 1.0 - saturate(dot(normal, eye));
    let fresnel = 0.0204 + 0.9796 * grazing * grazing * grazing * grazing * grazing;
    let elevation = smoothstep(0.0, 0.85, saturate(dot(reflected, up)));
    let daylight = 0.61 + 0.39 * saturate(0.4 + dot(up, sun));
    let sky = daylight * mix(vec3<f32>(0.30, 0.47, 0.61), vec3<f32>(0.085, 0.22, 0.43), elevation);
    let sunward = saturate(dot(reflected, sun));
    let glint = pow(sunward, mix(420.0, 48.0, roughness)) * smoothstep(0.0, 0.12, dot(up, sun));
    return mix(water_color * daylight, sky, fresnel)
        + vec3<f32>(1.0, 0.82, 0.62) * glint * (0.43 + 0.54 * fresnel);
}

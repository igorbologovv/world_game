#import "shaders/water_common.wgsl"::{WaterVertex, water_vertex, water_light}
#import bevy_pbr::forward_io::Vertex
#import bevy_pbr::mesh_view_bindings::view

@group(2) @binding(0)
var<uniform> river_brightness: f32;

@vertex
fn vertex(mesh: Vertex) -> WaterVertex {
    return water_vertex(mesh);
}

@fragment
fn fragment(mesh: WaterVertex) -> @location(0) vec4<f32> {
    // Match the static woodland pond surface; the same material also shades lakes.
    // The original material has no animated normal or flow map.
    let up = normalize(mesh.planet_position);
    let eye = normalize(view.world_position - mesh.world_position);
    let base_color = vec3<f32>(1.0, 0.937488, 0.892829);
    let surface = water_light(base_color, up, up, eye, 0.5) * river_brightness;
    return vec4<f32>(surface, 0.6);
}

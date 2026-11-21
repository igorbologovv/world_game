#import "shaders/water_common.wgsl"::{WaterVertex, water_vertex, wave_slope, water_normal, water_light}
#import bevy_pbr::forward_io::Vertex
#import bevy_pbr::mesh_view_bindings::{globals, view}

@group(2) @binding(0)
var<uniform> river_brightness: f32;

@vertex
fn vertex(mesh: Vertex) -> WaterVertex {
    return water_vertex(mesh);
}

fn current_slope(position: vec3<f32>) -> vec3<f32> {
    return wave_slope(position, 0.0, vec3<f32>(0.80, 0.0, 0.60), 2.8, 0.13)
        + wave_slope(position, 0.0, vec3<f32>(-0.48, 0.64, 0.60), 0.85, 0.055);
}

@fragment
fn fragment(mesh: WaterVertex) -> @location(0) vec4<f32> {
    let footprint = max(length(dpdx(mesh.planet_position)), length(dpdy(mesh.planet_position)));
    let detail = 1.0 - smoothstep(0.5, 4.0, footprint);
    let up = normalize(mesh.planet_position);
    let eye = normalize(view.world_position - mesh.world_position);
    let bank = smoothstep(0.42, 1.0, abs(mesh.uv.y - 0.5) * 2.0);
    let position = mesh.planet_position;
    let time = globals.time;
    var slope = vec3<f32>(0.0);
    var foam = 0.0;
    if detail > 0.0 {
        // Crossfade two short advection phases so changing flow at a bend
        // cannot shift the ripple pattern farther and farther out of phase.
        let phase = fract(time * 0.125) * 8.0;
        let other = fract(time * 0.125 + 0.5) * 8.0;
        let blend = 1.0 - abs(phase * 0.25 - 1.0);
        let first = position - mesh.velocity * phase;
        let second = position - mesh.velocity * other;
        slope = mix(current_slope(second), current_slope(first), blend) * detail;
        let foam_axis = vec3<f32>(0.78, 0.17, -0.47);
        let broken_water = mix(sin(dot(second, foam_axis)), sin(dot(first, foam_axis)), blend);
        let fast = smoothstep(1.4, 4.5, length(mesh.velocity));
        foam = detail * (0.24 * bank + 0.36 * fast) * smoothstep(0.50, 0.97, broken_water);
    }
    let surface = mix(vec3<f32>(0.020, 0.101, 0.095), vec3<f32>(0.125, 0.25, 0.16), bank);
    let color = water_light(surface, up, water_normal(up, slope), eye, 0.76)
        * river_brightness;
    return vec4<f32>(mix(color, vec3<f32>(0.68, 0.80, 0.72), foam), 1.0);
}

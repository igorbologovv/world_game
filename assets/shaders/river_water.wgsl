#import "shaders/water_common.wgsl"::{WaterVertex, water_vertex, wave_slope, water_normal, water_light}
#import bevy_pbr::forward_io::Vertex
#import bevy_pbr::mesh_view_bindings::{globals, view}

@group(2) @binding(0)
var<uniform> river_brightness: f32;

@vertex
fn vertex(mesh: Vertex) -> WaterVertex {
    return water_vertex(mesh);
}

@fragment
fn fragment(mesh: WaterVertex) -> @location(0) vec4<f32> {
    // The original river and lake vertices now carry surface-minus-terrain
    // elevation in mesh color alpha. This is measured depth, not a fake
    // distance-to-bank proxy.
    let depth = max(mesh.depth_m, 0.0);
    let absorption = 1.0 - exp(-depth * 0.14);
    let alpha = mix(0.32, 0.965, absorption);
    let shallow = vec3<f32>(0.39, 0.52, 0.45);
    let deep = vec3<f32>(0.055, 0.20, 0.19);
    let tint = mix(shallow, deep, 1.0 - exp(-depth * 0.09));

    let position = mesh.planet_position;
    let up = normalize(position);
    let eye = normalize(view.world_position - mesh.world_position);
    let footprint = max(length(dpdx(position)), length(dpdy(position)));
    let detail = 1.0 - smoothstep(0.45, 3.5, footprint);
    let glancing = smoothstep(0.015, 0.20, dot(up, eye));
    // Mild local movement, advected by the existing river velocity.
    // Lake vertices have zero velocity and remain gently rippled.
    let moving = position - mesh.velocity * globals.time * 0.15;
    let slope = detail * glancing * (
        wave_slope(moving, globals.time * 0.20,
                   vec3<f32>(0.80, 0.0, 0.60), 9.0, 0.035)
        + wave_slope(moving, globals.time * 0.15,
                     vec3<f32>(-0.48, 0.64, 0.60), 5.6, 0.018)
    );
    let normal = water_normal(up, slope);
    let color = water_light(tint, up, normal, eye, 0.42) * river_brightness;
    return vec4<f32>(color, alpha);
}

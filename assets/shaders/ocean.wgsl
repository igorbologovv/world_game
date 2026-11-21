#import "shaders/water_common.wgsl"::{WaterVertex, water_vertex, wave_slope, water_normal, water_light}
#import bevy_pbr::forward_io::Vertex
#import bevy_pbr::mesh_view_bindings::{globals, view}

@group(2) @binding(0)
var ocean_mask: texture_2d<f32>;
@group(2) @binding(1)
var ocean_sampler: sampler;

@vertex
fn vertex(mesh: Vertex) -> WaterVertex {
    return water_vertex(mesh);
}

@fragment
fn fragment(mesh: WaterVertex) -> @location(0) vec4<f32> {
    let dx = dpdx(mesh.planet_position);
    let dy = dpdy(mesh.planet_position);
    let footprint = max(length(dx), length(dy));
    let water_texel = textureLoad(ocean_mask, vec2<i32>(round(mesh.uv)), 0).rg;
    if water_texel.x < 0.5371 {
        discard;
    }

    let sea = water_texel.x > 0.9309;
    let up = normalize(mesh.planet_position);
    let eye = normalize(view.world_position - mesh.world_position);
    let time = globals.time;
    let waves = 1.0 - smoothstep(6.0, 34.0, footprint);
    let ripples = 1.0 - smoothstep(0.18, 2.0, footprint);
    let position = mesh.planet_position;
    var slope = vec3<f32>(0.0);
    if waves > 0.0 {
        let warp = 3.8 * sin(dot(position, vec3<f32>(0.062, -0.029, 0.091)) - time * 0.38)
            + 1.7 * sin(dot(position, vec3<f32>(-0.13, 0.091, 0.083)) + time * 0.53);
        let chop = 0.72 + 0.28 * sin(dot(position, vec3<f32>(0.19, 0.16, 0.071)) + time * 0.24);
        let swell_position = position + vec3<f32>(0.83, 0.12, -0.52) * warp;
        slope = waves * chop * (
            wave_slope(swell_position, time, vec3<f32>(0.80, 0.0, 0.60), 18.0, 0.082)
            + wave_slope(swell_position, time, vec3<f32>(-0.48, 0.64, 0.60), 10.4, 0.070)
            + wave_slope(position, time, vec3<f32>(0.60, 0.0, -0.80), 6.7, 0.048)
                * (1.0 - smoothstep(2.8, 11.0, footprint))
        );
        slope *= select(0.38, 1.0, sea);
    }
    if ripples > 0.0 {
        slope += ripples * (
            wave_slope(position, time * 0.75, vec3<f32>(0.36, 0.48, -0.80), 2.5, 0.085)
            + wave_slope(position, time, vec3<f32>(-0.80, 0.60, 0.0), 0.72, 0.051)
        );
    }

    var encoded_depth = water_texel.y;
    if footprint < 120.0 {
        let atlas_uv = (mesh.uv + 0.5) / vec2<f32>(textureDimensions(ocean_mask));
        let filtered = textureSampleLevel(ocean_mask, ocean_sampler, atlas_uv, 0.0).g;
        encoded_depth = mix(filtered, water_texel.y, smoothstep(50.0, 120.0, footprint));
    }
    let depth = max(encoded_depth * 96.0 - 16.0, 0.0);
    let shallow = select(vec3<f32>(0.13, 0.33, 0.24), vec3<f32>(0.14, 0.44, 0.38), sea);
    let deep = select(vec3<f32>(0.018, 0.085, 0.078), vec3<f32>(0.009, 0.056, 0.092), sea);
    let transmission = exp(-depth * select(0.14, 0.090, sea));
    // Wave troughs screen each other at grazing angles. Limit the visible
    // slope there so distant swells make a soft reflection across the horizon.
    let exposed_slope = mix(0.34, 1.0, smoothstep(0.015, 0.24, dot(up, eye)));
    let normal = water_normal(up, slope * exposed_slope);
    var color = water_light(mix(deep, shallow, transmission), up, normal, eye, 1.0 - ripples * 0.65);
    if waves > 0.0 && depth < 8.0 {
        let breakers = 0.5 + 0.5 * sin(dot(position, vec3<f32>(0.27, 0.18, -0.38)) - time * 2.2);
        let lace = 0.5 + 0.5 * sin(dot(position, vec3<f32>(0.86, -0.53, 0.37)) + breakers * 2.7 + time * 0.9);
        let foam = waves * (1.0 - smoothstep(0.45, 8.0, depth))
            * smoothstep(0.61, 0.93, breakers * 0.74 + lace * 0.26)
            * select(0.12, 0.75, sea);
        color = mix(color, vec3<f32>(0.66, 0.81, 0.76), foam);
    }
    return vec4<f32>(color, 1.0);
}

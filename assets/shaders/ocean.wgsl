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
    // Retain the generated ocean footprint and its encoded bathymetry.
    let texel = textureLoad(ocean_mask, vec2<i32>(round(mesh.uv)), 0).rg;
    if texel.x < 0.5371 {
        discard;
    }
    let footprint = max(length(dpdx(mesh.planet_position)),
                        length(dpdy(mesh.planet_position)));
    var encoded_depth = texel.y;
    if footprint < 120.0 {
        let size = vec2<f32>(textureDimensions(ocean_mask));
        let filtered = textureSampleLevel(ocean_mask, ocean_sampler,
                                          (mesh.uv + 0.5) / size, 0.0).g;
        encoded_depth = mix(filtered, texel.y, smoothstep(50.0, 120.0, footprint));
    }
    let depth = max(encoded_depth * 96.0 - 16.0, 0.0);
    let sea = texel.x > 0.9309;
    // Beer-Lambert-style visibility: shore is translucent, deep water
    // approaches opacity. The original scene's fixed 0.6 alpha cannot
    // conceal the bottom of a deep lake.
    let absorption = 1.0 - exp(-depth * 0.14);
    let alpha = mix(0.32, 0.965, absorption);
    let shallow = select(vec3<f32>(0.39, 0.52, 0.45),
                         vec3<f32>(0.35, 0.51, 0.50), sea);
    let deep = select(vec3<f32>(0.055, 0.20, 0.19),
                      vec3<f32>(0.042, 0.17, 0.25), sea);
    let tint = mix(shallow, deep, 1.0 - exp(-depth * 0.09));

    let position = mesh.planet_position;
    let up = normalize(position);
    let eye = normalize(view.world_position - mesh.world_position);
    // Short ripples only at near range; spatial derivatives suppress
    // the screen-space moire bands seen in the previous versions.
    let detail = 1.0 - smoothstep(0.45, 3.5, footprint);
    let glancing = smoothstep(0.015, 0.20, dot(up, eye));
    let slope = detail * glancing * (
        wave_slope(position, globals.time * 0.22,
                   vec3<f32>(0.80, 0.0, 0.60), 12.0, 0.035)
        + wave_slope(position, globals.time * 0.18,
                     vec3<f32>(-0.48, 0.64, 0.60), 7.3, 0.019)
    );
    let normal = water_normal(up, slope);
    let color = water_light(tint, up, normal, eye, 0.42);
    return vec4<f32>(color, alpha);
}

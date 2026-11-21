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
    let position = mesh.planet_position;
    // Calm woodland water: broad, low-amplitude irregular swells are only
    // evaluated near the camera. The old periodic short waves made moire
    // stripes at grazing angles across the lake.
    let detail = 1.0 - smoothstep(1.0, 5.0, footprint);
    let slope = detail * (
        wave_slope(position, globals.time * 0.32, vec3<f32>(0.82, 0.0, 0.57), 17.0, 0.024)
        + wave_slope(position, globals.time * 0.23, vec3<f32>(-0.44, 0.35, 0.83), 11.0, 0.015)
    );
    var encoded_depth = water_texel.y;
    if footprint < 120.0 {
        let atlas_uv = (mesh.uv + 0.5) / vec2<f32>(textureDimensions(ocean_mask));
        let filtered = textureSampleLevel(ocean_mask, ocean_sampler, atlas_uv, 0.0).g;
        encoded_depth = mix(filtered, water_texel.y, smoothstep(50.0, 120.0, footprint));
    }
    let depth = max(encoded_depth * 96.0 - 16.0, 0.0);
    // Woodland source pond reference: glossy, light aqua-green, not the
    // old saturated deep-blue ocean. Water depth still darkens naturally.
    let shallow = select(vec3<f32>(0.14, 0.34, 0.29), vec3<f32>(0.13, 0.31, 0.36), sea);
    let deep = select(vec3<f32>(0.043, 0.14, 0.135), vec3<f32>(0.035, 0.12, 0.19), sea);
    let transmission = exp(-depth * select(0.12, 0.085, sea));
    let normal = water_normal(up, slope);
    // A subtle horizon reflection and dark teal body avoid a flat cyan sheet.
    let color = water_light(mix(deep, shallow, transmission), up, normal, eye, 0.32);
    return vec4<f32>(color, 1.0);
}

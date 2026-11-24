// Shared planetary coordinates, reverse-Z depth, and cloud history uniforms.

struct CloudView {
    planet_from_view: mat4x4<f32>,
    view_from_clip: mat4x4<f32>,
    previous_clip_from_current: mat4x4<f32>,
    eye: vec4<f32>,
    sun: vec4<f32>,
    sunlight: vec4<f32>,
    wind_offset: vec4<f32>,
    // Surface, ocean, and atmosphere radii in meters.
    radii: vec4<f32>,
    // Inner and outer radii of the liquid and ice decks.
    layers: vec4<f32>,
    viewport: vec4<u32>,
    frame: vec4<u32>,
}

@group(0) @binding(0) var<uniform> cloud: CloudView;

@if(CLOUD_MSAA)
@group(0) @binding(1) var scene_depth_msaa: texture_depth_multisampled_2d;
@if(!CLOUD_MSAA)
@group(0) @binding(1) var scene_depth: texture_depth_2d;

const NO_OCCLUDER: f32 = 1e9;

fn view_ray(uv: vec2<f32>) -> vec3<f32> {
    let ndc = uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    return normalize((cloud.view_from_clip * vec4<f32>(ndc, 0.0, 1.0)).xyz);
}

fn scene_view_depth(uv: vec2<f32>) -> f32 {
    let pixel = vec2<i32>(cloud.viewport.xy + min(vec2<u32>(uv * vec2<f32>(cloud.viewport.zw)), cloud.viewport.zw - 1u));
    var depth = 0.0;
    @if(CLOUD_MSAA) {
        for (var i = 0u; i < textureNumSamples(scene_depth_msaa); i += 1u) {
            depth = max(depth, textureLoad(scene_depth_msaa, pixel, i));
        }
    }
    @if(!CLOUD_MSAA) {
        depth = textureLoad(scene_depth, pixel, 0);
    }
    if depth <= 0.0 {
        return NO_OCCLUDER;
    }
    // For the perspective camera, clip z is the near plane and clip w is -view z.
    return min(1.0 / (depth * cloud.view_from_clip[2].w), NO_OCCLUDER);
}

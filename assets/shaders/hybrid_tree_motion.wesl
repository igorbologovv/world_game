// Progressive mesh endpoints and retirement from Igor's scene_hybrid.wgsl.
import bevy_pbr::render::mesh_functions::{get_tag, get_local_from_world};

fn hybrid_position(position: vec3<f32>, next: vec3<f32>, phase: f32) -> vec3<f32> {
    return mix(position, next, smoothstep(0.0, 1.0, phase));
}

fn hybrid_phase(instance: u32) -> f32 {
    return f32(get_tag(instance)) / 65535.0;
}

fn hybrid_wind(position: vec3<f32>, wind: vec2<f32>, time: f32) -> vec3<f32> {
    let gust = 0.64 * sin(time * 1.3 + wind.y) + 0.36 * sin(time * 2.8 + wind.y * 1.6);
    let flutter = sin(time * 4.4 + wind.y * 2.1);
    return position + vec3(wind.x * gust, 0.0, wind.x * (gust * 0.43 + flutter * 0.27));
}

// Negative alpha carries the retirement threshold through Bevy's standard
// vertex output. Retained vertices keep their authored color.
fn hybrid_color(color: vec4<f32>, retire_at: f32, phase: f32) -> vec4<f32> {
    let retirement = smoothstep(retire_at, retire_at + 0.4, phase);
    return vec4(color.rgb, select(color.a, -retirement, retirement > 0.0));
}

fn hybrid_alpha_discard(world: vec4<f32>, instance: u32, retirement: f32) {
    if retirement <= 0.0 {
        return;
    }
    let local = (get_local_from_world(instance) * world).xyz;
    let cell = bitcast<vec3<u32>>(vec3<i32>(floor(local * 23.0)));
    var bits = (cell.x * 0x9e3779b9u) ^ (cell.y * 0x85ebca6bu) ^ (cell.z * 0xc2b2ae35u);
    bits = (bits ^ (bits >> 16u)) * 0x7feb352du;
    bits = (bits ^ (bits >> 15u)) * 0x846ca68bu;
    bits = bits ^ (bits >> 16u);
    if f32(bits >> 8u) * (1.0 / 16777216.0) < retirement {
        discard;
    }
}

#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::globals
#import bevy_pbr::mesh_view_bindings::view

// River UVs are supplied by river_surface.rs:
// x = distance along downstream flow in meters, y = 0..1 across the banks.
@group(2) @binding(0)
var<uniform> river_brightness: f32;

fn hash21(p: vec2<f32>) -> f32 {
    let h = fract(p * vec2<f32>(0.1031, 0.11369));
    let k = dot(h, h.yx + vec2<f32>(33.33));
    return fract((h.x + k) * (h.y + k));
}

fn noise2(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let t = fract(p);
    let w = t * t * (3.0 - 2.0 * t);
    return mix(
        mix(hash21(cell), hash21(cell + vec2<f32>(1.0, 0.0)), w.x),
        mix(hash21(cell + vec2<f32>(0.0, 1.0)), hash21(cell + vec2<f32>(1.0, 1.0)), w.x),
        w.y,
    );
}

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let uv = mesh.uv;
    let time = globals.time;
    let edge = clamp(abs(uv.y - 0.5) * 2.0, 0.0, 1.0);

    // Flow moves downstream (+UV.x). Two different scales avoid uniform stripes.
    let flow = uv.x * 0.058 - time * 0.55;
    let turbulence = noise2(vec2<f32>(flow * 0.37, uv.y * 4.5 + time * 0.11));
    let small_wave = sin(flow * 6.2 + uv.y * 24.0 + 2.5 * turbulence);
    let crossing_wave = sin(flow * 3.8 - uv.y * 31.0 + time * 0.24);
    let ripple = 0.5 + 0.25 * small_wave + 0.25 * crossing_wave;

    // Deeper, darker center; shallow green-blue near the banks.
    let depth_color = vec3<f32>(0.020, 0.20, 0.27);
    let shallow_color = vec3<f32>(0.085, 0.39, 0.37);
    let shallow = smoothstep(0.30, 1.0, edge);
    var color = mix(depth_color, shallow_color, 0.65 * shallow);
    color *= 0.90 + 0.19 * ripple + 0.10 * turbulence;

    // Highlights react to the camera and an approximate sun direction.
    let up = normalize(mesh.world_normal);
    let view_dir = normalize(view.world_position - mesh.world_position.xyz);
    let sun_dir = normalize(vec3<f32>(0.38, 0.82, 0.40));
    let x_axis = normalize(cross(up, select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(up.y) > 0.95)));
    let y_axis = normalize(cross(up, x_axis));
    let wave_normal = normalize(up
        + x_axis * (0.040 * small_wave)
        + y_axis * (0.028 * crossing_wave));
    let reflection = reflect(-sun_dir, wave_normal);
    let highlight = pow(max(dot(reflection, view_dir), 0.0), 52.0);
    let fresnel = pow(1.0 - max(dot(wave_normal, view_dir), 0.0), 4.0);
    color += vec3<f32>(0.54, 0.70, 0.66) * highlight * 0.65;
    color += vec3<f32>(0.07, 0.16, 0.22) * fresnel;

    // Subtle moving foam along both banks. River mesh stays opaque to prevent
    // sorting artifacts and preserve the existing depth behavior.
    let foam_noise = noise2(vec2<f32>(flow * 0.65, uv.y * 15.0 - time * 0.07));
    let bank_foam = smoothstep(0.73, 0.98, edge)
        * smoothstep(0.34, 0.88, foam_noise + 0.12 * ripple);
    let streaks = smoothstep(0.87, 0.99, ripple)
        * (1.0 - smoothstep(0.45, 0.93, edge)) * 0.13;
    let foam = clamp(bank_foam * 0.72 + streaks, 0.0, 0.85);
    color = mix(color, vec3<f32>(0.67, 0.87, 0.85), foam);

    return vec4<f32>(clamp(color * river_brightness, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

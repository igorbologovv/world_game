
struct Camera {
    origin: vec4<f32>,
    forward: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    screen: vec4<f32>,
    extent: vec4<f32>,
    params: vec4<u32>,
    effects: vec4<f32>,
    sun: vec4<f32>,
    sunlight: vec4<f32>,
    depth_coeff: vec4<f32>,
    viewport: vec4<u32>,
}

@group(0) @binding(0) var<uniform> cam: Camera;
@group(0) @binding(2) var<storage, read_write> screen_words: array<u32>;

@compute @workgroup_size(8, 8)
fn seed(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = cam.viewport.zw;
    if any(id.xy >= size) { return; }
    let i = id.x + id.y * size.x;
    let depth = opaque_depth(vec2<i32>(cam.viewport.xy + id.xy));
    var key = 0xffffffffu;
    if depth > 0.0 {
        let z = -(cam.depth_coeff.y - depth * cam.depth_coeff.w)
            / (depth * cam.depth_coeff.z - cam.depth_coeff.x);
        if z >= 0.05 {
            // Bias away from the camera to preserve near ties after roundoff.
            key = bitcast<u32>(z + max(0.002, z * 0.00001));
        }
    }
    screen_words[i] = key;
    screen_words[size.x * size.y + i] = 0xffffffffu;
}

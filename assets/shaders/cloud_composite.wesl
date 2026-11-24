import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import super::cloud_view::scene_view_depth;

@group(1) @binding(0) var cloud_radiance: texture_2d<f32>;
@group(1) @binding(1) var cloud_distances: texture_2d<f32>;

@fragment
fn upscale_clouds(input: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(cloud_radiance));
    let subpixel = input.uv * vec2<f32>(size) - 0.5;
    let corner = vec2<i32>(floor(subpixel));
    let f = fract(subpixel);
    let scene = scene_view_depth(input.uv);
    var color = vec4<f32>(0.0);
    var total = 0.0;
    for (var y = 0; y <= 1; y += 1) {
        for (var x = 0; x <= 1; x += 1) {
            let p = clamp(corner + vec2<i32>(x, y), vec2<i32>(0), size - 1);
            let distances = textureLoad(cloud_distances, p, 0).xy;
            let bilinear = select(1.0 - f.x, f.x, x == 1) * select(1.0 - f.y, f.y, y == 1);

            // Preserve foreground edges using the full-resolution reverse-Z buffer.
            // Prefer taps with a matching scene limit where an object bisects the cloud.
            let separation = abs(scene - distances.y);
            let near_depth = min(scene, distances.y);
            let same_surface = 1.0 - smoothstep(12.0 + near_depth * 0.012, 94.0 + near_depth * 0.06, separation);
            let visible = distances.x <= scene;
            let consistent = same_surface > 0.0 || min(near_depth, distances.x) == distances.x;
            let weight = bilinear * select(0.0, mix(0.30, 1.0, same_surface), visible && consistent);
            color += textureLoad(cloud_radiance, p, 0) * weight;
            total += weight;
        }
    }
    return color / max(total, 1e-4);
}

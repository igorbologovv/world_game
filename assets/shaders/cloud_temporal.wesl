import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import super::cloud_view::{cloud, view_ray};

@group(1) @binding(0) var current_radiance: texture_2d<f32>;
@group(1) @binding(1) var current_distances: texture_2d<f32>;
@group(1) @binding(2) var history_radiance: texture_2d<f32>;
@group(1) @binding(3) var history_distances: texture_2d<f32>;

@fragment
fn resolve_clouds(input: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(input.position.xy);
    let color = textureLoad(current_radiance, pixel, 0);
    let sample = textureLoad(current_distances, pixel, 0).xy;
    if cloud.frame.y == 0u || sample.x <= 0.0 {
        return color;
    }

    let ray = view_ray(input.uv);
    let current_hit = (cloud.planet_from_view * vec4<f32>(ray * (sample.x / -ray.z), 0.0)).xyz;
    let clip = cloud.previous_clip_from_current * vec4<f32>(current_hit, 1.0);
    if clip.w <= 0.0 {
        return color;
    }
    let uv = clip.xy / clip.w * vec2<f32>(0.5, -0.5) + 0.5;
    if any(uv < vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) {
        return color;
    }

    let size = vec2<i32>(textureDimensions(current_radiance));
    let old_pixel = uv * vec2<f32>(size) - 0.5;
    let corner = vec2<i32>(floor(old_pixel));
    let fraction = fract(old_pixel);
    let hit_tolerance = clamp(115.0 + clip.w * 0.0062, 115.0, 9000.0);
    var reprojected = vec4<f32>(0.0);
    var total = 0.0;
    for (var y = 0; y <= 1; y += 1) {
        for (var x = 0; x <= 1; x += 1) {
            let p = clamp(corner + vec2<i32>(x, y), vec2<i32>(0), size - 1);
            let previous = textureLoad(history_distances, p, 0).xy;
            if previous.x <= 0.0 || previous.y + hit_tolerance * 0.2 < clip.w {
                continue;
            }
            let consistency = 1.0 - smoothstep(hit_tolerance * 0.35, hit_tolerance, abs(previous.x - clip.w));
            let bilinear = select(1.0 - fraction.x, fraction.x, x == 1)
                * select(1.0 - fraction.y, fraction.y, y == 1);
            let w = consistency * bilinear;
            reprojected += w * textureLoad(history_radiance, p, 0);
            total += w;
        }
    }
    if total < 0.03 {
        return color;
    }

    // Clamp the history to recent samples to remove bright trails at newly
    // visible silhouettes. Depth gates each historical tap before filtering.
    var low = color;
    var high = color;
    let axes = array<vec2<i32>, 4>(
        vec2<i32>(-1, 0), vec2<i32>(1, 0), vec2<i32>(0, -1), vec2<i32>(0, 1),
    );
    for (var i = 0u; i < 4u; i += 1u) {
        let p = clamp(pixel + axes[i], vec2<i32>(0), size - 1);
        let neighbor = textureLoad(current_radiance, p, 0);
        low = min(low, neighbor);
        high = max(high, neighbor);
    }
    let previous = clamp(reprojected / total, low, high);
    let alpha_change = 1.0 - smoothstep(0.10, 0.48, abs(previous.a - color.a));
    let history_weight = mix(0.79, 0.93, smoothstep(600.0, 16000.0, clip.w));
    return mix(color, previous, history_weight * alpha_change * saturate(total * 1.4));
}

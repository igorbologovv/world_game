#import bevy_terrain::types::{AtlasTile}
#import bevy_terrain::bindings::config
#import bevy_terrain::attachments::{sample_height, sample_normal}
#import bevy_terrain::fragment::{FragmentInput, FragmentOutput, fragment_info, fragment_output, fragment_debug}
#import bevy_terrain::functions::lookup_tile


@group(3) @binding(0)
var gradient: texture_1d<f32>;
@group(3) @binding(1)
var gradient_sampler: sampler;

fn sample_color(tile: AtlasTile) -> vec4<f32> {
    let height = sample_height(tile);

    let ground = textureSampleLevel(
        gradient, gradient_sampler, mix(0.09, 0.82, clamp(height / 2400.0, 0.0, 1.0)), 0.0
    ).rgb;
    let rock = vec3<f32>(0.30, 0.26, 0.22);
    let snow = vec3<f32>(0.84, 0.89, 0.93);
    let highlands = mix(ground, rock, smoothstep(1400.0, 3400.0, height));
    return vec4<f32>(mix(highlands, snow, smoothstep(3600.0, 5700.0, height)), 1.0);
}

fn detail_hash(cell: vec3<f32>) -> f32 {
    var hash = fract(cell * vec3<f32>(0.1031, 0.1067, 0.1137));
    hash += vec3<f32>(dot(hash, hash.yzx + vec3<f32>(19.19)));
    return fract((hash.x + hash.y) * hash.z);
}

fn detail_noise(position: vec3<f32>) -> vec4<f32> {
    let origin = floor(position);
    let t = fract(position);
    let weight = t * t * (vec3<f32>(3.0) - 2.0 * t);
    let derivative = 6.0 * t * (vec3<f32>(1.0) - t);

    let a = detail_hash(origin);
    let b = detail_hash(origin + vec3<f32>(1.0, 0.0, 0.0));
    let c = detail_hash(origin + vec3<f32>(0.0, 1.0, 0.0));
    let d = detail_hash(origin + vec3<f32>(1.0, 1.0, 0.0));
    let e = detail_hash(origin + vec3<f32>(0.0, 0.0, 1.0));
    let f = detail_hash(origin + vec3<f32>(1.0, 0.0, 1.0));
    let g = detail_hash(origin + vec3<f32>(0.0, 1.0, 1.0));
    let h = detail_hash(origin + vec3<f32>(1.0, 1.0, 1.0));

    let lower = mix(mix(a, b, weight.x), mix(c, d, weight.x), weight.y);
    let upper = mix(mix(e, f, weight.x), mix(g, h, weight.x), weight.y);
    let dx = mix(mix(b - a, d - c, weight.y), mix(f - e, h - g, weight.y), weight.z);
    let dy = mix(mix(c - a, d - b, weight.x), mix(g - e, h - f, weight.x), weight.z);
    return vec4<f32>(mix(lower, upper, weight.z), derivative * vec3<f32>(dx, dy, upper - lower));
}

fn detail_weight(distance: f32, pixel_width: f32, size: f32, fade: vec2<f32>) -> f32 {
    let distance_weight = 1.0 - smoothstep(fade.x, fade.y, distance);
    let screen_weight = 1.0 - smoothstep(0.25, 0.65, pixel_width / size);
    return distance_weight * screen_weight;
}

@fragment
fn fragment(input: FragmentInput) -> FragmentOutput {
    var info = fragment_info(input);

    let tile   = lookup_tile(info.coordinate, info.blend, 0u);
    var color  = sample_color(tile);
    var normal = sample_normal(tile, info.world_normal);

    if (info.blend.ratio > 0.0) {
        let tile2 = lookup_tile(info.coordinate, info.blend, 1u);
        color     = mix(color,  sample_color(tile2),                     info.blend.ratio);
        normal    = mix(normal, sample_normal(tile2, info.world_normal), info.blend.ratio);
    }

    let radial = normalize(info.world_normal);
    let meters = config.scale * radial;
    let pixel_width = max(length(dpdx(meters)), length(dpdy(meters)));
    let distance = info.view_distance;
    let ground_weight = detail_weight(distance, pixel_width, 38.0, vec2<f32>(650.0, 2700.0));
    let rock_weight = detail_weight(distance, pixel_width, 4.5, vec2<f32>(130.0, 410.0));
    let grain_weight = detail_weight(distance, pixel_width, 0.55, vec2<f32>(60.0, 145.0));

    if ground_weight > 0.0 {
        let patches = detail_noise(meters / 38.0).x;
        color = vec4<f32>(color.rgb * (1.0 + (patches - 0.5) * 0.48 * ground_weight), color.a);
    }
    if rock_weight > 0.0 {
        let stone = detail_noise(meters / 4.5 + vec3<f32>(27.0));
        let steep = 1.0 - clamp(dot(normalize(normal), radial), 0.0, 1.0);
        let outcrop = smoothstep(0.50, 0.81, stone.x + steep * 0.28);
        let stone_color = color.rgb * vec3<f32>(1.20, 1.12, 1.02);
        color = vec4<f32>(mix(color.rgb, stone_color, outcrop * rock_weight), color.a);
        let tangent_gradient = stone.yzw - radial * dot(stone.yzw, radial);
        normal = normalize(normal - tangent_gradient * (0.20 * rock_weight));
    }
    if grain_weight > 0.0 {
        let grains = detail_noise(meters / 0.55 + vec3<f32>(43.0)).x;
        color = vec4<f32>(color.rgb * (1.0 + (grains - 0.5) * 0.34 * grain_weight), color.a);
    }

    let sun = normalize(vec3<f32>(0.011839, 0.878598, -0.477416));
    let slope_light = max(dot(normalize(normal), sun), 0.0);
    color = vec4<f32>(color.rgb * (0.48 + 0.72 * slope_light), color.a);

    var output: FragmentOutput;
    fragment_output(&info, &output, color, normal);

    fragment_debug(&info, &output, tile, normal);
    return output;
}

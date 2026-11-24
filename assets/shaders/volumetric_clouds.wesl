import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import super::cloud_view::{cloud, view_ray, scene_view_depth};

@group(1) @binding(0) var shape_field: texture_3d<f32>;
@group(1) @binding(1) var shape_sampler: sampler;
@group(1) @binding(2) var weather_field: texture_cube<f32>;
@group(1) @binding(3) var weather_sampler: sampler;
@group(1) @binding(4) var air_field: texture_2d<f32>;
@group(1) @binding(5) var air_sampler: sampler;

const LIQUID_EXTINCTION: f32 = 0.0040;
const ICE_EXTINCTION: f32 = 0.00028;
const WEIGHT_CUTOFF: f32 = 0.008;
const MAX_MARCH_STEPS: u32 = 88u;

struct Ray {
    direction: vec3<f32>,
    footprint: f32,
    phase: f32,
    jitter: f32,
}

struct Integral {
    radiance: vec3<f32>,
    transmission: f32,
    hit: f32,
}

struct Sample {
    extinction: f32,
    height: f32,
}

struct Fragment {
    @location(0) radiance: vec4<f32>,
    @location(1) distances: vec2<f32>,
}

struct SphereLine {
    radius: f32,
    axis: f32,
    impact: f32,
}

fn sphere_line(eye: vec3<f32>, radius: f32, direction: vec3<f32>) -> SphereLine {
    // The cross product avoids subtracting orbital-scale squared lengths.
    return SphereLine(radius, dot(eye, direction), length(cross(eye, direction)));
}

fn sphere_span(line: SphereLine, radius: f32) -> vec2<f32> {
    if line.impact >= radius {
        return vec2<f32>(0.0);
    }
    let root = sqrt((radius - line.impact) * (radius + line.impact));
    let far_root = -line.axis - select(-root, root, line.axis >= 0.0);
    if abs(far_root) < 0.001 {
        return vec2<f32>(0.0);
    }
    // Use the factored product for the other root to retain near-hit precision.
    let near_root = (line.radius - radius) * (line.radius + radius) / far_root;
    return vec2<f32>(min(near_root, far_root), max(near_root, far_root));
}

fn shell_spans(outer: vec2<f32>, inner: vec2<f32>, limit: f32) -> vec4<f32> {
    if outer.y <= 0.0 {
        return vec4<f32>(0.0);
    }
    if inner.y <= 0.0 {
        return vec4<f32>(max(outer.x, 0.0), min(outer.y, limit), 0.0, 0.0);
    }
    return vec4<f32>(max(outer.x, 0.0), min(inner.x, limit), inner.y, min(outer.y, limit));
}

fn wind_position(p: vec3<f32>) -> vec3<f32> {
    return p - cloud.wind_offset.xyz;
}

fn weather(p: vec3<f32>, footprint: f32) -> vec3<f32> {
    let lod = log2(max(1.0, footprint / 17000.0));
    return textureSampleLevel(weather_field, weather_sampler, wind_position(p), lod).rgb;
}

fn skew(p: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        dot(p, vec3<f32>(0.36, 0.48, -0.80)),
        dot(p, vec3<f32>(-0.80, 0.60, 0.0)),
        dot(p, vec3<f32>(0.48, 0.64, 0.60)),
    );
}

fn noise(p: vec3<f32>, scale: f32, footprint: f32) -> vec2<f32> {
    let lod = log2(max(1.0, footprint * 64.0 / scale));
    return textureSampleLevel(shape_field, shape_sampler, p / scale, lod).rg;
}

fn active_weather(w: vec3<f32>, ice: bool) -> bool {
    return select(w.x > 0.19, w.z > 0.26, ice);
}

fn density(p: vec3<f32>, radius: f32, w: vec3<f32>, footprint: f32, ice: bool, details: bool) -> Sample {
    if !active_weather(w, ice) {
        return Sample(0.0, 0.0);
    }
    let q = skew(wind_position(p));
    if ice {
        let h = saturate((radius - cloud.layers.z) / (cloud.layers.w - cloud.layers.z));
        let sheet = smoothstep(0.0, 0.20, h) * (1.0 - smoothstep(0.43, 1.0, h));
        let streak = noise(q * vec3<f32>(0.20, 1.00, 0.59), 83000.0, footprint).y;
        let cover = smoothstep(0.37, 0.83, w.z * 0.72 + streak * 0.34);
        return Sample(ICE_EXTINCTION * sheet * cover, h);
    }

    let development = smoothstep(0.19, 0.79, w.y);
    let depth = mix(1560.0, cloud.layers.y - cloud.layers.x, development);
    let h = (radius - cloud.layers.x) / depth;
    if h <= 0.0 || h >= 1.0 {
        return Sample(0.0, saturate(h));
    }
    let envelope = smoothstep(0.0, 0.14, h) * (1.0 - smoothstep(0.48, 1.0, h));
    let mass = noise(wind_position(p), 89000.0, footprint).x;
    let threshold = mix(0.69, 0.282, smoothstep(0.19, 0.84, w.x)) + 0.13 * h * h;
    // Even maximum billow density cannot fill this sample. Detail only erodes.
    let base_amount = 0.55 * mass - threshold;
    if base_amount < -0.45 {
        return Sample(0.0, h);
    }
    let billow = noise(q + vec3<f32>(8573.0, 325.0, 2981.0), 8600.0, footprint);
    var amount = base_amount + 0.45 * billow.x;
    if details && amount > 0.0 {
        let strength = 1.0 - smoothstep(380.0, 2200.0, footprint);
        if strength > 0.0 {
            let erosion = noise(q + vec3<f32>(532.0, 740.0, 891.0), 2380.0, footprint).y;
            amount -= 0.16 * strength * (1.0 - erosion) * (1.0 - smoothstep(0.06, 0.29, amount));
        }
    }
    return Sample(LIQUID_EXTINCTION * envelope * smoothstep(0.0, 0.17, amount), h);
}

fn air_transmission(radius: f32, zenith: f32) -> vec3<f32> {
    let ground = cloud.radii.x;
    let top = cloud.radii.z;
    let r = clamp(radius, ground, top);
    let h = sqrt((top - ground) * (top + ground));
    let rho = sqrt((r - ground) * (r + ground));
    let axis = r * zenith;
    let discriminant = sqrt(max(axis * axis + (top - r) * (top + r), 0.0));
    let distance = select(discriminant - axis, (top - r) * (top + r) / max(discriminant + axis, 1.0), axis > 0.0);
    let uv = vec2<f32>((distance - (top - r)) / (rho + h - (top - r)), rho / h);
    return textureSampleLevel(air_field, air_sampler, uv, 0.0).rgb;
}

fn hg(cosine: f32, g: f32) -> f32 {
    let denominator = 1.0 + g * g - 2.0 * g * cosine;
    return (1.0 - g * g) / (12.566371 * denominator * sqrt(denominator));
}

fn shadow_span(p: vec3<f32>, range: vec2<f32>, footprint: f32, ice: bool) -> f32 {
    let span = range.y - range.x;
    if span <= 0.0 {
        return 0.0;
    }
    let count = select(3u, 2u, ice || footprint > 1550.0);
    var optical_depth = 0.0;
    var start = range.x;
    for (var i = 1u; i <= count; i += 1u) {
        let fraction = f32(i) / f32(count);
        let finish = range.x + span * fraction * fraction;
        let step_size = finish - start;
        let q = p + cloud.sun.xyz * (start + step_size * 0.5);
        let scale = max(footprint, step_size * 0.55);
        optical_depth += density(q, length(q), weather(q, scale), scale, ice, false).extinction * step_size;
        start = finish;
    }
    return optical_depth;
}

fn scattered_light(p: vec3<f32>, radius: f32, sample: Sample, footprint: f32, phase: f32) -> vec3<f32> {
    let sun = cloud.sun.xyz;
    let zenith = dot(p / radius, sun);
    let ground = cloud.radii.x;
    let horizon = -sqrt(max((radius - ground) * (radius + ground), 0.0)) / radius;
    let daylight = smoothstep(horizon - 0.0047, horizon + 0.0047, zenith);
    let sky = smoothstep(horizon - 0.13, 0.27, zenith);
    let ambient = mix(vec3<f32>(0.0018, 0.003, 0.006), vec3<f32>(0.036, 0.045, 0.059), sky)
        * mix(0.72, 1.35, sample.height);
    let ground_bounce = 0.012 * smoothstep(0.0, 0.55, zenith) * (1.0 - sample.height);
    let sun_rgb = cloud.sunlight.rgb;
    if daylight < 0.001 {
        return sun_rgb * ambient;
    }
    let sun_line = sphere_line(p, radius, sun);
    let liquid = shell_spans(
        sphere_span(sun_line, cloud.layers.y), sphere_span(sun_line, cloud.layers.x), 45000.0,
    );
    let ice = shell_spans(
        sphere_span(sun_line, cloud.layers.w), sphere_span(sun_line, cloud.layers.z), 45000.0,
    );
    let optical = shadow_span(p, liquid.xy, footprint, false)
        + shadow_span(p, ice.xy, footprint, true)
        + shadow_span(p, liquid.zw, footprint, false)
        + shadow_span(p, ice.zw, footprint, true);
    let beam = exp(-optical) * phase;
    // A broad, lower-frequency lobe approximates higher scattering orders.
    let multiple = 0.098 * exp(-0.24 * optical) * (1.0 - exp(-1.4 * optical));
    let direct = sun_rgb * air_transmission(radius, max(zenith, horizon)) * daylight;
    return sun_rgb * (ambient + ground_bounce) + direct * (beam + multiple);
}

fn march(ray: Ray, interval: vec2<f32>, ice: bool, result: ptr<function, Integral>) {
    let span = interval.y - interval.x;
    if span <= 0.5 || (*result).transmission < WEIGHT_CUTOFF {
        return;
    }

    // Keep a minimum number of vertical samples at orbit, while making grazing
    // and distant rays cover more meters per step. The budget also bounds the
    // longest possible near-horizontal path inside a deck.
    let min_step = span / f32(MAX_MARCH_STEPS);
    let max_step = max(span / select(10.0, 5.0, ice), min_step);
    var start = interval.x;
    for (var i = 0u; i < MAX_MARCH_STEPS; i += 1u) {
        let footprint = start * ray.footprint;
        let spacing = max(select(100.0, 395.0, ice) + start * 0.012, footprint * 0.45);
        let step_size = min(clamp(spacing, min_step, max_step), interval.y - start);
        let t = start + step_size * ray.jitter;
        let p = cloud.eye.xyz + t * ray.direction;
        let sample_width = max(footprint, step_size * 0.53);
        let w = weather(p, sample_width);
        if !active_weather(w, ice) {
            // In clear weather, probe ahead before stepping across two more
            // cells. Near a weather boundary use the normal step size.
            let skip = min(step_size * 3.0, interval.y - start);
            let end = cloud.eye.xyz + (start + skip) * ray.direction;
            let middle = cloud.eye.xyz + (start + 0.5 * skip) * ray.direction;
            let skip_limit = select(0.15, 0.22, ice);
            let weather_end = weather(end, sample_width);
            let weather_middle = weather(middle, sample_width);
            let end_cover = select(weather_end.x, weather_end.z, ice);
            let middle_cover = select(weather_middle.x, weather_middle.z, ice);
            let start_cover = select(w.x, w.z, ice);
            start += select(step_size, skip, max(start_cover, max(end_cover, middle_cover)) < skip_limit);
        } else {
            let radius = length(p);
            let sample = density(p, radius, w, sample_width, ice, true);
            if sample.extinction > 1e-6 {
                let opacity = 1.0 - exp(-sample.extinction * step_size);
                let weight = (*result).transmission * opacity;
                (*result).radiance += weight * scattered_light(p, radius, sample, sample_width, ray.phase);
                (*result).hit += weight * t;
                (*result).transmission *= 1.0 - opacity;
            }
            start += step_size;
        }
        if start >= interval.y - 0.25 || (*result).transmission < WEIGHT_CUTOFF {
            break;
        }
    }
}

fn aerial_perspective(line: SphereLine, ray: vec3<f32>, hit: f32, radiance: vec3<f32>) -> vec3<f32> {
    let p = cloud.eye.xyz + ray * hit;
    let radius = length(p);
    let outer = sphere_span(line, cloud.radii.z);
    let entry = max(outer.x, 0.0);
    let origin = cloud.eye.xyz + ray * entry;
    let origin_radius = max(length(origin), cloud.radii.x);
    let ground = sphere_span(line, cloud.radii.x);
    let backwards = ground.y > 0.0;
    let direction = select(ray, -ray, backwards);
    let a = air_transmission(origin_radius, dot(normalize(origin), direction));
    let b = air_transmission(radius, dot(p / radius, direction));
    let transmission = saturate(select(a / max(b, vec3<f32>(1e-4)), b / max(a, vec3<f32>(1e-4)), backwards));
    let daylight = smoothstep(-0.13, 0.29, dot(p / radius, cloud.sun.xyz));
    let haze = cloud.sunlight.rgb * mix(vec3<f32>(0.005, 0.009, 0.020), vec3<f32>(0.071, 0.093, 0.138), daylight);
    return radiance * transmission + (1.0 - transmission) * haze;
}

@fragment
fn march_clouds(input: FullscreenVertexOutput) -> Fragment {
    let p_view = view_ray(input.uv);
    let direction = normalize((cloud.planet_from_view * vec4<f32>(p_view, 0.0)).xyz);
    let depth = scene_view_depth(input.uv);
    let clear = Fragment(vec4<f32>(0.0), vec2<f32>(0.0, depth));
    let scene_limit = depth / -p_view.z;
    let gap = max(cloud.layers.x - cloud.eye.w, cloud.eye.w - cloud.layers.w);
    if scene_limit < gap {
        return clear;
    }
    let line = sphere_line(cloud.eye.xyz, cloud.eye.w, direction);
    let outer = sphere_span(line, cloud.layers.w);
    if outer.y <= 0.0 || outer.x > scene_limit {
        return clear;
    }
    let ground = sphere_span(line, cloud.radii.y);
    var limit = min(scene_limit, outer.y);
    if ground.x > 0.0 {
        limit = min(limit, ground.x);
    }

    if limit <= gap {
        return clear;
    }
    let ice = shell_spans(outer, sphere_span(line, cloud.layers.z), limit);
    let liquid = shell_spans(
        sphere_span(line, cloud.layers.y),
        sphere_span(line, cloud.layers.x), limit,
    );
    let footprint = 4.0 * max(
        cloud.view_from_clip[0].x / f32(cloud.viewport.z),
        cloud.view_from_clip[1].y / f32(cloud.viewport.w),
    ) * -p_view.z;
    let cosine = dot(direction, cloud.sun.xyz);
    let phase = 0.77 * hg(cosine, 0.66) + 0.16 * hg(cosine, -0.27) + 0.07 * hg(cosine, 0.0);
    let noise = fract(52.9829189 * fract(dot(floor(input.position.xy), vec2<f32>(0.06711056, 0.00583715))));
    let jitter = 0.12 + 0.76 * fract(noise + f32(cloud.frame.x % 64u) * 0.61803399);
    let ray = Ray(direction, footprint, phase, jitter);
    var result = Integral(vec3<f32>(0.0), 1.0, 0.0);
    march(ray, ice.xy, true, &result);
    march(ray, liquid.xy, false, &result);
    march(ray, liquid.zw, false, &result);
    march(ray, ice.zw, true, &result);
    let alpha = 1.0 - result.transmission;
    if alpha < 0.001 {
        return clear;
    }
    let hit = result.hit / alpha;
    let radiance = aerial_perspective(line, direction, hit, result.radiance / alpha);
    return Fragment(vec4<f32>(radiance * alpha, alpha), vec2<f32>(hit * -p_view.z, depth));
}

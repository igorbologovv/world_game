import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import super::cloud_view::{cloud, view_ray, scene_view_depth};

@group(1) @binding(0) var shape_field: texture_3d<f32>;
@group(1) @binding(1) var shape_sampler: sampler;
@group(1) @binding(2) var weather_field: texture_cube<f32>;
@group(1) @binding(3) var weather_sampler: sampler;
@group(1) @binding(4) var air_field: texture_2d<f32>;
@group(1) @binding(5) var air_sampler: sampler;

const LIQUID_EXTINCTION: f32 = 0.0053;
const ICE_EXTINCTION: f32 = 0.00028;
const WEIGHT_CUTOFF: f32 = 0.008;
const MAX_MARCH_STEPS: u32 = 88u;

struct Ray {
    direction: vec3<f32>,
    footprint: f32,
    phase: vec2<f32>,
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
    mass: f32,
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

fn weather(p: vec3<f32>, footprint: f32) -> vec4<f32> {
    let lod = log2(max(1.0, footprint / 17000.0));
    return textureSampleLevel(weather_field, weather_sampler, wind_position(p), lod);
}

fn skew(p: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        dot(p, vec3<f32>(0.36, 0.48, -0.80)),
        dot(p, vec3<f32>(-0.80, 0.60, 0.0)),
        dot(p, vec3<f32>(0.48, 0.64, 0.60)),
    );
}

fn noise(p: vec3<f32>, scale: f32, footprint: f32) -> vec4<f32> {
    let lod = log2(max(1.0, footprint * 64.0 / scale));
    return textureSampleLevel(shape_field, shape_sampler, p / scale, lod);
}

fn active_weather(w: vec4<f32>, ice: bool) -> bool {
    return select(w.x > 0.23, w.z > 0.26, ice);
}

fn liquid_profile(w: vec4<f32>) -> vec2<f32> {
    let convection = smoothstep(0.31, 0.65, w.a + 0.23 * (w.y - 0.5));
    let development = smoothstep(0.16, 0.80, w.y);
    let depth = mix(1170.0 + 710.0 * development, 1850.0 + 1350.0 * development, convection);
    return vec2<f32>(convection, depth);
}

fn density(
    p: vec3<f32>, radius: f32, w: vec4<f32>, profile: vec2<f32>, footprint: f32,
    ice: bool, details: bool, local_mass: f32,
) -> Sample {
    if !active_weather(w, ice) {
        return Sample(0.0, 0.0, -1.0);
    }
    let advected = wind_position(p);
    if ice {
        let height = max(radius - cloud.layers.z, 0.0);
        let h = height / (cloud.layers.w - cloud.layers.z);
        let q = skew(advected) + height * vec3<f32>(1.8, -0.35, 0.6);
        // Thin crystal sheets stretch in the direction of the wind. Shear
        // above the base bends the wisps into trailing filaments.
        let streak = noise(q * vec3<f32>(0.14, 1.0, 0.53), 46000.0, footprint);
        let sheet = smoothstep(0.0, 0.14, h) * (1.0 - smoothstep(0.22 + 0.25 * streak.r, 1.0, h));
        let cover = smoothstep(0.39, 0.83, 0.66 * w.z + 0.46 * streak.g - 0.10 * streak.b);
        return Sample(ICE_EXTINCTION * sheet * cover, h, -1.0);
    }

    // Lift selects between shallow stratocumulus sheets and vertically
    // developed cumulus. Both retain the same spherical condensation base.
    let convection = profile.x;
    let elevation = radius - cloud.layers.x;
    let h = elevation / profile.y;
    if h <= 0.0 || h >= 1.0 {
        return Sample(0.0, saturate(h), -1.0);
    }
    var mass = local_mass;
    if mass < 0.0 {
        mass = noise(advected, 89000.0, footprint).r;
    }
    let threshold = mix(0.69, 0.278, smoothstep(0.19, 0.84, w.x));
    let crown = mix(0.085, 0.16, convection) * h * h;
    let body = 0.55 * mass - threshold - crown;
    // Even full billow density cannot reach the lowest visible edge here.
    if body < -0.52 {
        return Sample(0.0, h, mass);
    }
    // The height profile flattens shallow sheets. A radial offset changes
    // the billow cross-section as the ray climbs through either formation.
    let stretch = mix(0.46, -0.23, convection) * elevation;
    let q = skew(advected - (p / radius) * stretch);
    let billow = noise(q + vec3<f32>(8573.0, 325.0, 2981.0), 8600.0, footprint);
    let cells = (billow.g - 0.48) * mix(0.025, 0.12, convection);
    let pocket = 0.032 * (1.0 - smoothstep(0.27, 0.59, billow.a));
    var body_shape = body + 0.45 * billow.r + cells - pocket;
    var entrainment = billow.a;
    if details && body_shape > -0.026 && footprint < 1690.0 {
        let fine_weight = 1.0 - smoothstep(210.0, 1690.0, footprint);
        let eddy = vec3<f32>(billow.g - 0.5, billow.a - 0.5, billow.b - 0.5) * 740.0;
        let detail = noise(q + vec3<f32>(532.0, 740.0, 891.0) + eddy, 1650.0, footprint);
        let edge = 1.0 - smoothstep(0.035, 0.23, body_shape);
        body_shape -= (0.018 + 0.148 * detail.b) * fine_weight * edge;
        entrainment += (detail.a - 0.48) * (0.72 * fine_weight);
    }

    let condensation = smoothstep(0.004, 0.118, body_shape);
    let interior = mix(0.10, 1.0, smoothstep(0.32, 0.68, entrainment));
    let base = smoothstep(0.0, mix(0.16, 0.22, convection), h);
    let shoulder = mix(0.43, 0.45 + 0.27 * billow.g, convection);
    let top = 1.0 - smoothstep(shoulder, 1.0, h);
    return Sample(LIQUID_EXTINCTION * base * top * condensation * interior, h, mass);
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

fn shadow_span(
    p: vec3<f32>, range: vec2<f32>, footprint: f32, local_weather: vec4<f32>,
    local_profile: vec2<f32>, local_mass: f32, ice: bool,
) -> f32 {
    let span = range.y - range.x;
    if span <= 0.0 {
        return 0.0;
    }
    let count = select(select(3u, 2u, footprint > 450.0), 1u, ice);
    var optical_depth = 0.0;
    var start = range.x;
    for (var i = 1u; i <= count; i += 1u) {
        let fraction = f32(i) / f32(count);
        let finish = range.x + span * fraction * fraction;
        let step_size = finish - start;
        let distance = start + step_size * 0.5;
        let q = p + cloud.sun.xyz * distance;
        let width = max(footprint, step_size * 0.55);
        // The weather texels span kilometers. Nearby light taps can reuse
        // the view sample's weather while still evaluating 3D density.
        var w = local_weather;
        var mass = local_mass;
        var profile = local_profile;
        if distance > 2200.0 {
            w = weather(q, width);
            profile = liquid_profile(w);
            mass = -1.0;
        }
        optical_depth += density(q, length(q), w, profile, width, ice, false, mass).extinction * step_size;
        start = finish;
    }
    return optical_depth;
}

fn scattered_light(
    p: vec3<f32>, radius: f32, sample: Sample, footprint: f32, w: vec4<f32>, profile: vec2<f32>,
    phase: vec2<f32>,
) -> vec3<f32> {
    let sun = cloud.sun.xyz;
    let zenith = dot(p / radius, sun);
    let ground = cloud.radii.x;
    let horizon = -sqrt(max((radius - ground) * (radius + ground), 0.0)) / radius;
    let daylight = smoothstep(horizon - 0.0047, horizon + 0.0047, zenith);
    let sky = smoothstep(horizon - 0.13, 0.27, zenith);
    let ambient = mix(vec3<f32>(0.0018, 0.003, 0.006), vec3<f32>(0.034, 0.041, 0.051), sky)
        * mix(0.74, 1.39, sample.height);
    let ground_bounce = 0.011 * smoothstep(0.0, 0.55, zenith) * (1.0 - sample.height);
    let sun_rgb = cloud.sunlight.rgb;
    if daylight < 0.001 {
        return sun_rgb * ambient;
    }

    let sun_line = sphere_line(p, radius, sun);
    let local_top = cloud.layers.x + profile.y + 180.0;
    let liquid_top = select(cloud.layers.y, min(cloud.layers.y, local_top), zenith > 0.60);
    let liquid_exit = sphere_span(sun_line, liquid_top);
    let ice_exit = sphere_span(sun_line, cloud.layers.w);
    let ice_base = sphere_span(sun_line, cloud.layers.z);
    // In daylight the sun ray climbs monotonically through both decks.
    // Near the terminator it can enter the same deck on both sides of the tangent.
    var liquid = vec4<f32>(0.0, min(liquid_exit.y, 45000.0), 0.0, 0.0);
    var ice = vec4<f32>(max(ice_base.y, 0.0), min(ice_exit.y, 45000.0), 0.0, 0.0);
    if zenith < 0.0 {
        liquid = shell_spans(liquid_exit, sphere_span(sun_line, cloud.layers.x), 45000.0);
        ice = shell_spans(ice_exit, ice_base, 45000.0);
    }
    var optical = shadow_span(p, liquid.xy, footprint, w, profile, sample.mass, false)
        + shadow_span(p, ice.xy, footprint, w, profile, -1.0, true);
    if zenith < 0.0 {
        optical += shadow_span(p, liquid.zw, footprint, w, profile, sample.mass, false)
            + shadow_span(p, ice.zw, footprint, w, profile, -1.0, true);
    }
    let direct = sun_rgb * air_transmission(radius, max(zenith, horizon)) * daylight;
    let beam = exp(-optical);
    // The narrow single-scattering lobe is visible at backlit edges.
    // Broader, less attenuated orders carry light into the interior.
    let diffuse = exp(-0.30 * optical);
    let multiple = (0.113 + phase.y) * diffuse * (1.0 - beam);
    let sky_fill = ambient * (0.64 + 0.36 * diffuse);
    return sun_rgb * (sky_fill + ground_bounce) + direct * (beam * phase.x + multiple);
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
    var light_t = -1e9;
    var light_extinction = 0.0;
    var lighting = vec3<f32>(0.0);
    for (var i = 0u; i < MAX_MARCH_STEPS; i += 1u) {
        let footprint = start * ray.footprint;
        let liquid_spacing = 88.0 + 34.0 * smoothstep(850.0, 2800.0, start);
        let spacing = max(select(liquid_spacing, 420.0, ice) + start * 0.013, footprint * 0.46);
        let step_size = min(clamp(spacing, min_step, max_step), interval.y - start);
        let t = start + step_size * ray.jitter;
        let p = cloud.eye.xyz + t * ray.direction;
        let sample_width = max(footprint, step_size * 0.53);
        let w = weather(p, sample_width);
        if !active_weather(w, ice) {
            // In clear weather, probe ahead before stepping across two more
            // cells. Near a weather boundary use the normal step size.
            let skip = min(step_size * 3.0, interval.y - start);
            let skip_limit = select(0.18, 0.22, ice);
            let start_cover = select(w.x, w.z, ice);
            var clear = false;
            if start_cover < skip_limit {
                let end = weather(cloud.eye.xyz + (start + skip) * ray.direction, sample_width);
                clear = select(end.x, end.z, ice) < skip_limit;
                if clear && skip > 2500.0 {
                    let middle = weather(cloud.eye.xyz + (start + 0.5 * skip) * ray.direction, sample_width);
                    clear = select(middle.x, middle.z, ice) < skip_limit;
                }
            }
            start += select(step_size, skip, clear);
        } else {
            let radius = length(p);
            let profile = liquid_profile(w);
            let above_deck = radius - (cloud.layers.x + profile.y);
            let cap_skip = min(min(step_size * 2.5, 1600.0), interval.y - start);
            if !ice && cap_skip > step_size && above_deck > cap_skip + 200.0 {
                // Leave a margin for changes in the weather-driven cloud top.
                start += cap_skip;
                continue;
            }
            let sample = density(p, radius, w, profile, sample_width, ice, true, -1.0);
            if sample.extinction > 1e-6 {
                let opacity = 1.0 - exp(-sample.extinction * step_size);
                let weight = (*result).transmission * opacity;
                // Lighting varies more slowly inside a billow than its density.
                // Reuse it across adjacent steps and refresh it at density breaks.
                let extent = min(step_size * 2.1, 1100.0);
                let change = abs(light_extinction - sample.extinction);
                if t - light_t > extent || change > 0.00029 + 0.47 * sample.extinction {
                    lighting = scattered_light(p, radius, sample, sample_width, w, profile, ray.phase);
                    light_t = t;
                    light_extinction = sample.extinction;
                }
                (*result).radiance += weight * lighting;
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
    let distance = max(hit - entry, 0.0);
    let local_air = exp(-max(origin_radius - cloud.radii.x, 0.0) / 8500.0);
    var transmission = exp(-distance * local_air * vec3<f32>(0.000010, 0.000016, 0.000027));
    if distance > 950.0 {
        // For short paths, local extinction avoids cancellation between
        // nearly equal LUT values on either side of the ground tangent.
        let ground = sphere_span(line, cloud.radii.x);
        let backwards = ground.y > 0.0;
        let direction = select(ray, -ray, backwards);
        let a = air_transmission(origin_radius, dot(normalize(origin), direction));
        let b = air_transmission(radius, dot(p / radius, direction));
        let lut = saturate(select(a / max(b, vec3<f32>(1e-4)), b / max(a, vec3<f32>(1e-4)), backwards));
        transmission = mix(transmission, lut, smoothstep(950.0, 2300.0, distance));
    }
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
    let phase = vec2<f32>(
        0.47 * hg(cosine, 0.72) + 0.29 * hg(cosine, 0.88) + 0.18 * hg(cosine, -0.42) + 0.00478,
        0.34 * hg(cosine, 0.43),
    );
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

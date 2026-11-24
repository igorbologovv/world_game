import bevy_pbr::render::forward_io::{Vertex, decompress_vertex};
import bevy_pbr::render::mesh_functions::get_world_from_local;
import bevy_pbr::render::mesh_view_bindings::{globals, lights, view};
import bevy_pbr::render::view_transformations::{frag_coord_to_ndc, position_world_to_clip};
@if(ATMOSPHERE)
import bevy_pbr::render::mesh_view_bindings::atmosphere;
@if(ATMOSPHERE)
import bevy_pbr::render::pbr_lighting::sample_transmittance_lut;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> radii: vec4<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var billows: texture_3d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var billow_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var weather_map: texture_cube<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var weather_sampler: sampler;

const WEATHER_TEXEL: f32 = 17000.0;
const LOWER_DEPTH: f32 = 3200.0;
const UPPER_BASE: f32 = 4950.0;

struct CloudVertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) planet_center: vec3<f32>,
}

@vertex
fn vertex(input: Vertex) -> CloudVertex {
    let mesh = decompress_vertex(input, input.instance_index);
    let transform = get_world_from_local(mesh.instance_index);
    let world = (transform * vec4<f32>(mesh.position, 1.0)).xyz;
    return CloudVertex(position_world_to_clip(world), transform[3].xyz);
}

// This rotates the small-scale field without introducing seams or latitude singularities.
fn skew(p: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        dot(p, vec3<f32>(0.36, 0.48, -0.80)),
        dot(p, vec3<f32>(-0.80, 0.60, 0.0)),
        dot(p, vec3<f32>(0.48, 0.64, 0.60)),
    );
}

fn billow_sample(p: vec3<f32>, scale: f32, footprint: f32) -> vec2<f32> {
    let mip = log2(max(footprint * 64.0 / scale, 1.0));
    return textureSampleLevel(billows, billow_sampler, p / scale, mip).rg;
}

struct LayerSample {
    density: f32,
    height: f32,
}

// The cloud bases follow an altitude band. The weather map controls whether
// that band contains low sheets or vertically developed cumulus.
fn lower_cloud(
    p: vec3<f32>, elevation: f32, weather: vec2<f32>, footprint: f32, erode_edges: bool,
) -> LayerSample {
    let depth = LOWER_DEPTH * mix(0.52, 1.0, smoothstep(0.20, 0.77, weather.y));
    let h = (elevation - (radii.y - radii.x)) / depth;
    if h <= 0.0 || h >= 1.0 || weather.x < 0.15 {
        return LayerSample(0.0, saturate(h));
    }
    // Two different scales and orientations keep local features from lining
    // up across a weather system. Erosion affects edges more than cores.
    let mass = billow_sample(p, 92000.0, footprint).x;
    let q = skew(p) + vec3<f32>(8573.0, 325.0, 2981.0);
    let detail = billow_sample(q, 8200.0, footprint);
    let threshold = mix(0.71, 0.27, smoothstep(0.20, 0.83, weather.x));
    let detrainment = 0.19 * smoothstep(0.28, 1.0, h);
    var shape = 0.48 * mass + 0.52 * detail.x - threshold - detrainment;
    if erode_edges {
        let close = 1.0 - smoothstep(260.0, 1150.0, footprint);
        var fine = 0.0;
        if close > 0.0 {
            fine = (billow_sample(q + vec3<f32>(532.0, 740.0, 891.0), 2300.0, footprint).y - 0.5)
                * 0.18 * close;
        }
        shape += (detail.y - 0.5) * 0.29 + fine;
    }
    let base = smoothstep(0.0, 0.16, h);
    let top = 1.0 - smoothstep(0.48, 1.0, h);
    return LayerSample(base * top * smoothstep(-0.022, 0.16, shape), h);
}

fn upper_cloud(p: vec3<f32>, elevation: f32, cover: f32, footprint: f32) -> f32 {
    let h = (elevation - UPPER_BASE) / (radii.z - radii.x - UPPER_BASE);
    if h <= 0.0 || h >= 1.0 || cover < 0.23 {
        return 0.0;
    }
    // A weaker ice layer spreads along the prevailing flow.
    let streak = billow_sample(skew(p) * vec3<f32>(0.18, 1.0, 0.62), 118000.0, footprint).y;
    let profile = smoothstep(0.0, 0.24, h) * (1.0 - smoothstep(0.38, 1.0, h));
    return 0.14 * profile * smoothstep(0.43, 0.91, cover * 0.68 + streak * 0.40);
}

// Using the perpendicular distance avoids cancellation at orbital distances.
// The stable product also retains the near intersection at cloud altitude.
fn sphere_span(eye: vec3<f32>, ray: vec3<f32>, radius: f32) -> vec2<f32> {
    let b = dot(eye, ray);
    let perpendicular = eye - b * ray;
    let discriminant = radius * radius - dot(perpendicular, perpendicular);
    if discriminant <= 0.0 {
        return vec2<f32>(0.0);
    }
    let q = -b - select(-1.0, 1.0, b >= 0.0) * sqrt(discriminant);
    let eye_radius = length(eye);
    let other = (eye_radius - radius) * (eye_radius + radius) / q;
    return vec2<f32>(min(q, other), max(q, other));
}

fn cloud_light(
    p: vec3<f32>, drift: vec3<f32>, elevation: f32, weather: vec2<f32>, footprint: f32,
    height: f32, upper: bool, sun: vec3<f32>, sunlight: vec3<f32>, phase: f32,
) -> vec3<f32> {
    let zenith = dot(normalize(p), sun);
    let horizon = -sqrt(max(2.0 * elevation / radii.x, 0.0));
    let daylight = smoothstep(horizon - 0.035, horizon + 0.10, zenith);
    var light_path = 0.0;
    if !upper && daylight > 0.001 {
        let detail_weight = 1.0 - smoothstep(2300.0, 9000.0, footprint);
        // At long range, use the filtered cloud height to approximate the
        // light path. Local cloud samples resolve directional billow shadows.
        light_path = 3.6 * (1.0 - height) * smoothstep(0.3, 0.9, weather.x);
        if detail_weight > 0.0 {
            let first = p + sun * 210.0;
            let second = p + sun * 980.0;
            let local_path = 1.6 * lower_cloud(
                first + drift, length(first) - radii.x, weather, footprint + 90.0, true,
            ).density
                + 4.2 * lower_cloud(
                    second + drift, length(second) - radii.x, weather, footprint + 420.0, false,
                ).density;
            light_path = mix(light_path, local_path, detail_weight);
        }
    }
    let shade = 0.13 + 0.87 * exp(-light_path);
    let sky_strength = mix(0.12, 1.0, smoothstep(-0.17, 0.37, zenith));
    let sky = sky_strength * mix(
        vec3<f32>(0.16, 0.21, 0.29), vec3<f32>(0.37, 0.43, 0.51), height,
    );
    var direct = vec3<f32>(1.0);
    @if(ATMOSPHERE)
    direct = sample_transmittance_lut(radii.x + elevation, max(zenith, horizon + 0.001));
    return sky + sunlight * direct * daylight * shade * phase;
}

// The atmosphere pass shades the opaque background before transparent meshes.
// Apply the air between the camera and the cloud to the cloud's own radiance.
fn aerial_perspective(
    eye: vec3<f32>, ray: vec3<f32>, t: f32, sun: vec3<f32>,
) -> mat2x3<f32> {
    let p = eye + ray * t;
    let r = length(p);
    var transmission = vec3<f32>(exp(-t / 66000.0));
    @if(ATMOSPHERE) {
        let entry = max(sphere_span(eye, ray, atmosphere.outer_radius).x, 0.0);
        let origin = eye + ray * min(t, entry);
        let origin_r = clamp(length(origin), radii.x + 1.0, atmosphere.outer_radius);
        // Sample toward space along the same line. Reverse the direction for
        // rays that would enter the planet so the LUT never crosses ground.
        let ground = sphere_span(origin, ray, radii.x);
        let direction = select(ray, -ray, ground.y > 0.0);
        let a = sample_transmittance_lut(origin_r, dot(normalize(origin), direction));
        let b = sample_transmittance_lut(r, dot(p / r, direction));
        transmission = clamp(
            select(a / max(b, vec3<f32>(0.0001)), b / max(a, vec3<f32>(0.0001)), ground.y > 0.0),
            vec3<f32>(0.0), vec3<f32>(1.0),
        );
    }
    let daylight = smoothstep(-0.18, 0.36, dot(p / r, sun));
    let haze = mix(vec3<f32>(0.055, 0.084, 0.16), vec3<f32>(0.58, 0.73, 0.98), daylight);
    return mat2x3<f32>(transmission, (1.0 - transmission) * haze);
}

struct CloudFragment {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

struct CloudRay {
    eye: vec3<f32>,
    direction: vec3<f32>,
    cone: f32,
    radius: f32,
    sun: vec3<f32>,
    sunlight: vec3<f32>,
    phase: f32,
    drift: vec3<f32>,
    weather_drift: vec3<f32>,
}

struct Scattering {
    radiance: vec3<f32>,
    transmission: f32,
    mean_hit: f32,
    first_hit: f32,
}

// Split each shell at its hollow center. Marching through empty space between
// cloud decks would undersample the clouds and produce bands at grazing angles.
fn shell_segments(outer: vec2<f32>, inner: vec2<f32>, depth_limit: f32) -> vec4<f32> {
    if inner.y <= 0.0 {
        return vec4<f32>(max(outer.x, 0.0), min(outer.y, depth_limit), 0.0, 0.0);
    }
    return vec4<f32>(
        max(outer.x, 0.0), min(inner.x, depth_limit),
        inner.y, min(outer.y, depth_limit),
    );
}

fn march(
    ray: CloudRay, segment: vec2<f32>, upper: bool, result: ptr<function, Scattering>,
) {
    let span = segment.y - segment.x;
    if span <= 1.0 || (*result).transmission < 0.008 {
        return;
    }
    // The flight samples lie on the same distance grid for every ray.
    // Intersections with the cloud shell clip that grid at each end. Moving
    // past the horizon therefore does not stretch or move all the samples.
    let distance_lod = smoothstep(900.0, 6200.0, segment.x * ray.cone);
    let budget = select(mix(72.0, 9.0, distance_lod), mix(28.0, 7.0, distance_lod), upper);
    let shell_bottom = select(radii.y, radii.x + UPPER_BASE, upper);
    let shell_top = select(radii.y + LOWER_DEPTH, radii.z, upper);
    let anchor = max(max(shell_bottom - ray.radius, ray.radius - shell_top), 0.0);
    let spacing = select(113.0, 355.0, upper);
    let growth = select(0.046, 0.13, upper);
    let distance = max(segment.x - anchor, 0.0);
    let cell = floor(log(1.0 + distance * growth / spacing) / log(1.0 + growth));
    var stride = spacing * pow(1.0 + growth, cell);
    var grid_end = anchor + (stride - spacing) / growth + stride;
    var begin = segment.x;
    for (var step = 1.0; step <= 72.0; step += 1.0) {
        let progress = min(step / budget, 1.0);
        let orbital_end = segment.x + span * progress;
        let local_end = max(min(grid_end, segment.y), begin);
        let finish = select(mix(local_end, orbital_end, distance_lod), segment.y, progress >= 1.0);
        stride *= 1.0 + growth;
        grid_end += stride;
        let ds = finish - begin;
        let t = begin + 0.5 * ds;
        begin = finish;
        let p = ray.eye + ray.direction * t;
        let elevation = length(p) - radii.x;
        let footprint = max(t * ray.cone, ds * 0.56);
        let weather = textureSampleLevel(
            weather_map, weather_sampler, normalize(p + ray.weather_drift),
            log2(max(footprint / WEATHER_TEXEL, 1.0)),
        ).rgb;
        let shifted = p + ray.drift;
        var height = 1.0;
        var density = 0.0;
        if upper {
            density = upper_cloud(shifted, elevation, weather.z, footprint);
        } else {
            // Integrate wide flight-level intervals at two quadrature
            // points. This resolves density changes along the ray without
            // jittering individual pixels or doubling the lighting work.
            let filter_width = 0.289 * ds * smoothstep(300.0, 660.0, ds) * (1.0 - distance_lod);
            let offset = ray.direction * filter_width;
            let first = lower_cloud(
                shifted - offset, length(p - offset) - radii.x, weather.xy, footprint, true,
            );
            density = first.density;
            height = first.height;
            if filter_width > 1.0 {
                let second = lower_cloud(
                    shifted + offset, length(p + offset) - radii.x, weather.xy, footprint, true,
                );
                density = (first.density + second.density) * 0.5;
                height = (first.height + second.height) * 0.5;
            }
        }
        if density > 0.0002 {
            let opacity = 1.0 - exp(-density * ds * select(0.0056, 0.00120, upper));
            let weight = (*result).transmission * opacity;
            (*result).radiance += weight * cloud_light(
                p, ray.drift, elevation, weather.xy, footprint, height, upper,
                ray.sun, ray.sunlight, ray.phase,
            );
            (*result).mean_hit += weight * t;
            (*result).transmission *= 1.0 - opacity;
            if (*result).first_hit < 0.0 && opacity > 0.004 {
                (*result).first_hit = t;
            }
        }
        if finish >= segment.y || (*result).transmission < 0.008 {
            break;
        }
    }
}

@fragment
fn fragment(input: CloudVertex) -> CloudFragment {
    let eye = view.world_position - input.planet_center;
    // Reconstruct the perspective ray in view space before rotating it. The
    // ray and its footprint are independent of the proxy's tessellation.
    let ndc = frag_coord_to_ndc(input.clip);
    let view_ray = (view.view_from_clip * vec4<f32>(ndc.xy, 0.0, 1.0)).xyz;
    let direction = normalize((view.world_from_view * vec4<f32>(view_ray, 0.0)).xyz);
    let cone = 2.0 / (min(
        view.viewport.z * view.clip_from_view[0].x,
        view.viewport.w * view.clip_from_view[1].y,
    ) * length(view_ray));
    let outer = sphere_span(eye, direction, radii.z);
    if outer.y <= 0.0 {
        discard;
    }
    let ground = sphere_span(eye, direction, radii.x);
    var limit = outer.y;
    if ground.x > 0.0 {
        limit = min(limit, ground.x);
    } else if length(eye) < radii.x && dot(eye, direction) < 0.0 {
        discard;
    }
    let upper = shell_segments(outer, sphere_span(eye, direction, radii.x + UPPER_BASE), limit);
    let lower = shell_segments(
        sphere_span(eye, direction, radii.y + LOWER_DEPTH),
        sphere_span(eye, direction, radii.y),
        limit,
    );

    var sun = normalize(eye);
    var sunlight = vec3<f32>(0.0);
    if lights.n_directional_lights > 0u {
        let light = lights.directional_lights[0];
        sun = light.direction_to_light;
        sunlight = 0.108 * light.color.rgb * view.exposure;
    }
    let phase = 0.91 + 0.57 * pow(saturate(dot(direction, sun)), 6.0);
    let ray = CloudRay(
        eye, direction, cone, length(eye), sun, sunlight, phase,
        globals.time * vec3<f32>(-4.8, 1.1, 3.2),
        globals.time * vec3<f32>(-2.6, 0.6, 1.7),
    );
    var result = Scattering(vec3<f32>(0.0), 1.0, 0.0, -1.0);
    march(ray, upper.xy, true, &result);
    march(ray, lower.xy, false, &result);
    march(ray, lower.zw, false, &result);
    march(ray, upper.zw, true, &result);

    let alpha = 1.0 - result.transmission;
    if alpha < 0.0025 {
        discard;
    }
    let hit = result.mean_hit / alpha;
    let air = aerial_perspective(eye, direction, hit, sun);
    let color = result.radiance * air[0] + alpha * air[1];
    let world_hit = view.world_position + direction * select(hit, result.first_hit, result.first_hit >= 0.0);
    let clip = position_world_to_clip(world_hit);
    return CloudFragment(vec4<f32>(color, alpha), saturate(clip.z / clip.w));
}

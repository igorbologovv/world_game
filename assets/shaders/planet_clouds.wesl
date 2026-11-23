import bevy_pbr::render::forward_io::{Vertex, decompress_vertex};
import bevy_pbr::render::mesh_functions::get_world_from_local;
import bevy_pbr::render::mesh_view_bindings::{globals, view};
import bevy_pbr::render::view_transformations::position_world_to_clip;

struct CloudSettings {
    // Ground, cloud base, cloud top, and reciprocal cloud thickness.
    radii: vec4<f32>,
    sun: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0)
var<uniform> clouds: CloudSettings;
@group(constants::MATERIAL_BIND_GROUP) @binding(1)
var noise: texture_3d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2)
var noise_sampler: sampler;

struct CloudVertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) planet_position: vec3<f32>,
    @location(1) @interpolate(flat) planet_center: vec3<f32>,
}

@vertex
fn vertex(input: Vertex) -> CloudVertex {
    let mesh = decompress_vertex(input, input.instance_index);
    let transform = get_world_from_local(mesh.instance_index);
    let world = (transform * vec4<f32>(mesh.position, 1.0)).xyz;
    return CloudVertex(position_world_to_clip(world), mesh.position, transform[3].xyz);
}

fn noise_at(p: vec3<f32>) -> vec2<f32> {
    return textureSampleLevel(noise, noise_sampler, p, 0.0).rg;
}

fn density(p: vec3<f32>, drift: vec3<f32>) -> f32 {
    let radius = length(p);
    let height = (radius - clouds.radii.y) * clouds.radii.w;
    let ground_position = p * (clouds.radii.x / radius);
    let weather = noise_at((ground_position + drift * 0.36) / 52000.0).x;
    // Larger weather cells collect the clouds into fields with open sky in between.
    if weather < 0.30 || height <= 0.0 || height >= 1.0 {
        return 0.0;
    }
    let q = p + drift;
    let billows = noise_at(q / 9400.0).x;
    let curls = noise_at(q / 4300.0).y;
    let wisps = noise_at(q / 1550.0).y;
    let threshold = mix(0.77, 0.37, smoothstep(0.32, 0.76, weather));
    let shape = billows + 0.26 * (curls - 0.5) + 0.12 * (wisps - 0.5);
    // The rising columns share a fairly flat base and thin out near their tops.
    let base = smoothstep(0.0, 0.17, height);
    let top = 1.0 - smoothstep(0.36, 0.98, height);
    return base * top * smoothstep(0.0, 0.20, shape - threshold - 0.12 * height);
}

// Return distances along the ray to the near and far intersections.
fn sphere_span(eye: vec3<f32>, ray: vec3<f32>, radius: f32) -> vec2<f32> {
    let center = -dot(eye, ray);
    let closest = eye + ray * center;
    let half_squared = radius * radius - dot(closest, closest);
    if half_squared <= 0.0 {
        return vec2<f32>(0.0);
    }
    let half_width = sqrt(half_squared);
    return vec2<f32>(center - half_width, center + half_width);
}

struct CloudFragment {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@fragment
fn fragment(input: CloudVertex) -> CloudFragment {
    let eye = view.world_position - input.planet_center;
    let ray = normalize(input.planet_position - eye);
    let outer = sphere_span(eye, ray, clouds.radii.z);
    var start = max(outer.x, 0.0);
    var end = outer.y;
    let ground = sphere_span(eye, ray, clouds.radii.x);
    if ground.x > 0.0 {
        end = min(end, ground.x);
    } else if length(eye) < clouds.radii.x && dot(eye, ray) < 0.0 {
        discard;
    }
    let inner = sphere_span(eye, ray, clouds.radii.y);
    if start >= inner.x && start < inner.y {
        start = inner.y;
    }
    if start >= end {
        discard;
    }

    // Skip the empty space below cloud base. Both intervals can be visible
    // at the planet's limb, where the ray misses the ground.
    let gap_start = clamp(inner.x, start, end);
    let gap = max(clamp(inner.y, start, end) - gap_start, 0.0);
    let span = end - start - gap;
    if span < 1.0 {
        discard;
    }

    let sun = clouds.sun.xyz;
    let drift = globals.time * vec3<f32>(-5.2, 0.8, 3.1);
    let steps = ceil(clamp(span / 150.0, 8.0, 88.0));
    let step_size = span / steps;
    // Offset adjacent rays slightly to break up bands along grazing sightlines.
    let pixel = floor(input.clip.xy);
    let jitter = fract(52.9829189 * fract(dot(pixel, vec2<f32>(0.06711056, 0.00583715))));
    let offset = mix(0.24, 0.76, jitter);
    let towards_sun = pow(saturate(dot(ray, sun)), 6.0);
    var color = vec3<f32>(0.0);
    var transmission = 1.0;
    var first_hit = -1.0;
    for (var step = 0.0; step < steps; step += 1.0) {
        let distance = start + (step + offset) * step_size;
        let t = distance + select(0.0, gap, distance >= gap_start);
        let p = eye + ray * t;
        let amount = density(p, drift);
        if amount < 0.002 {
            continue;
        }
        if first_hit < 0.0 {
            first_hit = t;
        }
        let optical_depth = amount * step_size * 0.0062;
        let opacity = 1.0 - exp(-optical_depth);
        // A short march toward the sun softens lighting within each billow.
        let sun_depth = 1.65 * density(p + sun * 210.0, drift)
            + 2.35 * density(p + sun * 550.0, drift);
        let daylight = smoothstep(-0.08, 0.44, dot(normalize(p), sun));
        let skylight = vec3<f32>(0.23, 0.34, 0.50) * mix(0.16, 1.0, daylight);
        let direct = vec3<f32>(1.21, 1.11, 0.94) * daylight
            * (0.21 + 0.79 * exp(-sun_depth)) * (1.0 + 0.33 * towards_sun);
        color += transmission * opacity * (skylight + direct);
        transmission *= 1.0 - opacity;
        if transmission < 0.018 {
            break;
        }
    }
    let alpha = 1.0 - transmission;
    if alpha < 0.012 {
        discard;
    }

    // A long ground-level sightline picks up blue atmospheric haze. From
    // orbit the distance through clear space contributes no extra haze.
    let below = 1.0 - smoothstep(clouds.radii.y, clouds.radii.z, length(eye));
    let clear = exp(-first_hit * below / 24000.0);
    let haze = vec3<f32>(0.23, 0.39, 0.62);
    let contrast = mix(0.47, 1.0, clear);
    let world_hit = view.world_position + ray * first_hit;
    let clip = position_world_to_clip(world_hit);
    // Test against the terrain at the cloud itself, rather than at the proxy.
    return CloudFragment(vec4<f32>(mix(haze * alpha, color, clear) * contrast,
                                   alpha * contrast), saturate(clip.z / clip.w));
}

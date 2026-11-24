import super::water_common::{wave_slope, water_normal, water_light};
import super::ocean_projection::{cube_chart, ocean_plane, atlas_gradient, OceanPlane};
import super::ocean_waves::{TAU, wave_position, wave_phase, wind_field, swell_field};
import bevy_pbr::render::forward_io::{Vertex, decompress_vertex};
import bevy_pbr::render::mesh_functions::get_world_from_local;
import bevy_pbr::render::mesh_view_bindings as bindings;
import bevy_pbr::render::view_transformations::position_world_to_clip;
import bevy_render::maths::quat_rotate;

@group(constants::MATERIAL_BIND_GROUP) @binding(0)
var ocean_mask: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(1)
var ocean_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(2)
var wave_normals: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(3)
var wave_sampler: sampler;

struct OceanSettings {
    // Water radius, disk radius, maximum disk altitude, and lattice segments.
    geometry: vec4<f32>,
    // Cube warp and terrain texel-center correction.
    projection: vec2<f32>,
    // Unit radial direction and exact signed height of the floating origin.
    origin_radial: vec4<f32>,
    // Planet-space unit direction and wrapped start time of the storm fade.
    storm_center: vec4<f32>,
    // Starting strength, target strength, fade duration, and clock wrap.
    storm_fade: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(4)
var<uniform> settings: OceanSettings;

const INV_PI: f32 = 0.31830989;

struct OceanVertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) planet_center: vec3<f32>,
    @location(3) displacement: f32,
}

struct Shore {
    depth: f32,
    distance: f32,
    offshore: vec3<f32>,
    grade: f32,
    mask_distance: f32,
}

struct Surf {
    rise: f32,
    face: f32,
    breaker: f32,
    wash: f32,
}

struct RippleWarp {
    offset: vec2<f32>,
    u: vec3<f32>,
    v: vec3<f32>,
}

fn decode_depth(bytes: vec2<f32>) -> f32 {
    return dot(bytes, vec2<f32>(65280.0, 255.0)) * 0.25 - 8192.0;
}

fn radial_altitude(world: vec3<f32>) -> f32 {
    let origin = settings.origin_radial;
    let r = settings.geometry.x + origin.w;
    // Evaluate |origin + world| - |origin| using a short vector in the
    // numerator. Subtracting two rounded planet radii moved the whole sheet
    // in 25 cm increments as a level camera moved within its grid cell.
    let delta = (2.0 * r * dot(origin.xyz, world) + dot(world, world))
        / max(r + length(r * origin.xyz + world), 1.0);
    return origin.w + delta;
}

fn storm_strength(up: vec3<f32>) -> f32 {
    let fade = settings.storm_fade;
    if fade.x == 0.0 && fade.y == 0.0 {
        return 0.0;
    }
    var strength = fade.y;
    if fade.x != fade.y {
        let elapsed = bindings::globals.time - settings.storm_center.w;
        let age = select(elapsed, elapsed + fade.w, elapsed < 0.0);
        strength = mix(fade.x, fade.y, smoothstep(0.0, fade.z, age));
    }
    // Chord distance is well conditioned for small regions on this planet.
    // Using the radial direction keeps the front fixed through origin shifts.
    let distance = length(up - settings.storm_center.xyz) * settings.geometry.x;
    return strength * (1.0 - smoothstep(22000.0, 65000.0, distance));
}

fn read_shore(plane: OceanPlane, up: vec3<f32>, face: u32, size: f32) -> Shore {
    let uv = plane.uv;
    let nearest = vec2<i32>(round(uv));
    let origin = vec2<i32>(i32(face % 3u), i32(face / 3u)) * i32(size);
    let last = origin + i32(size) - 1;
    let base = vec2<i32>(floor(uv));
    let lo = clamp(base, origin, last);
    let hi = clamp(base + 1, origin, last);
    let w = fract(uv);
    let a = textureLoad(ocean_mask, lo, 0);
    let b = textureLoad(ocean_mask, vec2<i32>(hi.x, lo.y), 0);
    let c = textureLoad(ocean_mask, vec2<i32>(lo.x, hi.y), 0);
    let d = textureLoad(ocean_mask, hi, 0);
    let high = vec2<f32>(decode_depth(a.ba), decode_depth(b.ba));
    let low = vec2<f32>(decode_depth(c.ba), decode_depth(d.ba));
    let depth = mix(mix(high.x, high.y, w.x), mix(low.x, low.y, w.x), w.y) + plane.altitude;
    let grad = atlas_gradient(up, face, size, settings.geometry.x, settings.projection.x);
    let chart_slope = vec2<f32>(mix(high.y - high.x, low.y - low.x, w.y),
                               mix(low.x - high.x, low.y - high.y, w.x));
    let slope = chart_slope.x * grad.u + chart_slope.y * grad.v
        + up - plane.normal / dot(plane.normal, up);
    let grade = length(slope);
    var distance = max(depth, 0.0) / max(grade, 0.0035);
    var offshore = slope / max(grade, 0.0001);
    var mask_distance = 1e7;

    // A baked mask edge can meet water before the bottom rises to the
    // surface. Detect that edge in the same four texels used for depth.
    let row = select(vec2<f32>(c.r, d.r), vec2<f32>(a.r, b.r), nearest.y == lo.y);
    let col = select(vec2<f32>(b.r, d.r), vec2<f32>(a.r, c.r), nearest.x == lo.x);
    let offset = uv - vec2<f32>(nearest);
    let edge = (0.5 - abs(offset)) / vec2<f32>(length(grad.u), length(grad.v));
    if select(row.x, row.y, nearest.x == lo.x) < 0.9309 {
        mask_distance = edge.x;
        if edge.x < distance {
            distance = edge.x;
            offshore = -sign(offset.x) * normalize(grad.u);
        }
    }
    if select(col.x, col.y, nearest.y == lo.y) < 0.9309 {
        mask_distance = min(mask_distance, edge.y);
        if edge.y < distance {
            distance = edge.y;
            offshore = -sign(offset.y) * normalize(grad.v);
        }
    }
    return Shore(depth, distance, offshore, grade, mask_distance);
}

fn surf_field(shore: Shore, p: vec3<f32>, wind: vec2<f32>, clock: f32, width: f32, storm: f32) -> Surf {
    let d = shore.distance;
    let rip = sin(wave_phase(p, vec3<f32>(1.0, 2.0, -2.0) / 256.0, clock, 107.0));
    let packet = sin(wave_phase(p, vec3<f32>(1.0, -2.0, -1.0) / 512.0, clock, 63.0));
    // Increasing phase with time moves the crest across the shelf toward land.
    let phase = TAU * (d / 43.0 + fract(clock * 351.0))
        + rip * 0.77 + packet * 0.94 + wind.x * 1.5;
    let shoaling = 1.0 - smoothstep(3.5 + 2.0 * storm, 11.0 + 7.0 * storm, shore.depth);
    let belt = (1.0 - smoothstep(62.0 + 28.0 * storm, 191.0 + 96.0 * storm, d)) * shoaling;
    let swell = sin(phase);
    let strength = (0.86 + packet * 0.14) * belt;
    let resolved = 1.0 - smoothstep(0.08, 0.47, width / 43.0);
    let rise = (swell + (0.24 + 0.08 * storm) * (2.0 * swell * swell - 1.0))
        * strength * (1.0 + 1.30 * storm);
    let face = (0.17 + 0.14 * storm) * cos(phase) * (1.0 + 0.96 * swell) * strength * resolved;
    let crest = mix(0.21 + 0.08 * storm, smoothstep(0.50 + 0.07 * storm, 0.86 + 0.03 * storm, swell), resolved);
    let breaker = strength * crest * (1.0 + 0.37 * storm);
    let wash = (1.0 - smoothstep(1.0, 17.0 + 14.0 * storm + 5.0 * rip + 4.0 * swell * resolved, d))
        * (0.45 + 0.46 * mix(0.32, saturate(sin(phase - 1.5)), resolved)) * shoaling
        * (1.0 + 0.28 * storm);
    return Surf(rise * resolved, face, breaker, wash);
}

fn foam_lattice(cell: vec3<i32>) -> f32 {
    // The foam field uses 8 m cells in the same world-locked 32768 m domain.
    let c = bitcast<vec3<u32>>(cell) & vec3<u32>(4095u);
    var h = (c.x * 1597334677u) ^ (c.y * 3812015801u) ^ (c.z * 2798796415u);
    h = (h ^ (h >> 16u)) * 2246822519u;
    return f32((h ^ (h >> 13u)) & 65535u) / 65535.0;
}

fn foam_density(p: vec3<f32>) -> f32 {
    let q = p / 8.0;
    let cell = vec3<i32>(floor(q));
    let f = fract(q);
    let w = f * f * (3.0 - 2.0 * f);
    let low = mix(foam_lattice(cell), foam_lattice(cell + vec3<i32>(1, 0, 0)), w.x);
    let high = mix(foam_lattice(cell + vec3<i32>(0, 1, 0)), foam_lattice(cell + vec3<i32>(1, 1, 0)), w.x);
    let back_low = mix(foam_lattice(cell + vec3<i32>(0, 0, 1)), foam_lattice(cell + vec3<i32>(1, 0, 1)), w.x);
    let back_high = mix(foam_lattice(cell + vec3<i32>(0, 1, 1)), foam_lattice(cell + vec3<i32>(1, 1, 1)), w.x);
    return mix(mix(low, high, w.y), mix(back_low, back_high, w.y), w.z);
}

fn sea_mist(p: vec3<f32>, eye: vec3<f32>, facing: f32, range: f32, clock: f32,
            storm: f32, breaking: f32) -> f32 {
    // Sample a broad, wind-driven density halfway through a low spray layer.
    // Integrating over the ray segment gives a soft haze without particles,
    // screen noise, or a separate transparent pass.
    let path = min(range * 0.70, 8.5 / max(facing, 0.065));
    let q = p + eye * (path * 0.47);
    let long_streak = sin(wave_phase(q, vec3<f32>(-1.0, 2.0, -3.0) / 512.0, clock, 271.0));
    let billow = sin(wave_phase(q, vec3<f32>(1.0, -1.0, -2.0) / 256.0, clock, 389.0)
                     + 0.85 * long_streak);
    let gust = smoothstep(-0.28, 0.88, 0.59 * long_streak + 0.41 * billow);
    let density = (0.15 + 0.70 * gust) * (0.33 + 0.67 * saturate(breaking));
    return storm * (1.0 - exp(-path * density * 0.016));
}

@vertex
fn vertex(input: Vertex) -> OceanVertex {
    let mesh = decompress_vertex(input, input.instance_index);
    let transform = get_world_from_local(mesh.instance_index);
    let center = transform[3].xyz;
@if(VERTEX_UVS_B) {
    let radius = settings.geometry.x;
    let eye = bindings::view.world_position;
    let center_to_eye = eye - center;
    let eye_radius = length(center_to_eye);
    let radial = center_to_eye / eye_radius;
    let altitude = radial_altitude(eye);
    let axis = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), abs(radial.y) > 0.98);
    let tangent = normalize(cross(axis, radial));
    let offset = tangent * mesh.position.x + cross(radial, tangent) * mesh.position.y;
    let r2 = dot(mesh.position.xy, mesh.position.xy);
    let p_radius = sqrt(radius * radius + r2);
    let k = radius / p_radius;
    let direction = (radial + offset / radius) * k;
    let size = vec2<f32>(textureDimensions(ocean_mask));
    let chart = cube_chart(direction, settings.projection.x);
    let plane = ocean_plane(direction, chart, settings.geometry.w, radius,
                            size.y * 0.5, settings.projection.x, settings.projection.y);
    let chord = -r2 * k / (p_radius + radius);
    let world = eye + offset * k + radial * (chord - altitude)
        + direction * plane.altitude;
    let sample = textureSampleLevel(ocean_mask, ocean_sampler, (plane.uv + 0.5) / size, 0.0);
    var depth = decode_depth(sample.ba) + plane.altitude;
    let distance = sqrt(r2);
    let spacing = max(mesh.uv_b.x, mesh.uv_b.y);
    var rise = 0.0;
    if sample.r > 0.49 && depth > -2.0 && distance < settings.geometry.y - 200.0 {
        let storm = storm_strength(direction);
        var shore = Shore(depth, 1e7, vec3<f32>(0.0), 0.0, 1e7);
        let near_contact = (1.0 - smoothstep(12.0, 18.0, spacing))
            * (1.0 - smoothstep(12.0 + 7.0 * storm, 19.0 + 7.0 * storm, depth));
        let depth_weight = max(near_contact, select(0.0, 1.0, sample.r < 0.995));
        if depth_weight > 0.0 {
            shore = read_shore(plane, direction, chart.face, size.y * 0.5);
            depth = mix(depth, shore.depth, depth_weight);
        }
        let clock = bindings::globals.time / 3600.0;
        let p = wave_position(world, center, 32768.0);
        let wind = wind_field(p, clock);
        let edge = 1.0 - smoothstep(settings.geometry.y * 0.76, settings.geometry.y - 200.0, distance);
        let across_ring = offset / max(distance, 0.001);
        let around_ring = cross(radial, across_ring);
        if depth > 2.25 {
            let swell = swell_field(p, wind, clock, across_ring * mesh.uv_b.x,
                                    around_ring * mesh.uv_b.y, 0.0, true, storm);
            rise = swell.height * smoothstep(2.25, 23.0, depth) * edge;
        }
        if depth > 0.30 && depth < 12.0 + 7.0 * storm && spacing < 18.0
            && shore.distance < 192.0 + 96.0 * storm {
            let surf = surf_field(shore, p, wind, clock, 0.0, storm);
            rise += surf.rise * smoothstep(0.3, 1.6, shore.depth)
                * (1.0 - smoothstep(6.0, 18.0, spacing));
        }
        // Keep wave troughs above the bottom. Leave the baked contact points
        // and the edge of the detail disk on the original ocean triangles.
        let altitude_fade = 1.0 - smoothstep(settings.geometry.z * 0.65, settings.geometry.z, altitude);
        let limit = max(depth, 0.0);
        rise = clamp(rise, -0.42 * limit, 0.56 * limit) * altitude_fade
            * smoothstep(0.0, 35.0, shore.mask_distance);
    }
    let displaced = world + direction * rise;
    return OceanVertex(position_world_to_clip(displaced), displaced, plane.uv, center, rise);
} @else {
    let world = (transform * vec4<f32>(mesh.position, 1.0)).xyz;
    return OceanVertex(position_world_to_clip(world), world, mesh.uv, center, 0.0);
}
}

fn normal_detail(
    p: vec3<f32>,
    dx: vec3<f32>,
    dy: vec3<f32>,
    u: vec3<f32>,
    v: vec3<f32>,
    offset: vec2<f32>,
    warp: RippleWarp,
    filter_width: f32,
) -> vec4<f32> {
    let uv = vec2<f32>(dot(p, u), dot(p, v)) + offset + warp.offset;
    let du = vec2<f32>(dot(dx, u + warp.u), dot(dx, v + warp.v));
    let dv = vec2<f32>(dot(dy, u + warp.u), dot(dy, v + warp.v));
    // Match the mip footprint to the warped coordinates, including across
    // repeats. A small margin covers curvature within the pixel.
    return textureSampleGrad(wave_normals, wave_sampler, uv, du * filter_width, dv * filter_width) * 2.0 - 1.0;
}

fn ripple_warp(p: vec3<f32>, clock: f32, a: vec3<f32>, b: vec3<f32>,
               rates: vec2<f32>, strength: vec2<f32>) -> RippleWarp {
    let phase = vec2<f32>(wave_phase(p, a, clock, rates.x), wave_phase(p, b, clock, rates.y));
    let gradient = TAU * strength * cos(phase);
    return RippleWarp(strength * sin(phase), a * gradient.x, b * gradient.y);
}

fn fresnel_water(cosine: f32) -> f32 {
    let grazing = 1.0 - saturate(cosine);
    let grazing2 = grazing * grazing;
    // Schlick Fresnel for air to water, with an index of refraction of 1.333.
    return 0.0204 + 0.9796 * grazing2 * grazing2 * grazing;
}

fn reflected_sky(
    reflected: vec3<f32>,
    up: vec3<f32>,
    roughness: f32,
    sun_up: f32,
    camera_altitude: f32,
) -> vec3<f32> {
    let elevation = smoothstep(0.0, 0.85, dot(reflected, up));
    let daylight = mix(0.09, 1.0, smoothstep(-0.10, 0.20, sun_up));
    let sky = mix(vec3<f32>(0.36, 0.48, 0.58), vec3<f32>(0.095, 0.23, 0.42), elevation) * daylight;
@if(ENVIRONMENT_MAP) {
    let probe = bindings::light_probes.view_cubemap_index;
    // At orbital distances, use the sky at the water surface. The camera's
    // atmosphere cubemap represents its own location, often outside the sky.
    let weight = 1.0 - smoothstep(8000.0, 48000.0, camera_altitude);
    if probe >= 0 && weight > 0.0 {
        var direction = quat_rotate(bindings::light_probes.view_rotation, reflected);
        direction.z = -direction.z;
        let mip = roughness * f32(bindings::light_probes.smallest_specular_mip_level_for_view);
        @if(MULTIPLE_LIGHT_PROBES_IN_ARRAY)
        let radiance = textureSampleLevel(bindings::specular_environment_maps[probe],
                                          bindings::environment_map_sampler, direction, mip).rgb;
        @else
        let radiance = textureSampleLevel(bindings::specular_environment_map,
                                          bindings::environment_map_sampler, direction, mip).rgb;
        return mix(sky, radiance * bindings::light_probes.intensity_for_view * bindings::view.exposure,
                   weight);
    }
}
    return sky;
}

fn sun_glint(normal: vec3<f32>, eye: vec3<f32>, sun: vec3<f32>, roughness: f32) -> f32 {
    let no_l = saturate(dot(normal, sun));
    if no_l == 0.0 {
        return 0.0;
    }
    let halfway = normalize(eye + sun);
    let no_h = saturate(dot(normal, halfway));
    let no_v = max(dot(normal, eye), 0.001);
    let alpha = roughness * roughness;
    let alpha2 = alpha * alpha;
    let denominator = 1.0 - no_h * no_h * (1.0 - alpha2);
    let distribution = INV_PI * alpha2 / (denominator * denominator);
    let visibility = 0.5 / (no_l * sqrt(alpha2 + no_v * no_v * (1.0 - alpha2))
                         + no_v * sqrt(alpha2 + no_l * no_l * (1.0 - alpha2)));
    return distribution * visibility * no_l * fresnel_water(dot(eye, halfway));
}

// Atlases from older bakes also encoded inland water below the ocean coverage.
fn legacy_inland(
    position: vec3<f32>,
    eye: vec3<f32>,
    up: vec3<f32>,
    footprint: f32,
    depth: f32,
) -> vec4<f32> {
    let alpha = mix(0.32, 0.965, 1.0 - exp(-depth * 0.14));
    let tint = mix(vec3<f32>(0.39, 0.52, 0.45), vec3<f32>(0.055, 0.20, 0.19),
                   1.0 - exp(-depth * 0.09));
    let detail = 1.0 - smoothstep(0.45, 3.5, footprint);
    let glancing = smoothstep(0.015, 0.20, dot(up, eye));
    let slope = detail * glancing * (
        wave_slope(position, bindings::globals.time * 0.22,
                   vec3<f32>(0.80, 0.0, 0.60), 12.0, 0.035)
        + wave_slope(position, bindings::globals.time * 0.18,
                     vec3<f32>(-0.48, 0.64, 0.60), 7.3, 0.019)
    );
    return vec4<f32>(water_light(tint, up, water_normal(up, slope), eye, 0.42), alpha);
}


struct Ripples {
    slope: vec3<f32>,
    breakup: f32,
}

fn sampled_ripples(
    p: vec3<f32>, dx: vec3<f32>, dy: vec3<f32>, wind: vec2<f32>, clock: f32, footprint: f32,
) -> Ripples {
    let coverage = 1.0 - smoothstep(1.8, 13.0, footprint);
    var slope = vec3<f32>(0.0);
    var breakup = 0.0;
    if coverage > 0.0 {
        let a_u = vec3<f32>(2.0, 1.0, 2.0);
        let a_v = vec3<f32>(1.0, 0.0, -1.0);
        let k_u = vec3<f32>(2.0, -1.0, 3.0) / 1024.0;
        let k_v = vec3<f32>(-1.0, 2.0, -1.0) / 2048.0;
        let warp = ripple_warp(p, clock, k_u, k_v, vec2<f32>(53.0, 41.0), vec2<f32>(0.37));
        let drift = vec2<f32>(-fract(clock * 73.0), fract(clock * 31.0));
        let broad = normal_detail(p, dx, dy, a_u / 512.0, a_v / 256.0, drift + wind * 0.43, warp, 1.75).rg;
        slope = (normalize(a_u) * broad.x + normalize(a_v) * broad.y) * (0.36 + 0.12 * wind.y) * coverage;
        breakup = 1.8 * broad.x + broad.y;

        let middle = 1.0 - smoothstep(0.85, 5.0, footprint);
        if middle > 0.0 {
            let b_u = vec3<f32>(2.0, -3.0, 1.0);
            let b_v = vec3<f32>(1.0, 1.0, 1.0);
            let offset = vec2<f32>(fract(clock * 139.0), -fract(clock * 79.0)) - wind * 0.28;
            let bent = RippleWarp(warp.offset.yx * 0.89, warp.v * 0.89, warp.u * 0.89);
            let cross_chop = normal_detail(p, dx, dy, b_u / 256.0, b_v / 128.0, offset, bent, 1.95).rg;
            slope += (normalize(b_u) * cross_chop.x + normalize(b_v) * cross_chop.y) * middle * (0.32 - 0.10 * wind.x);
            breakup += (cross_chop.x - cross_chop.y * 0.9) * 1.6 * middle;
        }

        let fine_fade = 1.0 - smoothstep(0.28, 1.8, footprint);
        if fine_fade > 0.0 {
            let u = vec3<f32>(1.0, -2.0, -1.0);
            let v = vec3<f32>(2.0, 0.0, 2.0);
            let offset = vec2<f32>(-fract(clock * 307.0), fract(clock * 229.0)) + wind.yx * 0.47;
            let bend = RippleWarp(-warp.offset * 0.75, -warp.u * 0.75, -warp.v * 0.75);
            let fine = normal_detail(p, dx, dy, u / 64.0, v / 64.0, offset, bend, 2.35).ba;
            slope += (normalize(u) * fine.x + normalize(v) * fine.y) * fine_fade * 0.53;
            breakup += 0.90 * fine_fade * (fine.x + fine.y);
        }
    }
    return Ripples(slope, breakup);
}

@fragment
fn fragment(mesh: OceanVertex) -> @location(0) vec4<f32> {
    let planet_position = mesh.world_position - mesh.planet_center;
    let surface_radius = length(planet_position);
    let up = planet_position / surface_radius;
    let base_position = mesh.world_position - up * mesh.displacement;
    let dx = dpdxCoarse(base_position);
    let dy = dpdyCoarse(base_position);
    let footprint = max(length(dx), length(dy));
    let center_to_eye = bindings::view.world_position - mesh.planet_center;
    let eye_radius = length(center_to_eye);
    let camera_up = center_to_eye / eye_radius;
    let eye_to_surface = mesh.world_position - bindings::view.world_position;
    let view_distance = length(eye_to_surface);
    let eye = -eye_to_surface / max(0.001, view_distance);
    let camera_altitude = radial_altitude(bindings::view.world_position);
    let lateral = eye_to_surface - camera_up * dot(eye_to_surface, camera_up);
    let inside_detail = camera_altitude < settings.geometry.z
        && dot(lateral, lateral) < settings.geometry.y * settings.geometry.y;
    let size = vec2<f32>(textureDimensions(ocean_mask));
    var plane: OceanPlane;
@if(VERTEX_UVS_B) {
    if !inside_detail {
        discard;
    }
    let chart = cube_chart(up, settings.projection.x);
    plane = ocean_plane(up, chart, settings.geometry.w, settings.geometry.x,
                        size.y * 0.5, settings.projection.x, settings.projection.y);
} @else {
    let plane_normal = normalize(cross(dx, dy));
    let sheet_normal = select(plane_normal, -plane_normal, dot(up, plane_normal) < 0.0);
    plane = OceanPlane(mesh.uv, surface_radius - settings.geometry.x, sheet_normal);
    if camera_altitude < 12000.0 {
        plane.altitude = radial_altitude(mesh.world_position);
    }
}

    let uv = plane.uv;
    let nearest = vec2<i32>(round(uv));
    let texel = textureLoad(ocean_mask, nearest, 0);
    if texel.r < 0.5371 {
        discard;
    }
    let filtered = textureSampleLevel(ocean_mask, ocean_sampler, (uv + 0.5) / size, 0.0);
    if texel.r <= 0.9309 {
        @if(VERTEX_UVS_B) { discard; }
        @else {
            let encoded = mix(filtered.g, texel.g, smoothstep(50.0, 120.0, footprint));
            return legacy_inland(planet_position, eye, up, footprint, max(encoded * 96.0 - 16.0, 0.0));
        }
    }
    @if(!VERTEX_UVS_B) { if inside_detail { discard; } }

    let coarse_depth = decode_depth(filtered.ba) + plane.altitude;
    var shore = Shore(coarse_depth, 1e7, vec3<f32>(0.0), 0.0, 1e7);
    if (footprint < 55.0 && coarse_depth < 44.0) || filtered.r < 0.995 {
        let face = vec2<u32>(nearest) / u32(size.y * 0.5);
        shore = read_shore(plane, up, face.x + 3u * face.y, size.y * 0.5);
    }
    let depth = max(shore.depth, 0.0);
    // Fade nearly coplanar seabed contact and antialias the inside of the
    // baked edge. Fully transparent fragments need no wave or reflection work.
    let contact_width = clamp(shore.grade * max(0.6, footprint * 0.7), 0.016, 0.16);
    let contact = mix(smoothstep(-0.012, contact_width, shore.depth), 1.0,
                      smoothstep(18.0, 52.0, footprint));
    let coverage = saturate(shore.mask_distance / max(0.10, footprint * 0.65));
    let visibility = contact * coverage;
    if visibility == 0.0 {
        discard;
    }

    let storm = storm_strength(up);
    let clock = bindings::globals.time / 3600.0;
    let facing = saturate(dot(up, eye));
    var slope = vec3<f32>(0.0);
    var swell_height = 0.0;
    var foam = 0.0;
    var surf_shade = 1.0;
    var mist = 0.0;
    if footprint < 440.0 {
        let p = wave_position(base_position, mesh.planet_center, 32768.0);
        let wind = wind_field(p, clock);
        // At a glancing angle a single directional derivative can nearly
        // vanish. A small angular footprint keeps distant short-wave detail
        // from reappearing and strobing when the camera changes heading.
        let minimum_width = view_distance * 0.0048;
        let swell = swell_field(p, wind, clock, dx, dy, minimum_width, false, storm);
        let swell_fade = 1.0 - smoothstep(240.0, 440.0, footprint);
        swell_height = swell.height * swell_fade;
        let deeper = mix(0.27, 1.0, smoothstep(1.0, 21.0, depth));
        slope = swell.slope * deeper * swell_fade;
        let ripples = sampled_ripples(p, dx, dy, wind, clock, footprint);
        let ripple_visibility = mix(0.22, 1.0, smoothstep(0.025, 0.21, facing));
        slope += ripples.slope * mix(0.60, 1.0, deeper) * ripple_visibility
            * (1.0 + 0.45 * storm);
        let lump_k = vec3<f32>(3.0, 1.0, -2.0) / 128.0;
        let lump_width = max(max(abs(dot(dx, lump_k)), abs(dot(dy, lump_k))), minimum_width * length(lump_k));
        let lumps = sin(wave_phase(p, lump_k, clock, 107.0)) * (1.0 - smoothstep(0.10, 0.40, lump_width));
        let cap = smoothstep(5.1, 8.3, swell_height) * smoothstep(0.10, 0.71, wind.x);
        let cap_fade = 1.0 - smoothstep(1650.0, 7800.0, view_distance);
        let froth = smoothstep(0.14, 0.6, lumps + ripples.breakup);
        var whitecaps = cap * (0.13 + 0.47 * froth);
        var spray_source = 0.0;
        if storm > 0.0 {
            // The short wind waves break on the backs of larger swells.
            // Their crest signal is already filtered with the wave spectrum.
            let crest = swell.crest_signal * swell_fade;
            let gust = mix(0.30, 1.0, smoothstep(-0.57, 0.68, wind.x + 0.38 * wind.y));
            let lifted = smoothstep(0.35, 5.4, swell_height);
            let active_cap = smoothstep(0.047, 0.142, crest) * gust * lifted;
            let foam_trail = smoothstep(0.013, 0.082, crest) * (0.10 + 0.25 * froth);
            whitecaps = mix(whitecaps, max(active_cap * (0.53 + 0.44 * froth), foam_trail * lifted * gust), storm);
            spray_source = (0.41 * active_cap + 0.28 * lifted * gust) * cap_fade;
        }
        foam = whitecaps * cap_fade * (1.0 - smoothstep(14.0, 55.0, footprint))
            * smoothstep(13.0, 45.0, depth);

        if shore.distance < 192.0 + 96.0 * storm {
            let across_foam = max(abs(dot(dx, shore.offshore)), abs(dot(dy, shore.offshore)));
            let surf = surf_field(shore, p, wind, clock, across_foam, storm);
            let surf_detail = 1.0 - smoothstep(4.0, 26.0, footprint);
            slope += shore.offshore * surf.face * surf_detail;
            let lace = smoothstep(-0.48, 0.49, ripples.breakup + 0.23 * lumps);
            var density = 0.56;
            if footprint < 6.5 {
                let churn = foam_density(p + shore.offshore * surf.rise * 2.7);
                density = mix(density, churn, 1.0 - smoothstep(1.8, 6.5, footprint));
            }
            let pockets = smoothstep(0.22, 0.83, 0.72 * density + 0.28 * lace);
            let glancing_fade = 1.0 - smoothstep(24.0, 55.0, footprint);
            let breaking = surf.breaker * (0.30 + 0.16 * storm + 0.97 * pockets);
            let wash = surf.wash * mix(0.42 + 0.62 * pockets,
                                      0.32 + 0.77 * smoothstep(0.20, 0.77, pockets), storm);
            foam = saturate(foam + glancing_fade * saturate(breaking + wash));
            surf_shade = 1.0 - (0.34 + 0.16 * storm) * saturate(surf.rise * 2.8) * (1.0 - foam);
            spray_source += glancing_fade * (0.82 * surf.breaker + 0.42 * surf.wash);
        }
        if storm > 0.0 && footprint < 62.0 && camera_altitude < 160.0 {
            // Do not resolve the gust pattern beyond its pixel footprint.
            let mist_fade = (1.0 - smoothstep(18.0, 62.0, footprint))
                * (1.0 - smoothstep(48.0, 160.0, camera_altitude));
            mist = sea_mist(p, eye, facing, view_distance, clock, storm, spray_source) * mist_fade;
        }
    }

    // Keep the low-frequency swell visible at grazing angles. Suppress
    // high-frequency glints as footprints become large.
    let normal = water_normal(up, slope * mix(0.61, 1.0, smoothstep(0.0, 0.16, facing)));
    let reflected = reflect(-eye, normal);
    let fresnel = min(fresnel_water(dot(normal, eye)), 0.94);
    let base_roughness = mix(0.17, 0.34, smoothstep(0.32, 26.0, footprint)) + foam * 0.21
        + 0.06 * (1.0 - smoothstep(0.035, 0.18, facing)) + 0.076 * storm;
    // Broaden the GGX lobe and the cubemap footprint where the resolved
    // normal changes within a quad. Calm close water keeps its sharper glints.
    let nx = dpdxCoarse(normal);
    let ny = dpdyCoarse(normal);
    let variance = min(0.038, 0.155 * (dot(nx, nx) + dot(ny, ny)));
    let alpha2 = base_roughness * base_roughness;
    let roughness = sqrt(sqrt(alpha2 * alpha2 + variance));

    var sun = vec3<f32>(0.0, 1.0, 0.0);
    var sun_color = vec3<f32>(0.0);
    var sun_up = -1.0;
    if bindings::lights.n_directional_lights > 0u {
        let light = &bindings::lights.directional_lights[0];
        sun = (*light).direction_to_light;
        sun_color = (*light).color.rgb * bindings::view.exposure;
        sun_up = dot(up, sun);
    }
    let sky = reflected_sky(reflected, up, roughness, sun_up, camera_altitude)
        * mix(vec3<f32>(1.0), vec3<f32>(0.52, 0.59, 0.64), storm);
    var specular = vec3<f32>(0.0);
    if sun_up > 0.0 {
        let color = mix(vec3<f32>(1.0, 0.54, 0.28), vec3<f32>(0.86, 0.82, 0.75),
                        smoothstep(0.03, 0.65, sun_up));
        let max_glint = mix(5.0, 1.9, smoothstep(18000.0, 160000.0, camera_altitude));
        let sun_visibility = smoothstep(0.0, 0.12, sun_up);
        specular = min(sun_color * color * sun_visibility * sun_glint(normal, eye, sun, roughness),
                       vec3<f32>(max_glint)) * (1.0 - 0.69 * storm);
    }

    // Clear water transmits the bottom at the contact line. A turquoise shelf
    // gives way to blue offshore as the optical path absorbs red and green.
    let path = max(depth + mesh.displacement, 0.0) / mix(0.59, 1.0, facing);
    let clarity = exp(-path * vec3<f32>(0.205, 0.103, 0.068));
    let deep_tint = mix(vec3<f32>(0.0055, 0.032, 0.064), vec3<f32>(0.0038, 0.013, 0.023), storm);
    let shelf_tint = mix(vec3<f32>(0.085, 0.39, 0.29), vec3<f32>(0.038, 0.161, 0.151), storm);
    let tint = mix(deep_tint, shelf_tint, clarity);
    let daylight = mix(0.15, 1.0, sqrt(saturate(sun_up)));
    let opacity = 0.15 + 0.847 * (1.0 - exp(-path * (0.56 + 0.33 * storm)));
    let body = (1.0 - fresnel) * opacity;
    let crest_light = 0.91 + 0.12 * tanh(swell_height * 0.32);
    let reflection = sky * mix(0.73, 1.03, smoothstep(-0.05, 0.18, dot(reflected, up)));
    let surface = (tint * (daylight * body * crest_light) + reflection * fresnel) * surf_shade + specular;
    let foam_tint = mix(vec3<f32>(1.20, 1.28, 1.22), vec3<f32>(0.88, 0.97, 0.99), storm);
    let foam_color = foam_tint * (0.66 + 0.42 * daylight) + sky * 0.06;
    let alpha = mix(body + fresnel, 1.0, foam);
    let ocean_color = mix(surface, foam_color, foam);
    let mist_color = vec3<f32>(0.39, 0.49, 0.55) * (0.36 + 0.64 * daylight);
    let mist_alpha = mix(alpha, 1.0, mist);
    return vec4<f32>(mix(ocean_color, mist_color, mist) / mist_alpha, mist_alpha * visibility);
}

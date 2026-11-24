import bevy_pbr::render::forward_io::{VertexOutput, FragmentOutput};
import bevy_pbr::render::mesh_view_bindings::{lights, view};
import bevy_pbr::render::pbr_types::{PbrInput, pbr_input_new, STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT};
import bevy_pbr::render::pbr_functions::{calculate_view, apply_pbr_lighting, main_pass_post_lighting_processing};

@group(constants::MATERIAL_BIND_GROUP) @binding(0)
var terrain_albedo: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(1)
var terrain_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(2)
var terrain_normal_roughness: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(3)
var detail_albedo: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4)
var detail_normal_roughness: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(5)
var<uniform> detail_enabled: u32;
@group(constants::MATERIAL_BIND_GROUP) @binding(6)
var<uniform> terrain_debug_mode: u32;
@group(constants::MATERIAL_BIND_GROUP) @binding(7)
var<uniform> planet_radius: f32;
@group(constants::MATERIAL_BIND_GROUP) @binding(8)
var canopy_atlas: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(9)
var canopy_sampler: sampler;

struct CanopyCascade {
    east: vec4<f32>,
    north: vec4<f32>,
    center: vec4<f32>,
}

const CANOPY_CASCADES = 6u;
struct CanopyLayout {
    cascades: array<CanopyCascade, CANOPY_CASCADES>,
    fade: vec4<f32>,
    clearing: vec4<f32>,
}
@group(constants::MATERIAL_BIND_GROUP) @binding(10)
var<uniform> canopy_layout: CanopyLayout;
@group(constants::MATERIAL_BIND_GROUP) @binding(11)
var crown_detail: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(12)
var crown_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(13)
var globe_canopy: texture_cube<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(14)
var globe_sampler: sampler;

// Sand, grass, dirt, rock, snow. Distances are in meters.
const MATERIAL_PERIODS = array(96.0, 128.0, 112.0, 160.0, 96.0);
// Gravelly sand, rocky terrain, lichen rock, Ice003.
const DETAIL_PERIODS = array(3.6, 5.4, 3.8, 4.6);

fn noise_hash(cell: vec3<f32>) -> f32 {
    var h = fract(cell * vec3<f32>(0.1031, 0.1030, 0.0973));
    h += dot(h, h.yxz + 33.33);
    return fract((h.x + h.y) * h.z);
}

fn macro_noise(position: vec3<f32>) -> f32 {
    let cell = floor(position);
    let t = fract(position);
    let w = t * t * (3.0 - 2.0 * t);
    let near = mix(
        mix(noise_hash(cell), noise_hash(cell + vec3<f32>(1.0, 0.0, 0.0)), w.x),
        mix(noise_hash(cell + vec3<f32>(0.0, 1.0, 0.0)), noise_hash(cell + vec3<f32>(1.0, 1.0, 0.0)), w.x),
        w.y,
    );
    let far_cell = cell + vec3<f32>(0.0, 0.0, 1.0);
    let far = mix(
        mix(noise_hash(far_cell), noise_hash(far_cell + vec3<f32>(1.0, 0.0, 0.0)), w.x),
        mix(noise_hash(far_cell + vec3<f32>(0.0, 1.0, 0.0)), noise_hash(far_cell + vec3<f32>(1.0, 1.0, 0.0)), w.x),
        w.y,
    );
    return mix(near, far, w.z) - 0.5;
}

fn filtered_noise(position: vec3<f32>, strength: f32) -> f32 {
    if strength > 0.0 {
        return macro_noise(position) * strength;
    }
    return 0.0;
}

// A biome is climate, not simply a height band. Direction is in the planet's
// local frame, so climate stays fixed when the high-precision origin shifts.
struct BiomeSurface {
    weights: array<f32, 5>,
    vegetation_tint: vec3<f32>,
}

fn biome_surface(
    height: f32,
    slope: f32,
    direction: vec3<f32>,
    variation: f32,
) -> BiomeSurface {
    // Y is the planet's north/south axis. Separate broad moisture systems from
    // shorter weather-scale patches; neither depends on terrain tile boundaries.
    let latitude = abs(direction.y);
    let weather = macro_noise(direction * 5.8 + vec3<f32>(17.3, -4.7, 11.8));
    let rainfall_patches = macro_noise(direction * 17.0 + vec3<f32>(-2.1, 9.4, 4.6));
    // Match the CPU select_species() climate exactly. The local terrain
    // variation is only a visual tint, not a different biome map.
    let moisture = clamp(
        0.50 + 0.96 * weather + 0.32 * rainfall_patches,
        0.0, 1.0,
    );
    let temperature = clamp(
        1.07 - 0.78 * latitude - max(height, 0.0) / 16000.0
            + 0.13 * macro_noise(direction * 4.1 + vec3<f32>(3.2, 18.6, -8.4)),
        0.0, 1.0,
    );

    // The old 0.016–0.045 slope interval made almost every slope bare rock.
    // 0.48–0.88 on sin(inclination) restricts rock to genuinely steep cliffs.
    let cliff = smoothstep(0.48, 0.88, slope);
    let mountain_snow = smoothstep(
        2850.0, 4750.0,
        height + (0.52 - temperature) * 1400.0 + variation * 400.0,
    );
    let polar_snow = smoothstep(0.93, 0.994, latitude);
    let snow = clamp(max(mountain_snow, polar_snow) * (1.0 - 0.20 * cliff), 0.0, 1.0);
    let rock = (1.0 - snow) * cliff;
    let soil_available = (1.0 - snow) * (1.0 - cliff);

    // Sand on coastlines and in warm dry interiors, not on all low terrain.
    let beach = 1.0 - smoothstep(20.0, 170.0, height + variation * 80.0);
    let desert = smoothstep(0.65, 0.86, temperature)
        * (1.0 - smoothstep(0.19, 0.43, moisture));
    let sand = soil_available * max(beach, desert);
    let vegetated = max(soil_available - sand, 0.0);
    let exposed_soil = clamp(
        0.12 + 0.32 * (1.0 - moisture)
            + 0.14 * smoothstep(900.0, 3900.0, height) + 0.16 * variation,
        0.07, 0.65,
    );

    // Climate still tints the soil and grass beneath the separately sampled crowns.
    let rainforest = smoothstep(0.655, 0.705, temperature)
        * smoothstep(0.465, 0.525, moisture);
    let temperate_forest = smoothstep(0.265, 0.315, temperature)
        * (1.0 - smoothstep(0.655, 0.705, temperature))
        * smoothstep(0.385, 0.445, moisture);
    let savanna = smoothstep(0.635, 0.695, temperature)
        * smoothstep(0.165, 0.215, moisture)
        * (1.0 - smoothstep(0.58, 0.64, moisture));
    let steppe = (1.0 - smoothstep(0.22, 0.54, moisture))
        * (1.0 - savanna)
        * smoothstep(0.25, 0.51, temperature);
    let tundra = 1.0 - smoothstep(0.18, 0.40, temperature);

    // Tint only the grass/dirt layers. Rock remains mineral-grey and snow
    // remains white. Triplanar PBR albedo/normal/roughness stay in use.
    var foliage = vec3<f32>(0.91, 1.09, 0.83);
    foliage = mix(foliage, vec3<f32>(1.38, 1.12, 0.64), savanna);
    foliage = mix(foliage, vec3<f32>(1.23, 1.12, 0.75), steppe);
    foliage = mix(foliage, vec3<f32>(0.58, 0.92, 0.56), temperate_forest);
    foliage = mix(foliage, vec3<f32>(0.40, 0.85, 0.49), rainforest);
    foliage = mix(foliage, vec3<f32>(0.81, 0.90, 0.77), tundra);

    return BiomeSurface(
        array(
            sand,
            vegetated * (1.0 - exposed_soil),
            vegetated * exposed_soil,
            rock,
            snow,
        ),
        foliage,
    );
}

fn global_canopy(direction: vec3<f32>, footprint: f32, texel_meters: f32) -> vec4<f32> {
    if canopy_layout.fade.w == 0.0 {
        return vec4<f32>(0.0);
    }
    let lod = max(canopy_layout.fade.z, log2(max(footprint / texel_meters, 1.0)));
    let clearing = canopy_layout.clearing;
    var coverage = canopy_layout.fade.w;
    let resolved_texel = texel_meters * exp2(lod);
    if clearing.w > 0.2 * resolved_texel {
        let distance_to_clearing = distance(direction, clearing.xyz) * planet_radius;
        let edge = resolved_texel * 0.5;
        coverage *= smoothstep(clearing.w - edge, clearing.w + edge, distance_to_clearing);
    }
    return textureSampleLevel(globe_canopy, globe_sampler, direction, lod) * coverage;
}

fn forest_canopy(direction: vec3<f32>, footprint: f32) -> vec4<f32> {
    let axis = max(abs(direction.x), max(abs(direction.y), abs(direction.z)));
    // Account for the shrinking solid angle of a cube texel toward the face edges.
    let globe_texel = 2.0 * planet_radius * axis * axis / f32(textureDimensions(globe_canopy).x);
    let resident_texel = globe_texel * exp2(canopy_layout.fade.z);
    var result = vec4<f32>(0.0);
    var remaining = 1.0;
    for (var level = 0u; level < CANOPY_CASCADES; level++) {
        let layer = canopy_layout.cascades[level];
        if layer.center.w == 0.0 || footprint >= 2.7 * layer.north.w {
            continue;
        }
        let cosine = dot(layer.center.xyz, direction);
        if cosine <= 0.0 {
            continue;
        }
        let square = vec2<f32>(dot(layer.east.xyz, direction), dot(layer.north.xyz, direction))
            / (layer.east.w * cosine);
        let edge = max(abs(square.x), abs(square.y));
        let spatial = 1.0 - smoothstep(0.61, 0.995, edge);
        let footprint_blend = 1.0 - smoothstep(0.85, 2.7, footprint / layer.north.w);
        // Keep the coarsest local projections available as a fallback while the
        // global texture streams in. Afterwards prefer whichever resolves more terrain.
        let finer = 1.0 - canopy_layout.fade.w * smoothstep(1.0, 1.9, layer.north.w / resident_texel);
        let weight = spatial * footprint_blend * layer.center.w * finer;
        if weight > 0.0 {
            let uv = square * 0.5 + 0.5;
            let data = textureSampleLevel(canopy_atlas, canopy_sampler, uv, i32(level), 0.0);
            result += data * (remaining * weight);
            remaining *= 1.0 - weight;
            if remaining < 0.001 {
                return result;
            }
        }
    }
    return result + remaining * global_canopy(direction, footprint, globe_texel);
}

// Diagnostic only: visualize the actual cascade weights, not sampled color.
// This duplicates forest_canopy's weight selection without modifying its
// production sampling, lighting, bindings or texture data.
fn canopy_cascade_debug(direction: vec3<f32>, footprint: f32) -> vec3<f32> {
    let axis = max(abs(direction.x), max(abs(direction.y), abs(direction.z)));
    let globe_texel = 2.0 * planet_radius * axis * axis / f32(textureDimensions(globe_canopy).x);
    let resident_texel = globe_texel * exp2(canopy_layout.fade.z);
    let colors = array(
        vec3<f32>(0.10, 0.24, 1.00), // Cascade 0: blue
        vec3<f32>(0.02, 0.82, 0.97), // Cascade 1: cyan
        vec3<f32>(0.16, 0.90, 0.29), // Cascade 2: green
        vec3<f32>(0.98, 0.87, 0.08), // Cascade 3: yellow
        vec3<f32>(1.00, 0.45, 0.06), // Cascade 4: orange
        vec3<f32>(0.92, 0.13, 0.16), // Cascade 5: red
    );
    var debug = vec3<f32>(0.0);
    var remaining = 1.0;
    for (var level = 0u; level < CANOPY_CASCADES; level++) {
        let layer = canopy_layout.cascades[level];
        if layer.center.w == 0.0 || footprint >= 2.7 * layer.north.w {
            continue;
        }
        let cosine = dot(layer.center.xyz, direction);
        if cosine <= 0.0 {
            continue;
        }
        let square = vec2<f32>(dot(layer.east.xyz, direction), dot(layer.north.xyz, direction))
            / (layer.east.w * cosine);
        let edge = max(abs(square.x), abs(square.y));
        let spatial = 1.0 - smoothstep(0.61, 0.995, edge);
        let footprint_blend = 1.0 - smoothstep(0.85, 2.7, footprint / layer.north.w);
        let finer = 1.0 - canopy_layout.fade.w
            * smoothstep(1.0, 1.9, layer.north.w / resident_texel);
        let weight = spatial * footprint_blend * layer.center.w * finer;
        if weight > 0.0 {
            debug += remaining * weight * colors[level];
            remaining *= 1.0 - weight;
            if remaining < 0.001 {
                return debug;
            }
        }
    }
    // Global cubemap supplies the residual contribution after local cascades.
    return debug + remaining * canopy_layout.fade.w * vec3<f32>(0.78, 0.17, 0.87);
}

fn crown_shading(direction: vec3<f32>, dx: vec3<f32>, dy: vec3<f32>, sun: vec3<f32>) -> vec4<f32> {
    let layer = canopy_layout.cascades[0];
    let cosine = dot(layer.center.xyz, direction);
    if layer.center.w == 0.0 || cosine <= 0.0 {
        return vec4<f32>(1.0);
    }
    let square = vec2<f32>(dot(direction, layer.east.xyz), dot(direction, layer.north.xyz))
        / (layer.east.w * cosine);
    let edge = max(abs(square.x), abs(square.y));
    let side = f32(textureDimensions(crown_detail).x);
    let diameter = 2.0 * layer.east.w * planet_radius;
    let meters_per_texel = diameter / side;
    let pixel_dx = length(dx);
    let pixel_dy = length(dy);
    let footprint = max(sqrt(pixel_dx * pixel_dy), max(pixel_dx, pixel_dy) / 6.0);
    let strength = (1.0 - smoothstep(0.61, 0.995, edge))
        * (1.0 - smoothstep(1.2, 4.6, footprint / meters_per_texel)) * layer.center.w;
    if strength <= 0.0 {
        return vec4<f32>(1.0);
    }

    let uv = square * 0.5 + 0.5;
    // Retain horizontal crown detail at an oblique angle while prefiltering
    // the longer axis of the pixel through the premultiplied mip chain.
    let scale = 1.0 / (diameter * cosine);
    let chart_offset = square * layer.east.w;
    let uv_dx = (vec2<f32>(dot(dx, layer.east.xyz), dot(dx, layer.north.xyz))
        - chart_offset * dot(dx, layer.center.xyz)) * scale;
    let uv_dy = (vec2<f32>(dot(dy, layer.east.xyz), dot(dy, layer.north.xyz))
        - chart_offset * dot(dy, layer.center.xyz)) * scale;
    let crown = textureSampleGrad(crown_detail, crown_sampler, uv, uv_dx, uv_dy);
    let crown_amount = smoothstep(0.025, 0.76, crown.r);
    let gaps = mix(1.0, mix(0.26, 1.0, crown_amount), strength);
    if crown_amount <= 0.0 {
        return vec4<f32>(vec3<f32>(1.0), gaps);
    }

    let sun_plane = vec2<f32>(dot(sun, layer.east.xyz), dot(sun, layer.north.xyz));
    let sun_tangent = length(sun_plane);
    let step_texels = 1.45;
    let step_meters = step_texels * meters_per_texel;
    let offset = sun_plane * (step_texels / (max(sun_tangent, 0.001) * side));
    let sunward = textureSampleGrad(crown_detail, crown_sampler, uv + offset, uv_dx, uv_dy);
    let leeward = textureSampleGrad(crown_detail, crown_sampler, uv - offset, uv_dx, uv_dy);
    let height_scale = 40.0;
    let sun_height = dot(sun, direction);
    let direct = smoothstep(0.0, 0.25, sun_height);
    // The signed slope adds contrast around the stand's mean shading.
    // Filtering the height difference removes contrast at coarser footprints.
    let slope = (sunward.g - leeward.g) * height_scale / (2.0 * step_meters);
    let attributes = crown.gba / max(crown.r, 0.03);
    let side_light = clamp(slope * sun_tangent, -0.7, 0.7);
    let light = 1.0 - 0.48 * direct * side_light;
    let shade = light * mix(0.75, 1.25, attributes.y);
    let warmth = attributes.z - 0.5;
    let color = vec3<f32>(1.0 + 0.31 * warmth, 1.0 + 0.09 * warmth, 1.0 - 0.37 * warmth);
    let grove = mix(vec3<f32>(1.0), color * shade, crown_amount);
    return vec4<f32>(mix(vec3<f32>(1.0), grove, strength), gaps);
}

fn grove_shading(point: vec3<f32>, footprint: f32, sun: vec3<f32>) -> vec3<f32> {
    // These bands modulate the canonical crown color only. Feeding them into
    // canopy alpha would create trees in a clearing or cut holes in a stand.
    let crowns = filtered_noise(point / 52.0, 1.0 - smoothstep(14.0, 45.0, footprint));
    let grove_point = point / 235.0 + vec3<f32>(5.4, -1.2, 9.3);
    let grove_weight = 1.0 - smoothstep(64.0, 210.0, footprint);
    let groves = filtered_noise(grove_point, grove_weight);
    let slope = filtered_noise(grove_point + sun * 0.28, grove_weight) - groves;
    let stands = filtered_noise(point / 1260.0, 1.0 - smoothstep(380.0, 1160.0, footprint));
    let shade = 1.0 + 0.54 * crowns + 0.60 * groves - 0.26 * slope + 0.49 * stands;
    let warmth = 0.23 * groves + 0.31 * stands;
    return max(vec3<f32>(0.37), vec3<f32>(shade + warmth, shade, shade - warmth * 0.68));
}

struct Projection {
    uv: vec2<f32>,
    dx: vec2<f32>,
    dy: vec2<f32>,
    tangent: vec3<f32>,
    bitangent: vec3<f32>,
    weight: f32,
}

struct SurfaceSample {
    color: vec3<f32>,
    normal_offset: vec3<f32>,
    roughness: f32,
}

fn triplanar_projections(
    position: vec3<f32>, dx: vec3<f32>, dy: vec3<f32>, normal: vec3<f32>,
) -> array<Projection, 3> {
    let sign = select(vec3<f32>(-1.0), vec3<f32>(1.0), normal >= vec3<f32>(0.0));
    let squared = normal * normal;
    let axis_weights = squared * squared;
    let normalized = axis_weights / dot(axis_weights, vec3<f32>(1.0));
    // Fade negligible projections to zero before sampling. Renormalizing
    // keeps both the color and the normal blend continuous across slopes.
    let contributing = normalized * smoothstep(vec3<f32>(0.005), vec3<f32>(0.02), normalized);
    let weights = contributing / dot(contributing, vec3<f32>(1.0));

    // Flip the V axis on negative faces so each normal map has a right-handed basis.
    let x_uv = vec2<f32>(1.0, sign.x);
    let y_uv = vec2<f32>(1.0, sign.y);
    let z_uv = vec2<f32>(1.0, sign.z);
    return array(
        Projection(position.yz * x_uv, dx.yz * x_uv, dy.yz * x_uv,
            vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, sign.x), weights.x),
        Projection(position.zx * y_uv, dx.zx * y_uv, dy.zx * y_uv,
            vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(sign.y, 0.0, 0.0), weights.y),
        Projection(position.xy * z_uv, dx.xy * z_uv, dy.xy * z_uv,
            vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, sign.z, 0.0), weights.z),
    );
}

fn sample_layer(
    albedo: texture_2d_array<f32>, normal_roughness: texture_2d_array<f32>,
    layer: i32, scale: f32, projections: array<Projection, 3>,
    albedo_detail: f32, normal_detail: f32, normal_y_sign: f32,
) -> SurfaceSample {
    var color = vec3<f32>(0.0);
    var roughness = 0.0;
    var offset = vec3<f32>(0.0);

    if albedo_detail > 0.0 {
        var axes = projections;
        for (var axis = 0; axis < 3; axis++) {
            let projection = axes[axis];
            if projection.weight > 0.0 {
                let uv = projection.uv * scale;
                let dx = projection.dx * scale;
                let dy = projection.dy * scale;
                color += textureSampleGrad(albedo, terrain_sampler, uv, layer, dx, dy).rgb
                    * projection.weight;
                if normal_detail > 0.0 {
                    let mapped = textureSampleGrad(normal_roughness, terrain_sampler, uv, layer, dx, dy);
                    let normal = mapped.xyz * 2.0 - 1.0;
                    // Flat detail normals preserve the height-derived normal.
                    offset += (projection.tangent * normal.x + projection.bitangent * (normal.y * normal_y_sign))
                        * (projection.weight / max(normal.z, 0.25));
                    roughness += mapped.a * projection.weight;
                }
            }
        }
    }
    // The final mip of each array is a 1×1 average for every material.
    if albedo_detail < 1.0 {
        let mip = i32(textureNumLevels(albedo)) - 1;
        let average = textureLoad(albedo, vec2<i32>(0), layer, mip).rgb;
        color = mix(average, color, albedo_detail);
    }
    if normal_detail < 1.0 {
        let mip = i32(textureNumLevels(normal_roughness)) - 1;
        let average = textureLoad(normal_roughness, vec2<i32>(0), layer, mip).a;
        roughness = mix(average, roughness, normal_detail);
    }

    return SurfaceSample(color, offset * normal_detail, roughness);
}

fn surface_detail(
    base: SurfaceSample, biome: array<f32, 5>, projections: array<Projection, 3>,
    position: vec3<f32>, slope: f32, strength: f32,
) -> SurfaceSample {
    let variation = macro_noise(position / 19.0 + vec3<f32>(3.2, -8.4, 12.7));
    let outcrop = smoothstep(0.13, 0.49, slope + 0.28 * variation);
    let meadow = smoothstep(-0.16, 0.24, variation + 0.30 * slope);
    let glaze = smoothstep(0.02, 0.28, variation + 0.31 * slope);
    let soil = biome[2];
    var coverage = array(
        biome[0] + soil * 0.29 * (1.0 - outcrop),
        (biome[1] * 0.36 + soil * 0.68) * meadow * (1.0 - outcrop * 0.75),
        biome[3] * 0.83 + soil * 0.78 * outcrop,
        biome[4] * 0.66 * glaze,
    );
    var periods = DETAIL_PERIODS;
    var result = base;
    let last_mip = i32(textureNumLevels(detail_albedo)) - 1;
    for (var layer = 0; layer < 4; layer++) {
        let weight = coverage[layer] * smoothstep(0.012, 0.065, coverage[layer]) * strength;
        if weight > 0.0 {
            // The OpenGL maps point +Y up the image. Projection.bitangent
            // follows increasing texture V, which runs down the image.
            let finish = sample_layer(detail_albedo, detail_normal_roughness,
                layer, 1.0 / periods[layer], projections, 1.0, 1.0, -1.0);
            let average = textureLoad(detail_albedo, vec2<i32>(0), layer, last_mip).rgb;
            // Apply the photo's local contrast around the biome color so the
            // detail fade keeps the same ground palette at different distances.
            let contrast = clamp(finish.color / max(average, vec3<f32>(0.02)),
                vec3<f32>(0.42), vec3<f32>(1.85));
            var color = base.color * mix(vec3<f32>(1.0), contrast, 0.86);
            if layer == 3 {
                color *= vec3<f32>(0.88, 0.96, 1.0);
            }
            result.color += (color - base.color) * weight;
            result.normal_offset += (finish.normal_offset * 0.62 - base.normal_offset) * weight;
            result.roughness += (finish.roughness - base.roughness) * weight;
        }
    }
    return result;
}

@fragment
fn fragment(input: VertexOutput) -> FragmentOutput {
    let position = input.color.xyz;
    let height = input.color.w;
    let direction = normalize(position);
    let surface_normal = normalize(input.world_normal);
    let slope = length(cross(direction, surface_normal));
    let dx = dpdx(position);
    let dy = dpdy(position);
    let pixel_dx = length(dx);
    let pixel_dy = length(dy);
    let footprint = max(pixel_dx, pixel_dy);
    // Fade normal detail first to reduce shimmer at grazing angles. Keep
    // albedo detail while it still spans pixels along either screen axis.
    let normal_detail = 1.0 - smoothstep(1.5, 9.0, footprint);
    let albedo_detail = 1.0 - smoothstep(28.0, 90.0, min(pixel_dx, pixel_dy));
    let ground_detail = 1.0 - smoothstep(0.07, 0.42, footprint);
    let ground_cover = 1.0 - smoothstep(0.95, 3.4, footprint);

    let regional = macro_noise(position / 85000.0);
    let patches = filtered_noise(position / 2300.0, 1.0 - smoothstep(500.0, 2300.0, footprint));
    let detail = filtered_noise(position / 190.0, 1.0 - smoothstep(45.0, 190.0, footprint));
    let variation = regional * 0.6 + patches * 0.4;
    let tint = 1.0 + 0.32 * regional + 0.32 * patches + 0.26 * detail;

    let biome = biome_surface(height, slope, direction, variation);
    // F9 diagnostics are composited after terrain PBR lighting, and
    // never replace the actual grass/soil/rock directly under the camera.
    var weights = biome.weights;
    if ground_cover > 0.0 && weights[1] > 0.0 {
        // Soil shows between stands of grass. The plant streamer uses this
        // same field to concentrate clumps in the greener areas.
        let cover = macro_noise(position * 0.21 + vec3<f32>(4.1, 11.8, -7.3));
        let soil = weights[1] * ground_cover * 0.64 * (1.0 - smoothstep(-0.19, 0.12, cover));
        weights[1] -= soil;
        weights[2] += soil;
    }
    var projections: array<Projection, 3>;
    if albedo_detail > 0.0 {
        projections = triplanar_projections(position, dx, dy, surface_normal);
    }
    var periods = MATERIAL_PERIODS;

    var surface = SurfaceSample(vec3<f32>(0.0), vec3<f32>(0.0), 0.0);
    var dominant_layer = 0;
    for (var layer = 0; layer < 5; layer++) {
        let weight = weights[layer];
        if weight > weights[dominant_layer] {
            dominant_layer = layer;
        }
        if weight > 0.0 {
            let sample = sample_layer(terrain_albedo, terrain_normal_roughness,
                layer, 1.0 / periods[layer], projections, albedo_detail, normal_detail, 1.0);
            var albedo = sample.color;
            if layer == 1 || layer == 2 {
                albedo *= biome.vegetation_tint;
            }
            surface.color += albedo * weight;
            surface.normal_offset += sample.normal_offset * weight;
            surface.roughness += sample.roughness * weight;
        }
    }

    var occlusion = 1.0;
    let forest_distance = distance(input.world_position.xyz, view.world_position.xyz);
    var canopy = vec4<f32>(0.0);
    var canopy_coverage = 0.0;
    var handoff = 0.0;
    var grazing = 0.0;
    if canopy_layout.fade.y > 0.0 && (forest_distance > canopy_layout.fade.x || terrain_debug_mode != 0u) {
        // Limit anisotropy near the horizon so distant stands remain stable in motion.
        canopy = forest_canopy(direction, max(sqrt(pixel_dx * pixel_dy), footprint * 0.35));
        handoff = smoothstep(canopy_layout.fade.x, canopy_layout.fade.y, forest_distance);
        let soil = 1.0 - clamp(weights[3] + weights[4], 0.0, 1.0);
        let exposed_land = smoothstep(1.1, 3.5, height);
        let toward_eye = (view.world_position.xyz - input.world_position.xyz) / max(forest_distance, 1.0);
        // Near-vertical views reveal clearings. Oblique rays cross more crowns.
        grazing = 1.0 - saturate(abs(dot(direction, toward_eye)));
        let crown_depth = mix(2.0, 12.0, grazing * grazing);
        let crown_influence = 1.0 - pow(1.0 - canopy.a, crown_depth);
        canopy_coverage = soil * exposed_land * crown_influence;
    }

    if ground_detail > 0.0 {
        // A second, tighter sample gives dirt, stones and grass their own
        // visible grain at walking distance. The broad texture supplies color.
        var dominant = projections[0];
        if projections[1].weight > dominant.weight {
            dominant = projections[1];
        }
        if projections[2].weight > dominant.weight {
            dominant = projections[2];
        }
        let scale = 0.38;
        let uv = (dominant.uv * scale) + vec2<f32>(0.17, 0.36);
        let close_dx = dominant.dx * scale;
        let close_dy = dominant.dy * scale;
        let grain = textureSampleGrad(terrain_albedo, terrain_sampler, uv, dominant_layer, close_dx, close_dy).rgb;
        let finish = textureSampleGrad(terrain_normal_roughness, terrain_sampler, uv, dominant_layer, close_dx, close_dy);
        let mip = i32(textureNumLevels(terrain_albedo)) - 1;
        let average = textureLoad(terrain_albedo, vec2<i32>(0), dominant_layer, mip).rgb;
        let grain_tint = clamp(grain / max(average, vec3<f32>(0.03)), vec3<f32>(0.48), vec3<f32>(1.65));
        let strength = ground_detail * smoothstep(0.36, 0.69, dominant.weight);
        surface.color *= mix(vec3<f32>(1.0), grain_tint, strength * 0.43);
        surface.roughness = mix(surface.roughness, finish.a, strength * 0.66);
        let bump = finish.xyz * 2.0 - 1.0;
        surface.normal_offset += (dominant.tangent * bump.x + dominant.bitangent * bump.y)
            * (strength * 0.26 / max(bump.z, 0.35));
        occlusion = mix(1.0, 0.79, strength * (1.0 - smoothstep(0.5, 1.02, dot(grain_tint, vec3<f32>(0.21, 0.72, 0.07)))));
    }

    let fine_detail = 1.0 - smoothstep(0.15, 0.95, footprint);
    if detail_enabled != 0u && fine_detail > 0.0 {
        surface = surface_detail(surface, weights, projections, position, slope, fine_detail);
    }

    var shaded_normal = surface_normal;
    if normal_detail > 0.0 {
        let tangent_offset = (surface.normal_offset - surface_normal * dot(surface_normal, surface.normal_offset)) * 0.70;
        shaded_normal = normalize(surface_normal + tangent_offset);
    }
    let base_color = vec4<f32>(surface.color * tint, 1.0);

    var pbr_input: PbrInput = pbr_input_new();
    pbr_input.material.flags |= STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    pbr_input.material.base_color = base_color;
    pbr_input.material.perceptual_roughness = surface.roughness;
    pbr_input.diffuse_occlusion = vec3<f32>(occlusion);
    pbr_input.frag_coord = input.position;
    pbr_input.world_position = input.world_position;
    pbr_input.world_normal = surface_normal;
    pbr_input.N = shaded_normal;
    pbr_input.V = calculate_view(input.world_position, pbr_input.is_orthographic);
    if handoff > 0.0 && canopy_coverage > 0.0 {
        // The atlas stores premultiplied linear foliage color. Use the same
        // sun, sky and exposure as the 3D forest by blending before lighting.
        let canopy_color = canopy.rgb / max(canopy.a, 0.001) * 0.3;
        var sun = vec3<f32>(-0.57735, 0.57735, -0.57735);
        if lights.n_directional_lights > 0u {
            sun = lights.directional_lights[0].direction_to_light;
        }
        let groves = grove_shading(direction * planet_radius, footprint, sun);
        let crowns = crown_shading(direction, dx, dy, sun);
        let stand_cover = canopy_coverage * mix(crowns.a, 1.0, grazing);
        // Geometry has first claim on the pixel. Apply the canopy's share
        // to the remaining terrain so ground does not leak through both fades.
        let geometry_cover = (1.0 - handoff) * stand_cover;
        let cover = handoff * stand_cover / max(1.0 - geometry_cover, 0.001);
        // The coarse normal represents many crown faces. Retain their mean
        // self-shadowing at every resolution, including the orbital cubemap.
        let forest_color = canopy_color * groves * crowns.rgb * 0.24;
        let forest_normal = normalize(mix(direction, surface_normal, 0.5));
        pbr_input.material.base_color = vec4<f32>(mix(base_color.rgb, forest_color, cover), 1.0);
        pbr_input.material.perceptual_roughness = mix(surface.roughness, 0.85, cover);
        pbr_input.material.reflectance = mix(pbr_input.material.reflectance, vec3<f32>(0.16), cover);
        pbr_input.N = normalize(mix(shaded_normal, forest_normal, cover));
    }
    let lit = apply_pbr_lighting(pbr_input);
    var final_color = main_pass_post_lighting_processing(pbr_input, lit);

    if terrain_debug_mode != 0u {
        // Preserve detailed terrain within 650 m even while inspecting
        // the planet-wide map. Fade the diagnostic in gradually rather
        // than painting the nearby soil a flat green or gray.
        var debug_color = canopy.rgb;
        if terrain_debug_mode == 1u {
            debug_color = vec3<f32>(canopy.a);
        } else if terrain_debug_mode == 3u {
            debug_color = canopy_cascade_debug(
                direction, max(sqrt(pixel_dx * pixel_dy), footprint * 0.35));
        }
        let diagnostic_blend = smoothstep(650.0, 1450.0, forest_distance);
        final_color = mix(final_color, vec4<f32>(debug_color, 1.0), diagnostic_blend);
    }
    return FragmentOutput(final_color);
}

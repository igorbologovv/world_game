#import bevy_terrain::bindings::config
#import bevy_terrain::attachments::{sample_height, sample_normal}
#import bevy_terrain::fragment::{FragmentInput, FragmentOutput, fragment_info, fragment_debug}
#import bevy_terrain::functions::{compute_local_position, lookup_tile}
#import bevy_pbr::pbr_types::{PbrInput, pbr_input_new}
#import bevy_pbr::pbr_functions::{calculate_view, apply_pbr_lighting}

@group(3) @binding(0)
var terrain_albedo: texture_2d_array<f32>;
@group(3) @binding(1)
var terrain_sampler: sampler;
@group(3) @binding(2)
var terrain_normal_roughness: texture_2d_array<f32>;

// Sand, grass, dirt, rock, snow. Distances are in meters.
const MATERIAL_PERIODS = array(96.0, 128.0, 112.0, 160.0, 96.0);

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

// A biome is climate, not simply a height band. Direction is in the planet's
// local frame, so climate stays fixed when the high-precision origin shifts.
struct BiomeSurface {
    weights: array<f32, 5>,
    vegetation_tint: vec3<f32>,
    forest_cover: f32,
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
    let moisture = clamp(
        0.50 + 0.96 * weather + 0.32 * rainfall_patches + 0.13 * variation,
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

    // Smooth, overlapping climate zones: tropical rainforest, temperate
    // forest, savanna, steppe, temperate grassland and polar tundra.
    let rainforest = smoothstep(0.68, 0.88, temperature)
        * smoothstep(0.47, 0.76, moisture);
    let temperate_forest = smoothstep(0.29, 0.49, temperature)
        * (1.0 - smoothstep(0.70, 0.90, temperature))
        * smoothstep(0.44, 0.73, moisture);
    let forest = clamp(max(rainforest, temperate_forest), 0.0, 1.0);
    let savanna = smoothstep(0.68, 0.86, temperature)
        * smoothstep(0.18, 0.37, moisture)
        * (1.0 - smoothstep(0.49, 0.70, moisture));
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
        forest,
    );
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
    let weights = axis_weights / dot(axis_weights, vec3<f32>(1.0));

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

fn sample_projection(projection: Projection, scale: f32, layer: i32) -> SurfaceSample {
    let uv = projection.uv * scale;
    let dx = projection.dx * scale;
    let dy = projection.dy * scale;
    let color = textureSampleGrad(terrain_albedo, terrain_sampler, uv, layer, dx, dy).rgb;
    let normal_roughness = textureSampleGrad(terrain_normal_roughness, terrain_sampler, uv, layer, dx, dy);
    let normal = normal_roughness.xyz * 2.0 - 1.0;

    // Blend offsets so a flat detail normal preserves the height-derived normal on every axis.
    let offset = (projection.tangent * normal.x + projection.bitangent * normal.y) / max(normal.z, 0.25);
    return SurfaceSample(color, offset, normal_roughness.a);
}

@fragment
fn fragment(input: FragmentInput) -> FragmentOutput {
    var info = fragment_info(input);
    let tile = lookup_tile(info.coordinate, info.blend, 0u);
    var height = sample_height(tile);
    var surface_normal = sample_normal(tile, info.world_normal);
    if info.blend.ratio > 0.0 {
        let tile2 = lookup_tile(info.coordinate, info.blend, 1u);
        height = mix(height, sample_height(tile2), info.blend.ratio);
        surface_normal = normalize(mix(surface_normal, sample_normal(tile2, info.world_normal), info.blend.ratio));
    }

    let up = normalize(info.world_normal);
    let slope = length(cross(up, surface_normal));

    // Use planet-local coordinates so rebasing the floating origin leaves the textures anchored.
    let position = compute_local_position(info.coordinate) * (config.scale + height);
    let dx = dpdx(position);
    let dy = dpdy(position);
    let footprint = max(length(dx), length(dy));

    let regional = macro_noise(position / 85000.0);
    let patches = macro_noise(position / 2300.0) * (1.0 - smoothstep(500.0, 2300.0, footprint));
    let detail = macro_noise(position / 190.0) * (1.0 - smoothstep(45.0, 190.0, footprint));
    let variation = regional * 0.6 + patches * 0.4;
    let tint = 1.0 + 0.32 * regional + 0.32 * patches + 0.26 * detail;

    let biome = biome_surface(height, slope, normalize(position), variation);
    var weights = biome.weights;
    var projections = triplanar_projections(position, dx, dy, surface_normal);
    // Subtle canopy-scale color breakup in wooded biomes. This is surface
    // shading, not tree geometry; actual forests still need vegetation meshes.
    let canopy_patches = smoothstep(-0.18, 0.24, macro_noise(position / 320.0))
        * (1.0 - smoothstep(180.0, 750.0, footprint));
    // Local copies allow runtime array indexing on the terrain pipeline's WGSL compiler.
    var periods = MATERIAL_PERIODS;

    var surface = SurfaceSample(vec3<f32>(0.0), vec3<f32>(0.0), 0.0);
    for (var layer = 0; layer < 5; layer++) {
        let weight = weights[layer];
        if weight > 0.0 {
            for (var axis = 0; axis < 3; axis++) {
                let projection = projections[axis];
                let projected_weight = weight * projection.weight;
                let sample = sample_projection(projection, 1.0 / periods[layer], layer);
                var albedo = sample.color;
                if layer == 1 || layer == 2 {
                    let canopy_darkening = 1.0 - 0.16 * biome.forest_cover * canopy_patches;
                    albedo *= biome.vegetation_tint * canopy_darkening;
                }
                surface.color += albedo * projected_weight;
                surface.normal_offset += sample.normal_offset * projected_weight;
                surface.roughness += sample.roughness * projected_weight;
            }
        }
    }

    let tangent_offset = surface.normal_offset - surface_normal * dot(surface_normal, surface.normal_offset);
    let shaded_normal = normalize(surface_normal + tangent_offset);
    let base_color = vec4<f32>(surface.color * tint, 1.0);

    var output: FragmentOutput;
#ifdef LIGHTING
    var pbr_input: PbrInput = pbr_input_new();
    pbr_input.material.base_color = base_color;
    pbr_input.material.perceptual_roughness = surface.roughness;
    pbr_input.frag_coord = info.clip_position;
    pbr_input.world_position = info.world_position;
    pbr_input.world_normal = surface_normal;
    pbr_input.N = shaded_normal;
    pbr_input.V = calculate_view(info.world_position, pbr_input.is_orthographic);
    output.color = apply_pbr_lighting(pbr_input);
#else
    output.color = base_color;
#endif

    fragment_debug(&info, &output, tile, shaded_normal);
    return output;
}

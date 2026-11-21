import bevy_pbr::render::forward_io::{VertexOutput, FragmentOutput};
import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT;
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
    forest_cover: f32,
    canopy_tint: vec3<f32>,
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
    let tree_moisture = 0.50 + 0.96 * weather + 0.32 * rainfall_patches;
    let moisture = clamp(tree_moisture + 0.13 * variation, 0.0, 1.0);
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
    // Feather the tree placement climate thresholds for aggregate canopy.
    // Local ground variation still affects the soil and grass mixture.
    let rainforest = smoothstep(0.65, 0.72, temperature)
        * smoothstep(0.45, 0.54, tree_moisture);
    let temperate_forest = smoothstep(0.26, 0.33, temperature)
        * (1.0 - smoothstep(0.64, 0.72, temperature))
        * smoothstep(0.38, 0.45, tree_moisture);
    let boreal_forest = smoothstep(0.12, 0.16, temperature)
        * (1.0 - smoothstep(0.43, 0.49, temperature))
        * smoothstep(0.25, 0.34, tree_moisture);
    let savanna = smoothstep(0.63, 0.72, temperature)
        * smoothstep(0.17, 0.24, tree_moisture)
        * (1.0 - smoothstep(0.55, 0.65, tree_moisture));
    let steppe = (1.0 - smoothstep(0.22, 0.54, moisture))
        * (1.0 - savanna)
        * smoothstep(0.25, 0.51, temperature);
    let tundra = 1.0 - smoothstep(0.18, 0.40, temperature);
    // Use the climatic envelope of *all* supported trees, not just the
    // rain/temperate/boreal sub-biomes. Acacia, pine and warm coastal trees
    // can grow where the old narrow forest mask was zero, leaving mountains
    // behind visible stands inexplicably pale.
    //
    // Habitat describes the whole landscape; sparse individual placements,
    // local grove noise, streaming range and slope do not define its color.
    let forest = max(max(rainforest, temperate_forest), max(boreal_forest, savanna * 0.48));
    let tree_habitat = smoothstep(0.12, 0.23, temperature)
        * smoothstep(0.16, 0.33, tree_moisture)
        * (1.0 - smoothstep(0.91, 0.96, latitude));
    let alpine = 1.0 - smoothstep(2400.0, 2900.0, height);
    let forest_landcover = max(forest, tree_habitat * 0.92) * alpine
        * (1.0 - snow);
    var canopy = mix(vec3<f32>(0.063, 0.119, 0.035), vec3<f32>(0.039, 0.095, 0.043), boreal_forest);
    canopy = mix(canopy, vec3<f32>(0.033, 0.112, 0.036), rainforest);
    canopy = mix(canopy, vec3<f32>(0.104, 0.143, 0.049), savanna);

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
        forest_landcover,
        canopy,
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
    // Use camera-relative coordinates for gradients. Absolute Mars
    // positions lose fractions of a meter and destabilize close mip levels.
    let dx = dpdx(input.world_position.xyz);
    let dy = dpdy(input.world_position.xyz);
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

    // Distant forest is a regional landcover surface, not a scatter of
    // individual trees. The mesh's UV0.x carries the independent physical
    // canopy region from world-core. It does not vanish as trees are culled.
    // Keep material rock/snow exposed on genuinely steep or snowy terrain,
    // but never mistake a wooded hillside for bare ground just because its
    // local grass/dirt weight is small.
    let distance_to_eye = distance(input.world_position.xyz, view.world_position.xyz);
    let canopy_region = smoothstep(0.06, 0.45, input.uv.x);
    let exposed = clamp(weights[3] + weights[4], 0.0, 1.0);
    // Actual palms and jungle trees grow on low coastal ground: excluding
    // all land below 22 m painted a huge pale hole around Palm Coast.
    // Reserve only the immediate waterline for unpainted sand.
    let shoreline = 1.0 - smoothstep(1.5, 4.0, height);
    let forest_ground = canopy_region * (1.0 - exposed) * (1.0 - shoreline);
    let canopy_distance = smoothstep(180.0, 850.0, distance_to_eye);
    let broad_variation = clamp(
        1.0 + 0.09 * regional + 0.035 * patches,
        0.90, 1.10,
    );

    // The near-ground material is darker under a wooded canopy, but does
    // not lose its physical detail textures and normals.
    let forest_floor_color = vec3<f32>(0.055, 0.142, 0.051) * broad_variation;
    let ground_blend = forest_ground * (1.0 - canopy_distance) * 0.70;
    surface.color = mix(surface.color, forest_floor_color, ground_blend);

    // A canopy is a *surface of leaves*, not bare forest soil. Beyond the
    // detailed trees it must be dark and continuous, including spots where
    // sparse tree candidates leave holes and beyond forest impostor range.
    // Slight low-frequency variation changes lightness, not vegetation type.
    // Fixed leaf palette is intentionally independent of biome.forest_cover:
    // the previous climate tint yielded washed-out pale hills even where
    // the streamed tree species clearly formed a dense dark-green stand.
    let canopy_color = vec3<f32>(0.036, 0.125, 0.046) * broad_variation;
    let canopy_blend = forest_ground * canopy_distance * 0.995;
    surface.color = mix(surface.color, canopy_color, canopy_blend);
    surface.roughness = mix(surface.roughness, 0.93, canopy_blend);

    // Keep the climate's actual non-forest terrain exposed: dry grasslands,
    // bare soil and meadows must not inherit the forest canopy hue.
    let meadow = smoothstep(-0.19, 0.18, patches + 0.30 * regional);
    let meadow_color = mix(
        vec3<f32>(0.87, 0.84, 0.78),
        vec3<f32>(1.09, 1.05, 0.95),
        meadow,
    );
    let meadow_strength = clamp(weights[1] + weights[2], 0.0, 1.0)
        * (1.0 - canopy_region) * 0.18;
    surface.color *= mix(vec3<f32>(1.0), meadow_color, meadow_strength);

    // Preserve rock, snow and sand, already excluded from vegetation by
    // material weights. Steep outcrops stay mineral even in forest regions.
    let outcrop = smoothstep(0.28, 0.64, slope + patches * 0.10)
        * smoothstep(140.0, 780.0, height)
        * (1.0 - weights[4]);
    let rock_average = textureLoad(
        terrain_albedo, vec2<i32>(0), 3,
        i32(textureNumLevels(terrain_albedo)) - 1,
    ).rgb;
    surface.color = mix(surface.color, rock_average, outcrop * 0.30);

    var shaded_normal = surface_normal;
    if normal_detail > 0.0 {
        let tangent_offset = (surface.normal_offset - surface_normal * dot(surface_normal, surface.normal_offset)) * 0.70;
        shaded_normal = normalize(surface_normal + tangent_offset);
    }
    // Avoid bright biome tint washing the distant leaf color back to pale
    // grass. The canopy hue is set above; sunlight and atmosphere still
    // shape the final color.
    let biome_tint = mix(tint, 1.0, canopy_blend);
    let base_color = vec4<f32>(surface.color * biome_tint, 1.0);

    var pbr_input: PbrInput = pbr_input_new();
    pbr_input.material.flags |= STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    pbr_input.flags |= MESH_FLAGS_SHADOW_RECEIVER_BIT;
    pbr_input.material.base_color = base_color;
    pbr_input.material.perceptual_roughness = surface.roughness;
    pbr_input.diffuse_occlusion = vec3<f32>(occlusion);
    pbr_input.frag_coord = input.position;
    pbr_input.world_position = input.world_position;
    pbr_input.world_normal = surface_normal;
    pbr_input.N = shaded_normal;
    pbr_input.V = calculate_view(input.world_position, pbr_input.is_orthographic);
    let lit = apply_pbr_lighting(pbr_input);
    return FragmentOutput(main_pass_post_lighting_processing(pbr_input, lit));
}

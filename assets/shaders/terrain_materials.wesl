import super::terrain_coordinates::{noise_cell, macro_noise, Projection, SurfaceSample, triplanar_projections, terrain_sampler};

@group(constants::MATERIAL_BIND_GROUP) @binding(15)
var biome_albedo_height: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(16)
var biome_normal_roughness: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(17)
var<uniform> biome_enabled: u32;

struct BiomeGround {
    surface: SurfaceSample,
    coverage: f32,
    occlusion: f32,
}

struct GroundTexture {
    surface: SurfaceSample,
    height: f32,
}

struct Candidate {
    layer: i32,
    score: f32,
}

// Reconstruct the lattice cell and in-cell fraction independently. Adding the
// local position to the planet origin first would round away sub-meter detail.
fn material_noise(origin: vec3<f32>, position: vec3<f32>, period: f32, seed: vec3<f32>) -> f32 {
    let local = position / period;
    return noise_cell(origin * (128.0 / period) + floor(local) + seed, fract(local));
}

fn rank(candidates: ptr<function, array<Candidate, 4>>, candidate: Candidate) {
    if candidate.score > (*candidates)[0].score {
        (*candidates)[3] = (*candidates)[2];
        (*candidates)[2] = (*candidates)[1];
        (*candidates)[1] = (*candidates)[0];
        (*candidates)[0] = candidate;
    } else if candidate.score > (*candidates)[1].score {
        (*candidates)[3] = (*candidates)[2];
        (*candidates)[2] = (*candidates)[1];
        (*candidates)[1] = candidate;
    } else if candidate.score > (*candidates)[2].score {
        (*candidates)[3] = (*candidates)[2];
        (*candidates)[2] = candidate;
    } else if candidate.score > (*candidates)[3].score {
        (*candidates)[3] = candidate;
    }
}

fn habitat_scores(
    direction: vec3<f32>, elevation: f32, grade: f32, moisture: f32, temperature: f32,
    water: f32, patch_field: f32, pocket: f32, litter: f32, clearing: f32, radius: f32,
) -> array<f32, 17> {
    // The vegetation stream uses these climate bands, 260 m grove field, and
    // tree-line elevation. The material fields below vary the floor of a grove.
    let grove = clamp(0.82 + macro_noise(direction * (radius / 260.0)
        + vec3<f32>(7.4, -12.8, 3.7)) * 0.90, 0.25, 1.0);
    let tropical = smoothstep(0.655, 0.705, temperature) * smoothstep(0.465, 0.525, moisture);
    let temperate = smoothstep(0.265, 0.315, temperature)
        * (1.0 - smoothstep(0.655, 0.705, temperature)) * smoothstep(0.385, 0.445, moisture);
    let boreal = smoothstep(0.115, 0.155, temperature)
        * (1.0 - smoothstep(0.435, 0.485, temperature)) * smoothstep(0.245, 0.295, moisture);
    let savanna = smoothstep(0.635, 0.695, temperature) * smoothstep(0.165, 0.215, moisture)
        * (1.0 - smoothstep(0.58, 0.64, moisture));
    let forest = saturate(max(max(tropical * 0.98, temperate * 0.94), max(boreal * 0.87, savanna * 0.34))
        * grove * 1.30) * clearing * smoothstep(3.5, 11.0, elevation)
        * (1.0 - smoothstep(2200.0, 2600.0, elevation)) * (1.0 - smoothstep(0.37, 0.65, grade));
    let pine = clamp(max(boreal * 0.95, temperate * 0.29) - tropical * 0.50, 0.0, 1.0);
    let broadleaf = forest * (1.0 - 0.78 * pine);
    let coast = smoothstep(0.04, 0.85, water) * (1.0 - smoothstep(0.37, 0.66, grade));
    let fresh = smoothstep(0.025, 0.76, -water + pocket * 0.16);
    let submerged = smoothstep(1.02, 1.62, -water);
    let bank = fresh * (1.0 - smoothstep(0.25, 0.69, grade));
    let washed = max(submerged * 0.77,
        bank * smoothstep(0.36, 0.78, 0.52 + 0.60 * patch_field - 0.42 * pocket));
    let damp = saturate(moisture + 0.35 * fresh + 0.25 * patch_field + 0.12 * pocket);
    let lowland = (1.0 - smoothstep(120.0, 850.0, elevation))
        * (1.0 - smoothstep(0.08, 0.31, grade));
    let wetland = saturate(max(lowland * smoothstep(0.56, 0.87, damp), fresh * (0.83 - 0.65 * grade)))
        * (1.0 - 0.91 * coast);
    let arid = (1.0 - smoothstep(0.21, 0.49, damp)) * smoothstep(0.33, 0.72, temperature);
    let alpine = smoothstep(1400.0, 3000.0, elevation + (0.47 - temperature) * 1250.0 + patch_field * 290.0);
    let mineral = smoothstep(0.16, 0.74, grade + patch_field * 0.20 + alpine * 0.22);
    let low_cover = (1.0 - 0.81 * forest) * (1.0 - 0.80 * wetland) * (1.0 - 0.84 * coast);
    let organic = (1.0 - 0.82 * mineral) * (1.0 - 0.91 * coast) * (1.0 - 0.61 * submerged);
    let wet_leaf = smoothstep(0.36, 0.78, damp);
    let dry_cover = (1.0 - arid * 0.82) * (1.0 - alpine * 0.81);

    // Values compete locally instead of assigning each biome one texture.
    // Every family is modulated by a different combination of patch_field fields.
    return array(
        // Humus, dry and wet deciduous litter, needles.
        organic * (0.24 + forest * (0.26 + 0.68 * pocket - 0.45 * patch_field) + 0.16 * arid),
        organic * broadleaf * (0.54 + 0.24 * (1.0 - wet_leaf) + 0.88 * patch_field - 0.45 * litter),
        organic * broadleaf * (0.55 + 0.38 * wet_leaf - 0.44 * patch_field + 0.87 * litter),
        organic * forest * pine * (1.12 + 0.75 * patch_field - 0.32 * pocket),
        // Green/dry moss, meadow soil, exposed roots and debris.
        organic * (0.14 + 0.60 * forest + 0.43 * wetland) * smoothstep(0.23, 0.66, damp)
            * (1.0 + 1.33 * pocket + 0.68 * patch_field - 0.44 * litter),
        organic * (0.17 + 0.48 * forest + 0.16 * alpine) * (1.06 - 0.68 * damp)
            * (1.0 - 1.20 * pocket - 0.81 * litter),
        organic * low_cover * dry_cover * (0.77 + 0.29 * damp + 0.52 * patch_field - 0.41 * pocket),
        organic * forest * (0.40 + 0.23 * mineral + 0.72 * pocket - 1.17 * litter),
        // Saturated organic soil, mud, and exposed drying flats.
        organic * wetland * (0.88 + 0.90 * patch_field + 0.42 * litter) * (1.0 - 0.56 * washed),
        organic * wetland * (0.63 + 0.54 * fresh - 0.59 * patch_field - 0.92 * litter),
        organic * (arid * lowland + fresh * (1.0 - wet_leaf)) * (0.51 + 0.74 * pocket + 0.40 * litter),
        // Talus and sorted gravel, sand and clay-rich exposed soil.
        (0.13 + 0.57 * mineral + 0.20 * coast + 1.05 * washed) * (1.0 - 0.62 * forest)
            * (1.0 + 0.82 * patch_field + 0.33 * litter),
        (arid * 0.88 + coast * 0.72 + 0.55 * washed) * (1.0 - 0.80 * mineral)
            * (0.77 + 0.79 * pocket - 0.40 * patch_field),
        (arid * 0.66 + mineral * 0.24 + (1.0 - forest) * (1.0 - damp) * 0.31)
            * (1.0 - coast * 0.85) * (1.0 - 0.73 * pocket - 0.54 * litter),
        // Bedrock, cold stony ground, and shore sediment.
        mineral * (0.42 + 0.85 * mineral + 0.33 * alpine - 0.53 * patch_field),
        alpine * (1.08 - 0.52 * mineral + 0.55 * litter + 0.43 * pocket),
        coast * (1.10 - 0.59 * pocket + 0.53 * patch_field),
    );
}

fn blend_surface(a: SurfaceSample, b: SurfaceSample, weight: f32) -> SurfaceSample {
    return SurfaceSample(mix(a.color, b.color, weight),
        mix(a.normal_offset, b.normal_offset, weight), mix(a.roughness, b.roughness, weight));
}

fn sample_ground(
    layer: i32, scale: f32, axes: array<Projection, 3>, normal_strength: f32,
) -> GroundTexture {
    var color_height = vec4<f32>(0.0);
    var surface = SurfaceSample(vec3<f32>(0.0), vec3<f32>(0.0), 0.0);
    let turn = layer % 2 != 0;
    for (var i = 0; i < 3; i++) {
        let axis = axes[i];
        if axis.weight <= 0.0 {
            continue;
        }
        // Variant coordinates and their tangent-space normal use the same rotation.
        let uv = select(axis.uv, vec2<f32>(axis.uv.y, -axis.uv.x), turn) * scale;
        let uv_dx = select(axis.dx, vec2<f32>(axis.dx.y, -axis.dx.x), turn) * scale;
        let uv_dy = select(axis.dy, vec2<f32>(axis.dy.y, -axis.dy.x), turn) * scale;
        color_height += textureSampleGrad(biome_albedo_height, terrain_sampler, uv, layer, uv_dx, uv_dy) * axis.weight;
        let finish = textureSampleGrad(biome_normal_roughness, terrain_sampler, uv, layer, uv_dx, uv_dy);
        let mapped = finish.xyz * 2.0 - 1.0;
        let slope = select(vec2<f32>(mapped.x, -mapped.y), mapped.yx, turn);
        surface.normal_offset += (axis.tangent * slope.x + axis.bitangent * slope.y)
            * (axis.weight * normal_strength / max(mapped.z, 0.32));
        surface.roughness += finish.a * axis.weight;
    }
    surface.color = color_height.rgb;
    return GroundTexture(surface, color_height.a);
}

fn material_texture(
    layer: i32, fine_axes: array<Projection, 3>, medium_axes: array<Projection, 3>,
    medium: f32, normal_strength: f32,
) -> GroundTexture {
    // Tile periods 2 m and 8 m (4 m and 16 m on rock) divide the 128 m origin interval.
    let scale = select(0.5, 0.25, layer / 2 == 14);
    if medium >= 1.0 {
        return sample_ground(layer, scale * 0.25, medium_axes, normal_strength);
    }
    let fine = sample_ground(layer, scale, fine_axes, normal_strength);
    if medium <= 0.0 {
        return fine;
    }
    let wide = sample_ground(layer, scale * 0.25, medium_axes, normal_strength);
    return GroundTexture(blend_surface(fine.surface, wide.surface, medium), mix(fine.height, wide.height, medium));
}

fn biome_ground(
    position: vec3<f32>, origin: vec3<f32>, dx: vec3<f32>, dy: vec3<f32>,
    direction: vec3<f32>, normal: vec3<f32>, height: f32, grade: f32, water: f32,
    moisture: f32, temperature: f32, clearing: f32, radius: f32, distance: f32,
) -> BiomeGround {
    let projected = max(length(dx), length(dy));
    let patch_field = material_noise(origin, position, 32.0, vec3<f32>(7.0, 23.0, -16.0));
    let pocket_field = material_noise(origin, position, 8.0, vec3<f32>(19.0, -3.0, 11.0));
    let litter_field = material_noise(origin, position, 2.0, vec3<f32>(-4.0, 12.0, 5.0));
    let pocket = pocket_field * (1.0 - smoothstep(0.9, 4.0, projected));
    let litter = litter_field * (1.0 - smoothstep(0.25, 1.4, projected));
    let variety = material_noise(origin, position, 16.0, vec3<f32>(-13.0, 6.0, 28.0));
    let scores = habitat_scores(direction, height, grade, moisture, temperature, water,
        patch_field, pocket, litter, clearing, radius);

    var candidates = array(Candidate(0, -1.0), Candidate(0, -1.0), Candidate(0, -1.0), Candidate(0, -1.0));
    for (var family = 0; family < 17; family++) {
        // Fold the variant into the same continuous competition as the families.
        let variant = select(variety + patch_field * 0.32, -variety + pocket * 0.32, family % 3 == 0);
        let first = family * 2 + select(0, 1, variant > 0.0);
        let score = scores[family];
        rank(&candidates, Candidate(first, score));
        let alternate = score - abs(variant) * 2.7;
        if alternate > candidates[3].score {
            rank(&candidates, Candidate(family * 4 + 1 - first, alternate));
        }
    }
    // The fourth-highest score is a continuous cutoff. A surface entering
    // or leaving the three-sample budget has zero weight on that boundary.
    let cutoff = max(candidates[0].score - 0.31, candidates[3].score);
    let span = candidates[0].score - cutoff;
    let leading_scores = vec3<f32>(candidates[0].score, candidates[1].score, candidates[2].score);
    let relative = max((leading_scores - cutoff) / max(span, 0.000001) - 0.23, vec3<f32>(0.0));
    let weights = relative * relative;
    let coverage = smoothstep(0.0, 0.0004, span);

    let medium = max(smoothstep(17.0, 63.0, distance), smoothstep(0.075, 0.35, projected));
    let normal_strength = 0.75 * (1.0 - smoothstep(0.55, 3.0, projected));
    let projection_normal = normalize(mix(direction, normal, smoothstep(0.14, 0.62, grade)));
    var fine_axes: array<Projection, 3>;
    if medium < 1.0 {
        let warped = position + vec3<f32>(patch_field + pocket_field * 0.72,
            litter_field * 0.20 + pocket_field, variety - pocket_field * 0.68) * 2.8;
        fine_axes = triplanar_projections(warped, dx, dy, projection_normal);
    }
    var medium_axes: array<Projection, 3>;
    if medium > 0.0 {
        let warped = position + vec3<f32>(patch_field + variety * 0.48,
            variety * 1.3, patch_field - variety * 0.92) * 8.3;
        medium_axes = triplanar_projections(warped, dx, dy, projection_normal);
    }

    var color = vec3<f32>(0.0);
    var normal_offset = vec3<f32>(0.0);
    var roughness = 0.0;
    var relief = 0.0;
    var total = 0.0;
    for (var i = 0; i < 3; i++) {
        if weights[i] <= 0.0 {
            continue;
        }
        let material = material_texture(candidates[i].layer, fine_axes, medium_axes, medium, normal_strength);
        // Relief biases existing overlap without creating a new high-frequency edge.
        let weight = weights[i] * mix(0.68, 1.32, material.height);
        color += material.surface.color * weight;
        normal_offset += material.surface.normal_offset * weight;
        roughness += material.surface.roughness * weight;
        relief += material.height * weight;
        total += weight;
    }
    let normalization = 1.0 / max(total, 0.00000001);
    let soaked = smoothstep(0.36, 0.94, abs(water) + 0.10 * pocket + 0.08 * patch_field);
    let wet = soaked * (1.0 - smoothstep(0.46, 0.78, grade));
    let surface = SurfaceSample(color * normalization * mix(vec3<f32>(1.0),
        vec3<f32>(0.69, 0.74, 0.72), wet * 0.68),
        normal_offset * normalization * (1.0 - 0.22 * wet),
        mix(roughness * normalization, 0.44, wet * 0.66));
    let occlusion = mix(0.86, 1.0, smoothstep(0.05, 0.63, relief * normalization));
    return BiomeGround(surface, coverage, occlusion);
}

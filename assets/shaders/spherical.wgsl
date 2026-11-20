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

fn material_weights(height: f32, slope: f32, variation: f32) -> array<f32, 5> {
    // The height atlas is smoothed at planetary scale. Use its resolved grades for exposed rock.
    let exposed = smoothstep(0.016, 0.045, slope);
    let snow = smoothstep(3200.0, 5000.0, height - 1400.0 * exposed + 900.0 * variation)
        * (1.0 - 0.85 * exposed);
    let rock = (1.0 - snow) * exposed;
    let ground = (1.0 - snow) * (1.0 - exposed);
    let sand = ground * (1.0 - smoothstep(60.0, 250.0, height + 100.0 * variation));
    let dirt = clamp(0.27 + 0.32 * smoothstep(600.0, 3300.0, height)
        + 0.26 * smoothstep(0.004, 0.027, slope) + 0.85 * variation, 0.05, 0.95);
    let soil = ground - sand;
    return array(sand, soil * (1.0 - dirt), soil * dirt, rock, snow);
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

    var weights = material_weights(height, slope, variation);
    var projections = triplanar_projections(position, dx, dy, surface_normal);
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
                surface.color += sample.color * projected_weight;
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

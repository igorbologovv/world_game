import super::spherical::{canopy_layout, crown_detail, crown_sampler, planet_radius, forest_canopy, crown_shading, grove_shading};
import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings::{lights, view};
import bevy_pbr::render::pbr_types::{PbrInput, pbr_input_new, STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT};
import bevy_pbr::render::pbr_functions::{calculate_view, apply_pbr_lighting, main_pass_post_lighting_processing};

@group(constants::MATERIAL_BIND_GROUP) @binding(18)
var<uniform> far_fade: vec2<f32>;

struct CanopyOutput {
    @location(0) color: vec4<f32>,
    @builtin(sample_mask) mask: u32,
}

// Use Igor's ordered sample ranks so the two meshes cover complementary
// samples at the first handoff. Terrain fills the samples left uncovered.
fn canopy_mask(pixel: vec2<u32>, low: f32, high: f32) -> u32 {
    var rank = 0u;
    for (var i = 0u; i < 3u; i += 1u) {
        let x = (pixel.x >> i) & 1u;
        let y = (pixel.y >> i) & 1u;
        rank = rank * 4u + ((x ^ y) * 2u + y);
    }
    let offset = (f32(rank) + 0.5) / 64.0;
    let count = u32(constants::CANOPY_SAMPLES);
    var mask = 0u;
    for (var i = 0u; i < count; i += 1u) {
        let sample_rank = (f32(i) + offset) / f32(count);
        if sample_rank >= low && sample_rank < high {
            mask |= 1u << i;
        }
    }
    return mask;
}

@fragment
fn fragment(input: VertexOutput) -> CanopyOutput {
    let position = input.color.xyz;
    let direction = normalize(position);
    let dx = dpdx(position);
    let dy = dpdy(position);
    let world_dx = dpdx(input.world_position.xyz);
    let world_dy = dpdy(input.world_position.xyz);
    let uv_dx = dpdx(input.uv);
    let uv_dy = dpdy(input.uv);
    let eye_distance = distance(input.world_position.xyz, view.world_position.xyz);
    if eye_distance <= canopy_layout.fade.x || eye_distance >= far_fade.y {
        discard;
    }
    let crown = textureSampleGrad(crown_detail, crown_sampler, input.uv, uv_dx, uv_dy);
    let near = smoothstep(canopy_layout.fade.x, canopy_layout.fade.y, eye_distance);
    let far = 1.0 - smoothstep(far_fade.x, far_fade.y, eye_distance);
    let square = abs(input.uv * 2.0 - 1.0);
    let edge = 1.0 - smoothstep(0.82, 0.995, max(square.x, square.y));
    let coverage = crown.r * near * far * edge * canopy_layout.cascades[0].center.w;
    let mask = canopy_mask(vec2<u32>(input.position.xy), 1.0 - near, 1.0 - near + coverage);
    if mask == 0u {
        discard;
    }

    let pixel_dx = length(dx);
    let pixel_dy = length(dy);
    let footprint = max(pixel_dx, pixel_dy);
    let canopy = forest_canopy(direction, max(sqrt(pixel_dx * pixel_dy), footprint * 0.35));
    if canopy.a <= 0.001 {
        discard;
    }
    var sun = vec3<f32>(-0.57735, 0.57735, -0.57735);
    if lights.n_directional_lights > 0u {
        sun = lights.directional_lights[0].direction_to_light;
    }
    let groves = grove_shading(direction * planet_radius, footprint, sun);
    let crowns = crown_shading(direction, dx, dy, sun);
    let canopy_color = canopy.rgb / canopy.a * 0.3;
    // The terrain uses the crown color as one component of a soil/foliage
    // blend. Lift the opaque crown response where that blend exposes more
    // ground so emerging geometry does not darken the whole stand.
    let zenith_cover = 1.0 - (1.0 - canopy.a) * (1.0 - canopy.a);
    let forest_color = canopy_color * groves * crowns.rgb * mix(1.0, 0.65, zenith_cover);
    let geometric = normalize(cross(world_dx, world_dy));
    let outward = select(-geometric, geometric, dot(geometric, direction) > 0.0);
    let normal = normalize(mix(direction, outward, 0.5));

    var pbr_input: PbrInput = pbr_input_new();
    pbr_input.material.flags |= STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    pbr_input.material.base_color = vec4<f32>(forest_color, 1.0);
    pbr_input.material.perceptual_roughness = 0.85;
    pbr_input.material.reflectance = vec3<f32>(0.16);
    pbr_input.frag_coord = input.position;
    pbr_input.world_position = input.world_position;
    pbr_input.world_normal = normal;
    pbr_input.N = normal;
    pbr_input.V = calculate_view(input.world_position, pbr_input.is_orthographic);
    let lit = apply_pbr_lighting(pbr_input);
    return CanopyOutput(main_pass_post_lighting_processing(pbr_input, lit), mask);
}

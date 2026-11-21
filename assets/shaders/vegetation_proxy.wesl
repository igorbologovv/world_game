import super::vegetation_dither::{vegetation_threshold, vegetation_coverage};
import bevy_pbr::render::mesh_functions::{get_world_from_local, mesh_normal_local_to_world};
import bevy_pbr::render::view_transformations::position_world_to_clip;
import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::pbr_types::{pbr_input_new, STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT};
import bevy_pbr::render::pbr_functions::main_pass_post_lighting_processing;

struct ProxyCoverage {
    leaf: vec4<f32>,
    forest: vec4<f32>,
    impostor: vec4<f32>,
    children: array<vec4<f32>, 6>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> coverage: ProxyCoverage;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var atlas: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var atlas_sampler: sampler;

struct ProxyVertex {
    @builtin(instance_index) instance: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) root_slot: vec4<f32>,
}

struct ProxyOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world: vec4<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(4) @interpolate(flat) mask: vec2<f32>,
}

@vertex
fn vertex(mesh: ProxyVertex) -> ProxyOutput {
    let transform = get_world_from_local(mesh.instance);
    let root = (transform * vec4<f32>(mesh.root_slot.xyz, 1.0)).xyz;
    let to_eye = view.world_position.xyz - root;
    let distance_to_eye = length(to_eye);
    let leaf_mix = 1.0 - smoothstep(coverage.impostor.x, coverage.impostor.y, distance_to_eye);
    var mask: vec2<f32>;
    if coverage.forest.w > 0.5 {
        let slot = u32(mesh.root_slot.w);
        let lower = vegetation_coverage(leaf_mix * coverage.children[slot / 4u][slot % 4u]);
        let far = (1.0 - smoothstep(coverage.forest.x, coverage.forest.y, distance_to_eye)) * coverage.forest.z;
        mask = vec2<f32>(lower, far);
    } else {
        let upper = vegetation_coverage(leaf_mix * coverage.leaf.x);
        let near = 1.0 - smoothstep(coverage.leaf.z, coverage.leaf.w, distance_to_eye);
        let lower = vegetation_coverage(upper * coverage.leaf.y * near);
        mask = vec2<f32>(lower, upper);
    }

    let up = mesh_normal_local_to_world(mesh.normal, mesh.instance);
    let right = cross(up, to_eye);
    let reference = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(up.y) > 0.95);
    let side = select(normalize(cross(up, reference)), right * inverseSqrt(max(dot(right, right), 1e-8)), dot(right, right) > 1e-8);
    // Proxy tiles use translation-only transforms. Pack the signed card offset
    // in color.a and replace its local X with the camera-facing tangent.
    let local = mesh.position + (side - vec3<f32>(1.0, 0.0, 0.0)) * mesh.color.a;
    let normal = cross(side, up);

    let world = transform * vec4<f32>(local, 1.0);
    // Suppress whole trees already covered by a finer representation before rasterization.
    let clip = select(vec4<f32>(2.0, 2.0, 2.0, 1.0), position_world_to_clip(world.xyz), mask.y > mask.x);
    return ProxyOutput(clip, world, normal, mesh.uv, mask);
}

@fragment
fn fragment(in: ProxyOutput) -> @location(0) vec4<f32> {
    // The forest and leaf batches use the same stand-in, including its silhouette.
    let texel = textureSample(atlas, atlas_sampler, in.uv);
    if texel.a < 0.46 {
        discard;
    }
    if in.mask.x > 0.0 || in.mask.y < 1.0 {
        let threshold = vegetation_threshold(in.position.xy);
        if threshold < in.mask.x || threshold >= in.mask.y {
            discard;
        }
    }

    let sun = vec3<f32>(-0.57735, 0.57735, -0.57735);
    // The atlas already contains mild ambient/directional shading baked at
    // portrait generation. Keep the existing proxy/dither pipeline intact,
    // and avoid applying an additional heavy darkness in the middle LOD.
    let shade = 0.90 + 0.18 * abs(dot(normalize(in.normal), sun));
    var pbr = pbr_input_new();
    pbr.material.flags |= STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    pbr.frag_coord = in.position;
    pbr.world_position = in.world;
    return main_pass_post_lighting_processing(pbr, vec4<f32>(texel.rgb * shade, 1.0));
}

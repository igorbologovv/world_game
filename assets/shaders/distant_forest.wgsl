#import bevy_pbr::forward_io::{VertexOutput, FragmentOutput}
#import bevy_pbr::mesh_view_bindings::view
#import bevy_pbr::pbr_types::{pbr_input_new, STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT}
#import bevy_pbr::pbr_functions::main_pass_post_lighting_processing

@group(2) @binding(0)
var<uniform> distances: vec4<f32>;

@fragment
fn fragment(mesh: VertexOutput) -> FragmentOutput {
    let distance_to_eye = distance(mesh.world_position.xyz, view.world_position);
    let near = smoothstep(distances.x - 65.0, distances.x + 28.0, distance_to_eye);
    let far = 1.0 - smoothstep(distances.y, distances.z, distance_to_eye);
    let coverage = near * far;
    // Screen-space coverage keeps both transitions in the opaque phase.
    if coverage < 1.0 {
        let threshold = fract(52.9829189 * fract(dot(floor(mesh.position.xy), vec2<f32>(0.06711056, 0.00583715))));
        if coverage <= threshold {
            discard;
        }
    }

    let normal = normalize(mesh.world_normal);
    let sun = vec3<f32>(-0.57735, 0.57735, -0.57735);
    let shade = 0.56 + 0.39 * saturate(dot(normal, sun));
    var pbr = pbr_input_new();
    pbr.material.flags |= STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    pbr.frag_coord = mesh.position;
    pbr.world_position = mesh.world_position;
    return FragmentOutput(main_pass_post_lighting_processing(pbr, vec4<f32>(mesh.color.rgb * shade, 1.0)));
}

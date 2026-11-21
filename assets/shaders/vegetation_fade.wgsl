#import "shaders/vegetation_dither.wgsl"::vegetation_threshold
#import bevy_pbr::forward_io::{VertexOutput, FragmentOutput}
#import bevy_pbr::mesh_view_bindings::view
#import bevy_pbr::pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT
#import bevy_pbr::pbr_fragment::pbr_input_from_standard_material
#import bevy_pbr::pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing}

@group(2) @binding(100) var<uniform> coverage: vec4<f32>;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var upper = coverage.y;
    if coverage.w > coverage.z {
        upper *= 1.0 - smoothstep(coverage.z, coverage.w, distance(in.world_position.xyz, view.world_position));
    }
    if coverage.x > 0.0 || upper < 1.0 {
        let threshold = vegetation_threshold(in.position.xy);
        if threshold < coverage.x || threshold >= upper {
            discard;
        }
    }

    var pbr = pbr_input_from_standard_material(in, is_front);
    pbr.material.base_color = alpha_discard(pbr.material, pbr.material.base_color);
    var color = pbr.material.base_color;
    if (pbr.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        color = apply_pbr_lighting(pbr);
    }
    return FragmentOutput(main_pass_post_lighting_processing(pbr, color));
}

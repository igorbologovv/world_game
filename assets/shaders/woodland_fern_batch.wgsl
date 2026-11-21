#import "shaders/vegetation_dither.wgsl"::vegetation_threshold
#import bevy_pbr::forward_io::{VertexOutput, FragmentOutput}
#import bevy_pbr::pbr_fragment::pbr_input_from_standard_material
#import bevy_pbr::pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing}

// Two independent fern coverage intervals are packed per vec4.
@group(2) @binding(100) var<uniform> masks: array<vec4<f32>, 8>;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) front: bool) -> FragmentOutput {
    let index = u32(in.uv_b.x + 0.5);
    let pair = masks[index / 2u];
    let interval = select(pair.xy, pair.zw, index % 2u == 1u);
    if interval.x > 0.0 || interval.y < 1.0 {
        let threshold = vegetation_threshold(in.position.xy);
        if threshold < interval.x || threshold >= interval.y {
            discard;
        }
    }

    var pbr = pbr_input_from_standard_material(in, front);
    pbr.material.base_color = alpha_discard(pbr.material, pbr.material.base_color);
    return FragmentOutput(main_pass_post_lighting_processing(pbr, apply_pbr_lighting(pbr)));
}

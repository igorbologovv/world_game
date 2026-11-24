import super::vegetation_dither::vegetation_threshold;
@if(TREE_CROWN) import super::tree_crown_lighting::shade_broadleaf;
@if(TREE_CROWN) import bevy_pbr::render::mesh_functions::get_local_from_world;
@if(TREE_CROWN) import bevy_pbr::render::mesh_view_bindings::lights;
import bevy_pbr::render::forward_io::{VertexOutput, FragmentOutput};
import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT;
import bevy_pbr::render::pbr_fragment::pbr_input_from_standard_material;
import bevy_pbr::render::pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing};

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> coverage: vec4<f32>;
@if(TREE_CROWN)
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var<uniform> crown_frame: vec2<f32>;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var upper = coverage.y;
    if coverage.w > coverage.z {
        upper *= 1.0 - smoothstep(coverage.z, coverage.w, distance(in.world_position.xyz, view.world_position.xyz));
    }
    if coverage.x > 0.0 || upper < 1.0 {
        let threshold = vegetation_threshold(in.position.xy);
        if threshold < coverage.x || threshold >= upper {
            discard;
        }
    }

    var pbr = pbr_input_from_standard_material(in, is_front);
    pbr.material.base_color = alpha_discard(pbr.material.flags, pbr.material.alpha_cutoff, pbr.material.base_color);
    var color = pbr.material.base_color;
    if (pbr.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        @if(TREE_CROWN)
        if crown_frame.y > 0.0 {
            let local_from_world = get_local_from_world(in.instance_index);
            let local = (local_from_world * in.world_position).xyz;
            let point = (local - vec3<f32>(0.0, crown_frame.x, 0.0)) / crown_frame.y;
            var sun = vec3<f32>(0.0, 1.0, 0.0);
            if lights.n_directional_lights > 0u {
                sun = (local_from_world * vec4<f32>(lights.directional_lights[0].direction_to_light, 0.0)).xyz;
            }
            shade_broadleaf(&pbr, point, sun, 1.0);
        }
        color = apply_pbr_lighting(pbr);
    }
    return FragmentOutput(main_pass_post_lighting_processing(pbr, color));
}

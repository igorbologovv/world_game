//! Bevy-specific shading and output for the pinned Igor compute source.

pub(super) fn replace_once(source: &str, original: &str, replacement: &str) -> String {
    let (head, tail) = source
        .split_once(original)
        .expect("review the Igor shader adapter when updating the renderer pin");
    assert!(!tail.contains(original), "ambiguous Igor shader adaptation");
    [head, replacement, tail].concat()
}

pub(super) fn compute() -> String {
    let source = igor_renderer::planetary::compute_shader();
    let source = replace_once(
        &source,
        "    let sun=normalize(cam.sun.xyz);\n    return c*(0.10+0.24*max(dot(n,sun),0.0)*planet_visibility(world));",
        r#"    let sun = normalize(cam.sun.xyz);
    let up = cam.extent.xyz;
    let hemisphere = 0.5 + 0.5 * dot(n, up);
    let daylight = smoothstep(-0.08, 0.12, dot(sun, up));
    let exposure = max(max(cam.sunlight.r, cam.sunlight.g), cam.sunlight.b);
    let sky = mix(vec3<f32>(0.035, 0.039, 0.045), vec3<f32>(0.066, 0.085, 0.12), hemisphere);
    let ambient = sky * (0.008 + exposure * daylight);
    let direct = cam.sunlight.rgb * (max(dot(n, sun), 0.0) * (1.0 / 3.14159265))
        * planet_visibility(world) * daylight;
    let haze = 1.0 - exp(-length(world) * 0.000075);
    let airlight = vec3<f32>(0.08, 0.18, 0.30) * (0.1 * exposure * daylight);
    return mix(c * (ambient + direct), airlight, haze);"#,
    );
    replace_once(
        &source,
        "atomicStore(&screen_words[2u*pixel_count()+i],pack4x8unorm(vec4<f32>(color,1.0)));",
        r#"let n = pixel_count();
    atomicStore(&screen_words[2u*n+i], pack2x16float(color.rg));
    atomicStore(&screen_words[3u*n+i], pack2x16float(vec2<f32>(color.b, 1.0)));"#,
    )
}

pub(super) fn ground(multisampled: bool) -> String {
    let source = igor_renderer::planetary::ground_shadow_shader();
    if !multisampled {
        return source;
    }
    let source = replace_once(
        &source,
        "scene_depth:texture_depth_2d",
        "scene_depth:texture_depth_multisampled_2d",
    );
    let source = replace_once(
        &source,
        "pos:vec4<f32>)->",
        "pos:vec4<f32>, @builtin(sample_index) sample:u32)->",
    );
    replace_once(
        &source,
        "textureLoad(scene_depth,vec2<i32>(pos.xy),0)",
        "textureLoad(scene_depth,vec2<i32>(pos.xy),i32(sample))",
    )
}

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
    let source = replace_once(
        &source,
        "atomicStore(&screen_words[2u*pixel_count()+i],pack4x8unorm(vec4<f32>(color,1.0)));",
        r#"let n = pixel_count();
    atomicStore(&screen_words[2u*n+i], pack2x16float(color.rg));
    atomicStore(&screen_words[3u*n+i], pack2x16float(vec2<f32>(color.b, 1.0)));"#,
    );
    let source = replace_once(
        &source,
        "sin(t*6.2+dot(world,vec3<f32>(2.1,1.7,1.3)))",
        "sin(t*6.2+cam.effects.w+dot(world,vec3<f32>(2.1,1.7,1.3)))",
    );
    // Only retain the bounds needed by the owner pass. Depth only decreases
    // during the first pass, so a splat with no accepted sample stays hidden.
    let source = replace_once(
        &source,
        "    if !owner_pass && cache_enabled(){projection_cache[id]=vec2<u32>(pack_xy(p.lo),pack_xy(p.hi));}",
        "    var emitted = false;",
    );
    let source = replace_once(
        &source,
        "                atomicMin(&screen_words[i],key);\n            }\n        }\n    }",
        r#"                atomicMin(&screen_words[i],key);
                emitted = true;
            }
        }
    }
    if !owner_pass && cache_enabled() && emitted {
        projection_cache[id] = vec2<u32>(pack_xy(p.lo), pack_xy(p.hi));
    }"#,
    );
    shadow_blend(&source, 13)
}

pub(super) fn ground(multisampled: bool) -> String {
    let source = shadow_blend(&igor_renderer::planetary::ground_shadow_shader(), 4);
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

fn shadow_blend(source: &str, binding: u32) -> String {
    let source = replace_once(
        source,
        "shadow_x:vec4<f32>,shadow_z:vec4<f32>,shadow_height:vec4<f32>,",
        "shadow_x:vec4<f32>,shadow_z:vec4<f32>,shadow_height:vec4<f32>,\n    previous_shadow_x:vec4<f32>,previous_shadow_z:vec4<f32>,previous_shadow_height:vec4<f32>,shadow_blend:vec4<f32>,",
    );
    let (head, _) = source
        .split_once("\nfn planet_visibility(")
        .expect("review the Igor canopy shadow adapter when updating the renderer pin");
    let decl =
        format!("@group(0) @binding({binding}) var previous_canopy_shadow: texture_2d<f32>;");
    [head, "\n", &decl, SHADOW_BLEND].concat()
}

const SHADOW_BLEND: &str = r#"
fn field_visibility(field: texture_2d<f32>, world: vec3<f32>, x: vec4<f32>,
    z: vec4<f32>, band: vec4<f32>, strength: f32) -> f32 {
    if strength <= 0.0 { return 1.0; }
    let uv = vec2<f32>(dot(world, x.xyz) + x.w, dot(world, z.xyz) + z.w);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) { return 1.0; }
    let height = dot(world, band.xyz) + band.w;
    let opacity = textureSampleLevel(field, atlas_sampler, uv, 0.0);
    let above = smoothstep(vec4<f32>(0.125), vec4<f32>(1.125), vec4<f32>(0.5, 1.5, 2.5, 3.5) - height);
    let edge = smoothstep(0.0, 0.08, min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y)));
    let t = vec4<f32>(1.0) - opacity * above * strength * edge;
    return max(0.12, t.x * t.y * t.z * t.w);
}

fn planet_visibility(world: vec3<f32>) -> f32 {
    let visibility = field_visibility(canopy_shadow, world, cam.shadow_x, cam.shadow_z,
        cam.shadow_height, cam.effects.z);
    if cam.shadow_blend.x >= 1.0 { return visibility; }
    let previous = field_visibility(previous_canopy_shadow, world, cam.previous_shadow_x,
        cam.previous_shadow_z, cam.previous_shadow_height, cam.shadow_blend.y);
    return mix(previous, visibility, cam.shadow_blend.x);
}
"#;

pub(super) fn temporal() -> String {
    // Adapt the standalone reprojection to the Bevy viewport and point-only
    // HDR buffer. Reject uncovered pixels and keep the unjittered projection.
    let source = replace_once(
        igor_renderer::render_api::TEMPORAL_SHADER,
        "screen:vec4<f32>, extent:vec4<f32>, hud:array<vec4<u32>,8>, params:vec4<u32>,",
        "screen:vec4<f32>,",
    );
    let source = replace_once(
        &source,
        "@group(0) @binding(2) var current:texture_2d<f32>;",
        "@group(0) @binding(2) var<storage,read> words:array<u32>;",
    );
    let source = replace_once(
        &source,
        "@group(0) @binding(5) var<storage,read_write> depths:array<atomic<u32>>;",
        r#"fn current_sample(p: vec2<i32>, size: vec2<u32>) -> vec4<f32> {
    let n = size.x * size.y;
    let i = u32(p.x) + u32(p.y) * size.x;
    if words[n + i] == 0xffffffffu { return vec4<f32>(0.0); }
    let rg = unpack2x16float(words[2u * n + i]);
    let ba = unpack2x16float(words[3u * n + i]);
    return vec4<f32>(rg, ba.x, bitcast<f32>(words[i]));
}"#,
    );
    let source = replace_once(
        &source,
        "    let z=bitcast<f32>(atomicLoad(&depths[id.x+id.y*size.x]));\n    let color=textureLoad(current,p,0).rgb;",
        r#"    let fresh = current_sample(p, size);
    let z = fresh.a;
    if z <= 0.0 {
        textureStore(output_history, p, vec4<f32>(0.0));
        return;
    }
    let color = fresh.rgb;"#,
    );
    let source = replace_once(
        &source,
        "var rel=dir;\n        if z>0.0{rel=cam.origin.xyz+dir*z-old.origin.xyz;}",
        "let rel=old.origin.xyz+dir*z;",
    );
    let source = replace_once(
        &source,
        "select(h.a==0.0,h.a>0.0&&abs(hz-prev_z)<=max(0.25,prev_z*0.025),z>0.0)",
        "h.a>0.0&&abs(hz-prev_z)<=max(0.08,prev_z*0.012)",
    );
    let source = replace_once(
        &source,
        "let c=textureLoad(current,clamp(p+vec2<i32>(x,y),vec2<i32>(0),vec2<i32>(size)-1),0).rgb;",
        r#"let sample = current_sample(clamp(p+vec2<i32>(x,y),vec2<i32>(0),vec2<i32>(size)-1),size);
                            if sample.a <= 0.0 || abs(sample.a-z)>max(0.08,z*0.015){continue;}
                            let c = sample.rgb;"#,
    );
    replace_once(&source, "mix(0.88,0.50,", "mix(0.62,0.22,")
}

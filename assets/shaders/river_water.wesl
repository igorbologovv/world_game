import bevy_pbr::render::forward_io::{Vertex, decompress_vertex};
import bevy_pbr::render::mesh_functions::get_world_from_local;
import bevy_pbr::render::mesh_view_bindings as bindings;
import bevy_pbr::render::view_transformations::position_world_to_clip;

@group(constants::MATERIAL_BIND_GROUP) @binding(0)
var ripple_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(1)
var ripple_sampler: sampler;

struct InlandVertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) local: vec3<f32>,
    @location(2) radial: vec3<f32>,
    @location(3) current: vec3<f32>,
    @location(4) depth: f32,
    @location(5) bank: f32,
    @location(6) @interpolate(flat) kind: f32,
}

@vertex
fn vertex(input: Vertex) -> InlandVertex {
    let mesh = decompress_vertex(input, input.instance_index);
    let world = get_world_from_local(mesh.instance_index) * vec4<f32>(mesh.position, 1.0);
    return InlandVertex(position_world_to_clip(world.xyz), world.xyz, mesh.position,
        mesh.normal, mesh.color.xyz, mesh.color.w, mesh.uv.x, mesh.uv.y);
}

struct Ripples {
    slope: vec3<f32>,
    foam: f32,
    sediment: f32,
}

fn ripples(point: vec3<f32>, up: vec3<f32>, scale: f32) -> Ripples {
    let x = textureSample(ripple_texture, ripple_sampler, point.yz * scale);
    let y = textureSample(ripple_texture, ripple_sampler, point.zx * scale);
    let z = textureSample(ripple_texture, ripple_sampler, point.xy * scale);
    let weight = pow(abs(up), vec3<f32>(6.0));
    let w = weight / max(dot(weight, vec3<f32>(1.0)), 0.0001);
    let gx = vec3<f32>(0.0, x.r * 2.0 - 1.0, x.g * 2.0 - 1.0);
    let gy = vec3<f32>(y.g * 2.0 - 1.0, 0.0, y.r * 2.0 - 1.0);
    let gz = vec3<f32>(z.r * 2.0 - 1.0, z.g * 2.0 - 1.0, 0.0);
    return Ripples(gx * w.x + gy * w.y + gz * w.z, dot(vec3<f32>(x.b, y.b, z.b), w),
        dot(vec3<f32>(x.a, y.a, z.a), w));
}

@fragment
fn fragment(mesh: InlandVertex) -> @location(0) vec4<f32> {
    let up = normalize(mesh.radial);
    let eye = normalize(bindings::view.world_position.xyz - mesh.world);
    let footprint = max(length(dpdx(mesh.local)), length(dpdy(mesh.local)));
    let wetland = mesh.kind > 1.5;
    let river = mesh.kind < 0.5;
    let depth = max(mesh.depth, 0.0);
    let contact = smoothstep(-0.035, max(0.055, fwidth(mesh.depth) * 0.55), mesh.depth);
    let bank = select(1.0, 1.0 - smoothstep(0.985, 1.04, abs(mesh.bank)), river);
    let coverage = contact * bank;

    var surface = up;
    var sediment = 0.5;
    var foam = 0.0;
    if footprint < 3.0 {
        let detail = 1.0 - smoothstep(0.45, 3.0, footprint);
        // Two offset phases keep direction changes at confluences stable even
        // after long sessions. The flow field advects both wave slopes and flecks.
        let clock = bindings::globals.time;
        let phase = fract(clock / 4.0);
        let blend = 1.0 - abs(2.0 * phase - 1.0);
        // Patch anchors are multiples of 128 m, so all projections meet
        // continuously across different patch origins and floating-origin shifts.
        let velocity = mesh.current * 0.61 + vec3<f32>(0.16, 0.035, 0.10);
        let a = ripples(mesh.local - velocity * (phase * 4.0), up, 0.125);
        let b = ripples(mesh.local - velocity * (fract(phase + 0.5) * 4.0) + vec3<f32>(1.7),
            up, 0.125);
        let fine = ripples(mesh.local + velocity * (blend * 1.8) + vec3<f32>(0.0, 0.4, -1.6),
            up, 0.5);
        let ripple = mix(b.slope, a.slope, blend) + fine.slope * 0.30;
        let slope = ripple - up * dot(ripple, up);
        let calm = select(0.31, 0.105, wetland);
        surface = normalize(up - slope * (calm * detail) * smoothstep(0.015, 0.21, depth));
        sediment = mix(0.5, mix(b.sediment, a.sediment, blend), detail);
        let current = length(mesh.current);
        let broken = smoothstep(0.55, 0.87, mix(b.foam, a.foam, blend) * (0.78 + 0.32 * fine.foam));
        let shallow = smoothstep(0.05, 0.25, depth) * (1.0 - smoothstep(0.7, 2.2, depth));
        foam = detail * shallow * broken * smoothstep(0.75, 2.8, current) * 0.53;
    }

    let facing = max(dot(up, eye), 0.0);
    let grazing = 1.0 - saturate(dot(surface, eye));
    let fresnel = 0.018 + 0.982 * pow(grazing, 5.0);
    let path = depth / mix(0.47, 1.0, sqrt(facing));
    let absorption = select(vec3<f32>(1.10, 0.55, 0.36), vec3<f32>(2.5, 1.65, 3.3), wetland);
    let transmittance = exp(-absorption * path);
    let clarity = dot(transmittance, vec3<f32>(0.28, 0.45, 0.27));
    let river_color = vec3<f32>(0.084, 0.189, 0.152);
    let lake_color = vec3<f32>(0.046, 0.142, 0.166);
    let wetland_color = vec3<f32>(0.114, 0.125, 0.056);
    let body = select(select(lake_color, river_color, river), wetland_color, wetland)
        * mix(0.86, 1.14, sediment);

    var sun = vec3<f32>(0.0, 1.0, 0.0);
    var sun_color = vec3<f32>(0.0);
    var sun_up = -1.0;
    if bindings::lights.n_directional_lights > 0u {
        let light = &bindings::lights.directional_lights[0];
        sun = (*light).direction_to_light;
        sun_color = (*light).color.rgb * bindings::view.exposure;
        sun_up = dot(up, sun);
    }
    let daylight = mix(0.11, 1.0, smoothstep(-0.12, 0.63, sun_up));
    let reflection = reflect(-eye, surface);
    let sky_height = smoothstep(0.0, 0.91, dot(reflection, up));
    let sky_horizon = mix(vec3<f32>(0.26, 0.38, 0.43), vec3<f32>(0.21, 0.28, 0.23), select(0.0, 1.0, wetland));
    let sky = mix(sky_horizon, vec3<f32>(0.067, 0.173, 0.33), sky_height) * daylight;
    let glint = pow(max(dot(reflection, sun), 0.0), select(190.0, 350.0, wetland));
    let sunlit = min(sun_color * glint * smoothstep(0.0, 0.14, sun_up) * 0.14, vec3<f32>(2.2));

    let opacity = clamp(1.0 - (1.0 - fresnel) * clarity, 0.0, 1.0);
    // Standard alpha compositing leaves the real terrain visible through the
    // shallows, including the wet gravel, mud, rocks and vegetation on the bed.
    let scatter = body * daylight * (1.0 - clarity);
    let reflected = (sky * fresnel + sunlit * (0.12 + 0.88 * fresnel));
    let color = (scatter + reflected) / max(opacity, 0.045);
    let whitewater = vec3<f32>(0.56, 0.66, 0.61) * daylight;
    return vec4<f32>(mix(color, whitewater, foam), coverage * mix(opacity, 0.94, foam));
}

import super::vegetation_dither::{vegetation_threshold, vegetation_coverage};
import bevy_pbr::render::mesh_functions::get_world_from_local;
import bevy_pbr::render::view_transformations::position_world_to_clip;
import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::pbr_types::{pbr_input_new, STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT};
import bevy_pbr::render::pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing};

struct ProxyCoverage {
    leaf: vec4<f32>,
    forest: vec4<f32>,
    impostor: vec4<f32>,
    children: array<vec4<f32>, 6>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> coverage: ProxyCoverage;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var atlas: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var atlas_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var normal_atlas: texture_2d_array<f32>;

struct ProxyVertex {
    @builtin(instance_index) instance: u32,
    @location(0) center: vec3<f32>,
    @location(1) up: vec3<f32>,
    @location(2) tile_uv: vec2<f32>,
    @location(3) forward_radius: vec4<f32>,
    @location(4) root_slot: vec4<f32>,
}

struct ProxyOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) tile: f32,
    @location(3) @interpolate(flat) mask: vec2<f32>,
    @location(4) @interpolate(flat) up: vec3<f32>,
    @location(5) @interpolate(flat) forward: vec3<f32>,
    @location(6) @interpolate(flat) eye: vec3<f32>,
    @location(7) @interpolate(flat) axis: vec2<f32>,
    @location(8) @interpolate(flat) layers: vec3<i32>,
    @location(9) @interpolate(flat) weights: vec3<f32>,
}

@vertex
fn vertex(mesh: ProxyVertex) -> ProxyOutput {
    let transform = get_world_from_local(mesh.instance);
    let root = (transform * vec4<f32>(mesh.root_slot.xyz, 1.0)).xyz;
    let distance_to_eye = distance(view.world_position, root);
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

    if mask.y <= mask.x {
        return ProxyOutput(vec4<f32>(2.0, 2.0, 2.0, 1.0), vec3<f32>(0.0), vec2<f32>(0.0),
            0.0, mask, vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0), vec2<f32>(0.0),
            vec3<i32>(0), vec3<f32>(0.0));
    }
    // The rotation in each vertex is the same site rotation used by the detailed mesh.
    let center = (transform * vec4<f32>(mesh.center, 1.0)).xyz;
    let up = mesh.up;
    let forward = mesh.forward_radius.xyz;
    let local_x = cross(up, forward);
    let to_eye = normalize(view.world_position - center);
    let eye = vec3<f32>(dot(to_eye, local_x), dot(to_eye, up), dot(to_eye, forward));
    let horizontal = eye.xz;
    let axis = select(vec2<f32>(0.0, 1.0), horizontal * inverseSqrt(max(dot(horizontal, horizontal), 1e-8)), dot(horizontal, horizontal) > 1e-8);
    let side = axis.y * local_x - axis.x * forward;
    let rise = cross(to_eye, side);
    let tile = floor(mesh.tile_uv.x);
    let uv = vec2<f32>((fract(mesh.tile_uv.x) - 0.25) * 2.0, mesh.tile_uv.y);
    let world = center + mesh.forward_radius.w * (side * (2.0 * uv.x - 1.0) + rise * (1.0 - 2.0 * uv.y));
    let views = select_views(eye);
    return ProxyOutput(position_world_to_clip(world), world, uv, tile, mask, up, forward, eye, axis, views.layers, views.weights);
}

const HORIZON_VIEWS: i32 = 16;
const ELEVATED_VIEWS: i32 = 8;
const TOP_VIEW: i32 = HORIZON_VIEWS + ELEVATED_VIEWS;
const HORIZON: f32 = -0.06981317;
const ELEVATED: f32 = 0.69813170;
const ZENITH: f32 = 1.57079633;

struct Views {
    layers: vec3<i32>,
    weights: vec3<f32>,
}

fn select_views(eye: vec3<f32>) -> Views {
    let azimuth = atan2(eye.x, eye.z + 1e-8);
    let elevation = asin(clamp(eye.y, -1.0, 1.0));
    let sector = fract(azimuth / 6.28318530718) * f32(ELEVATED_VIEWS);
    let h0 = i32(floor(sector));
    let h1 = (h0 + 1) % ELEVATED_VIEWS;
    let l0 = 2 * h0;
    let l1 = l0 + 1;
    let l2 = 2 * h1;
    let angle = fract(sector);

    // The horizon ring has twice as many views. Triangulate the grid to
    // interpolate at most three images, with matching weights at the edges.
    let elevated = smoothstep(0.10, ELEVATED, elevation);
    let overhead = smoothstep(ELEVATED, ZENITH, elevation);
    let lower = 1.0 - elevated;
    var layers: vec3<i32>;
    var weights: vec3<f32>;
    if overhead > 0.0 {
        layers = vec3<i32>(h0 + HORIZON_VIEWS, h1 + HORIZON_VIEWS, TOP_VIEW);
        weights = vec3<f32>((1.0 - overhead) * (1.0 - angle), (1.0 - overhead) * angle, overhead);
    } else if 2.0 * angle <= lower {
        layers = vec3<i32>(l0, l1, h0 + HORIZON_VIEWS);
        weights = vec3<f32>(lower - 2.0 * angle, 2.0 * angle, elevated);
    } else if 2.0 * (1.0 - angle) <= lower {
        layers = vec3<i32>(l1, l2, h1 + HORIZON_VIEWS);
        weights = vec3<f32>(2.0 * (1.0 - angle), lower - 2.0 * (1.0 - angle), elevated);
    } else {
        let upper_right = angle - 0.5 * lower;
        layers = vec3<i32>(l1, h0 + HORIZON_VIEWS, h1 + HORIZON_VIEWS);
        weights = vec3<f32>(lower, elevated - upper_right, upper_right);
    }
    return Views(layers, weights);
}

struct Basis {
    side: vec3<f32>,
    rise: vec3<f32>,
    eye: vec3<f32>,
}

fn basis(layer: i32) -> Basis {
    if layer == TOP_VIEW {
        return Basis(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, -1.0), vec3<f32>(0.0, 1.0, 0.0));
    }
    let elevated = layer >= HORIZON_VIEWS;
    let index = select(layer, layer - HORIZON_VIEWS, elevated);
    let azimuth = f32(index) * select(0.39269908170, 0.78539816339, elevated);
    let angle = select(HORIZON, ELEVATED, elevated);
    let side = vec3<f32>(cos(azimuth), 0.0, -sin(azimuth));
    let eye = vec3<f32>(sin(azimuth) * cos(angle), sin(angle), cos(azimuth) * cos(angle));
    return Basis(side, cross(eye, side), eye);
}

fn project(bake: Basis, point: vec3<f32>) -> vec2<f32> {
    return vec2<f32>(dot(point, bake.side), -dot(point, bake.rise)) * 0.5 + 0.5;
}

fn atlas_uv(uv: vec2<f32>, tile: f32, tile_scale: f32, inset: f32) -> vec2<f32> {
    let safe = clamp(uv, vec2<f32>(inset), vec2<f32>(1.0 - inset));
    return vec2<f32>((tile + safe.x) * tile_scale, safe.y);
}

fn decode_normal(encoded: vec2<f32>) -> vec3<f32> {
    let oct = encoded * 2.0 - 1.0;
    let z = 1.0 - abs(oct.x) - abs(oct.y);
    let unfolded = oct - clamp(-z, 0.0, 1.0) * select(vec2<f32>(-1.0), vec2<f32>(1.0), oct >= vec2<f32>(0.0));
    return normalize(vec3<f32>(unfolded, z));
}

struct Capture {
    color: vec4<f32>,
    surface: vec4<f32>,
}

fn sample_view(
    in: ProxyOutput, point: vec3<f32>, layer: i32, weight: f32,
    tile_scale: f32, inset: f32, parallax: f32, dx: vec2<f32>, dy: vec2<f32>,
) -> Capture {
    if weight <= 0.005 {
        return Capture(vec4<f32>(0.0), vec4<f32>(0.0));
    }
    let camera = basis(layer);
    let plane_uv = project(camera, point);
    let plane_atlas_uv = atlas_uv(plane_uv, in.tile, tile_scale, inset);
    // Reproject the sampled surface depth into this view. Near fronds and
    // branches line up before their coverage is interpolated.
    var sampled = vec4<f32>(0.0);
    var displacement = 0.0;
    if parallax > 0.0 {
        sampled = textureSampleGrad(normal_atlas, atlas_sampler, plane_atlas_uv, layer, dx, dy);
        let plane_depth = dot(point, camera.eye);
        let depth = select(plane_depth, sampled.a * 2.0 - 1.0, sampled.a > 0.0);
        displacement = parallax * clamp((depth - plane_depth) / max(dot(in.eye, camera.eye), 0.35), -0.65, 0.65);
    }
    let uv = project(camera, point + in.eye * displacement);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) {
        return Capture(vec4<f32>(0.0), vec4<f32>(0.0));
    }
    let lookup = atlas_uv(uv, in.tile, tile_scale, inset);
    var albedo = textureSampleGrad(atlas, atlas_sampler, lookup, layer, dx, dy);
    var surface = textureSampleGrad(normal_atlas, atlas_sampler, lookup, layer, dx, dy);
    if albedo.a < 0.45 && abs(displacement) > 0.02 {
        let original = textureSampleGrad(atlas, atlas_sampler, plane_atlas_uv, layer, dx, dy);
        if 0.75 * original.a > albedo.a {
            albedo = vec4<f32>(original.rgb, 0.75 * original.a);
            surface = sampled;
        }
    }
    let alpha = weight * albedo.a;
    return Capture(vec4<f32>(albedo.rgb * alpha, alpha), vec4<f32>(decode_normal(surface.rg), surface.b) * alpha);
}

@fragment
fn fragment(in: ProxyOutput) -> @location(0) vec4<f32> {
    // Derivatives stay continuous across view boundaries and drive both atlases.
    let dims = vec2<f32>(textureDimensions(atlas, 0));
    let tile_scale = dims.y / dims.x;
    let uv_dx = dpdx(in.uv);
    let uv_dy = dpdy(in.uv);
    let dx = uv_dx * vec2<f32>(tile_scale, 1.0);
    let dy = uv_dy * vec2<f32>(tile_scale, 1.0);
    let footprint = max(length(uv_dx), length(uv_dy));
    let inset = clamp(0.5 * footprint, 0.5 / dims.y, 0.5 / 6.0);
    let parallax = 1.0 - smoothstep(1.0 / 45.0, 1.0 / 23.0, footprint);
    if in.mask.x > 0.0 || in.mask.y < 1.0 {
        let threshold = vegetation_threshold(in.position.xy);
        if threshold < in.mask.x || threshold >= in.mask.y {
            discard;
        }
    }
    let right = vec3<f32>(in.axis.y, 0.0, -in.axis.x);
    let point = right * (2.0 * in.uv.x - 1.0) + cross(in.eye, right) * (1.0 - 2.0 * in.uv.y);
    let p0 = sample_view(in, point, in.layers.x, in.weights.x, tile_scale, inset, parallax, dx, dy);
    let p1 = sample_view(in, point, in.layers.y, in.weights.y, tile_scale, inset, parallax, dx, dy);
    let p2 = sample_view(in, point, in.layers.z, in.weights.z, tile_scale, inset, parallax, dx, dy);
    let albedo = p0.color + p1.color + p2.color;
    if albedo.a < 0.37 {
        discard;
    }
    let surface = (p0.surface + p1.surface + p2.surface) / albedo.a;
    let normal = surface.xyz;
    let world_normal = normalize(cross(in.up, in.forward) * normal.x + in.up * normal.y + in.forward * normal.z);

    var pbr = pbr_input_new();
    pbr.material.base_color = vec4<f32>(albedo.rgb / albedo.a, 1.0);
    pbr.material.perceptual_roughness = 0.85;
    pbr.material.diffuse_transmission = surface.a;
    pbr.material.reflectance = vec3<f32>(mix(0.5, 0.16, clamp(surface.a / 0.34, 0.0, 1.0)));
    pbr.material.flags |= STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    pbr.frag_coord = in.position;
    pbr.world_position = vec4<f32>(in.world, 1.0);
    pbr.world_normal = world_normal;
    pbr.N = world_normal;
    pbr.V = normalize(view.world_position - in.world);
    return main_pass_post_lighting_processing(pbr, apply_pbr_lighting(pbr));
}

struct Camera {
    origin:vec4<f32>,forward:vec4<f32>,right:vec4<f32>,up:vec4<f32>,screen:vec4<f32>,extent:vec4<f32>,
    params:vec4<u32>,effects:vec4<f32>,sun:vec4<f32>,sunlight:vec4<f32>,depth_coeff:vec4<f32>,viewport:vec4<u32>,
}
@group(0) @binding(0) var<uniform> cam:Camera;
@group(0) @binding(1) var<storage,read> screen_words:array<u32>;
@group(0) @binding(2) var resolved: texture_2d<f32>;
struct Output{@location(0) color:vec4<f32>,@builtin(frag_depth) depth:f32}
@fragment fn fragment(@builtin(position) pos:vec4<f32>)->Output{
    let pixel=vec2<u32>(pos.xy)-cam.viewport.xy;
    if any(pixel>=cam.viewport.zw){discard;}
    let n=cam.viewport.z*cam.viewport.w;let i=pixel.x+pixel.y*cam.viewport.z;
    if screen_words[n+i]==0xffffffffu{discard;}
    let z=-bitcast<f32>(screen_words[i]);
    let depth=(cam.depth_coeff.x*z+cam.depth_coeff.y)/(cam.depth_coeff.z*z+cam.depth_coeff.w);
    let color = textureLoad(resolved, vec2<i32>(pixel), 0).rgb;
    return Output(vec4<f32>(color, 1.0), clamp(depth, 0.0, 1.0));
}

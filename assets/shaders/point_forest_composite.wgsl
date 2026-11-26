// Only a fullscreen presentation primitive. No tree polygons.
// Bevy's reversed-Z depth test merges the point result with opaque terrain.
struct Frame { clip:mat4x4<f32>,viewport:vec4<u32>,params:vec4<u32>,sun_time:vec4<f32>,sunlight:vec4<f32> }
@group(0) @binding(0) var<uniform> cam:Frame;
@group(0) @binding(1) var<storage,read> screen:array<u32>;
struct Output { @location(0) color:vec4<f32>,@builtin(frag_depth) depth:f32 }
@fragment
fn fragment(@builtin(position) pos:vec4<f32>)->Output{
    let pixel=vec2<u32>(pos.xy)-cam.viewport.xy;
    if any(pixel>=cam.viewport.zw){discard;}
    let count=cam.viewport.z*cam.viewport.w;
    let i=pixel.x+pixel.y*cam.viewport.z;
    if screen[count+i]==0u{discard;}
    let color=unpack4x8unorm(screen[2u*count+i]).rgb*cam.sunlight.rgb;
    return Output(vec4<f32>(color,1.0),bitcast<f32>(screen[i]));
}

struct Camera {
    origin:vec4<f32>, forward:vec4<f32>, right:vec4<f32>, up:vec4<f32>,
    screen:vec4<f32>, extent:vec4<f32>, hud:array<vec4<u32>,8>,
}
@group(0) @binding(0) var<uniform> cam:Camera;
@group(0) @binding(1) var resolved:texture_2d<f32>;
@vertex fn fullscreen_vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32>{
    return vec4<f32>(f32((i<<1u)&2u)*2.0-1.0,f32(i&2u)*2.0-1.0,0.0,1.0);
}
@fragment fn present_fs(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32>{
    return textureLoad(resolved,vec2<i32>(p.xy),0);
}
@fragment fn hud_fs(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32>{
    let hud_scale=select(1,2,cam.screen.x>=186.0&&cam.screen.y>=52.0);
    let hud_pixel=vec2<i32>(p.xy)/hud_scale;
    if hud_pixel.x<5||hud_pixel.y<5||hud_pixel.x>=93||hud_pixel.y>=26{discard;}
    let q=hud_pixel-vec2<i32>(9,8);
    if q.x>=0&&q.y>=0 {
        let digit=q.x/5;let line=q.y/8;let gx=q.x%5;let gy=q.y%8;
        if digit<16&&line<2&&gx<4&&gy<6 {
            let i=u32(line*16+digit);
            let glyph=cam.hud[i/4u][i%4u];
            let bit=u32(23-gy*4-gx);
            if ((glyph>>bit)&1u)!=0u {return vec4<f32>(0.88,0.94,0.90,1.0);}
        }
    }
    return vec4<f32>(0.014,0.022,0.019,1.0);
}

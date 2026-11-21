fn vegetation_threshold(pixel: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(floor(pixel), vec2<f32>(0.06711056, 0.00583715))));
}

fn vegetation_coverage(value: f32) -> f32 {
    return floor(clamp(value, 0.0, 1.0) * 12.0 + 0.5) / 12.0;
}

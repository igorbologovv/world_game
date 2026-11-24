const TAU: f32 = 6.2831853;

struct Swell {
    height: f32,
    slope: vec3<f32>,
    crest_signal: f32,
}

// All axes use integer coefficients and power-of-two scales. Subtracting an
// aligned origin before evaluating the phases keeps waves fixed as big_space
// moves the render origin, with meter-scale precision on the whole planet.
fn wave_position(world: vec3<f32>, center: vec3<f32>, period: f32) -> vec3<f32> {
    return world - (center - round(center / period) * period);
}

fn wave_phase(p: vec3<f32>, k: vec3<f32>, clock: f32, cycles: f32) -> f32 {
    return TAU * (fract(dot(p, k)) - fract(clock * cycles));
}

fn wind_field(p: vec3<f32>, clock: f32) -> vec2<f32> {
    return vec2<f32>(
        sin(wave_phase(p, vec3<f32>(1.0, 3.0, -2.0) / 32768.0, clock, 13.0)),
        sin(wave_phase(p, vec3<f32>(-3.0, 1.0, -1.0) / 8192.0, clock, 29.0)),
    );
}

fn wave_band(
    p: vec3<f32>, k: vec3<f32>, clock: f32, cycles: f32, amplitude: f32,
    warp: f32, dx: vec3<f32>, dy: vec3<f32>, minimum_width: f32, geometry: bool,
) -> Swell {
    let k_length = length(k);
    let phase_step = max(max(abs(dot(dx, k)), abs(dot(dy, k))), minimum_width * k_length);
    // Filter along the wave vector, using the actual ring basis in the
    // vertex stage. The sharper overtone needs twice the sampling rate.
    let limit = select(vec2<f32>(0.14, 0.48), vec2<f32>(0.095, 0.35), geometry);
    let weight = 1.0 - smoothstep(limit.x, limit.y, phase_step);
    if weight == 0.0 {
        return Swell(0.0, vec3<f32>(0.0), 0.0);
    }
    let phase = wave_phase(p, k, clock, cycles) + warp;
    let crest = sin(phase);
    let a = amplitude * weight;
    let harmonic = 0.19 * (1.0 - smoothstep(limit.x, limit.y, 2.0 * phase_step));
    // The second harmonic sharpens the crest and rounds the trough. Its
    // analytic derivative also drives the optical normal between vertices.
    let height = a * (crest + harmonic * (2.0 * crest * crest - 1.0));
    let slope = (TAU * a * (1.0 + 4.0 * harmonic * crest) * cos(phase)) * k;
    return Swell(height, slope, TAU * k_length * height);
}

fn swell_field(
    p: vec3<f32>, wind: vec2<f32>, clock: f32,
    dx: vec3<f32>, dy: vec3<f32>, minimum_width: f32, geometry: bool, storm: f32,
) -> Swell {
    // Rates follow deep-water dispersion at Mars gravity, rounded to whole
    // cycles per hour so the Bevy time wrap is continuous.
    let a = wave_band(p, vec3<f32>(2.0, -1.0, 1.0) / 2048.0, clock, 96.0, 4.4 * (1.0 + 0.85 * storm),
                      1.35 * wind.x + 0.55 * wind.y, dx, dy, minimum_width, geometry);
    let b = wave_band(p, vec3<f32>(1.0, 2.0, 1.0) / 1024.0, clock, 135.0, 2.9 * (1.0 + 0.55 * storm),
                      2.1 * wind.y - 0.60 * wind.x, dx, dy, minimum_width, geometry);
    let c = wave_band(p, vec3<f32>(3.0, -2.0, 1.0) / 1024.0, clock, 167.0, 2.4 * (1.0 + 0.68 * storm),
                      -1.4 * wind.x + 0.8 * wind.y, dx, dy, minimum_width, geometry);
    let d = wave_band(p, vec3<f32>(-1.0, 1.0, 2.0) / 512.0, clock, 191.0, 1.2 * (1.0 + 0.63 * storm),
                      1.8 * wind.y + 1.25 * wind.x, dx, dy, minimum_width, geometry);
    let e = wave_band(p, vec3<f32>(2.0, -3.0, -1.0) / 256.0, clock, 335.0, 0.70 * (1.0 + 0.75 * storm),
                      -2.4 * wind.y - 0.75 * wind.x, dx, dy, minimum_width, geometry);
    let f = wave_band(p, vec3<f32>(3.0, 1.0, 2.0) / 128.0, clock, 473.0, 0.28 * (1.0 + 0.85 * storm),
                      1.25 * wind.x + 1.15 * wind.y, dx, dy, minimum_width, geometry);
    let envelope = 0.80 + 0.18 * wind.x * wind.y;
    var result = Swell((a.height + b.height + c.height + d.height + e.height + f.height) * envelope,
                       (a.slope + b.slope + c.slope + d.slope + e.slope + f.slope) * envelope,
                       d.crest_signal * 0.39 + e.crest_signal + f.crest_signal * 0.76);
    if storm > 0.0 {
        let storm_swell = wave_band(p, vec3<f32>(3.0, -1.0, 2.0) / 4096.0, clock, 84.0, 6.2 * storm,
                                    -0.6 + 0.85 * wind.x, dx, dy, minimum_width, geometry);
        result.height += storm_swell.height;
        result.slope += storm_swell.slope;
    }
    return result;
}

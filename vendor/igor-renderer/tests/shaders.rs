#[test]
fn validate_shaders() {
    for (name,source) in [
        ("legacy",include_str!("../src/trace.wgsl")),
        ("packet",concat!(include_str!("../src/trace.wgsl"),"\n",include_str!("../src/packet.wgsl"))),
        ("present",include_str!("../src/present.wgsl")),
        ("direct splat",include_str!("../src/splat.wgsl")),
        ("compute splat",include_str!("../src/compute_splat.wgsl")),
        ("compute present",include_str!("../src/compute_present.wgsl")),
        ("gpu lod",include_str!("../src/gpu_lod.wgsl")),
        ("temporal",include_str!("../src/temporal.wgsl")),
    ] {
        let module=naga::front::wgsl::parse_str(source).unwrap_or_else(|e|panic!("{name}: {}",e.emit_to_string(source)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(),naga::valid::Capabilities::all())
            .validate(&module).unwrap_or_else(|e|panic!("{name}: {e:?}"));
    }
}

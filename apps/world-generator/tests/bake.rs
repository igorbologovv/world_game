use std::{fs, path::PathBuf, process::Command};

use world_core::planet::{MAX_HEIGHT, MIN_HEIGHT, RADIUS};

struct OutputDirectory(PathBuf);

impl Drop for OutputDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn generator_bakes_connected_water_in_its_working_directory() {
    let path = std::env::temp_dir().join(format!("world-generator-bake-{}", std::process::id()));
    fs::create_dir(&path).unwrap();
    let output_dir = OutputDirectory(path);
    let output = Command::new(env!("CARGO_BIN_EXE_generate_world"))
        .current_dir(&output_dir.0)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "generator failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let assets = output_dir.0.join("assets");
    let check = |relative: &str| {
        let path = assets.join(relative);
        assert!(
            fs::metadata(&path).is_ok_and(|file| file.len() > 0),
            "missing or empty {}",
            path.display()
        );
    };
    for face in 0..6 {
        check(&format!("terrains/spherical/source/height/face{face}.tif"));
        check(&format!("world/ocean/face{face}.png"));
        check(&format!("world/basins/face{face}.png"));
    }
    check("world/ocean_atlas.png");
    check("world/river_atlas.png");
    let bytes = fs::read(assets.join("world/river_surface.bin")).expect("missing river output");
    let rivers = bytes
        .strip_prefix(b"WLWATER1")
        .expect("missing baked river header");
    assert!(!rivers.is_empty() && rivers.len() % (30 * 3 * 4) == 0);
    let radius = |point: &[u8]| -> f64 {
        point
            .chunks_exact(4)
            .map(|value| f64::from(f32::from_le_bytes(value.try_into().unwrap())).powi(2))
            .sum::<f64>()
            .sqrt()
    };
    for reach in rivers.chunks_exact(30 * 12) {
        let start = radius(&reach[6 * 12..7 * 12]);
        let end = (radius(&reach[12..24]) + radius(&reach[5 * 12..6 * 12])) * 0.5;
        let altitude_range = (RADIUS + MIN_HEIGHT)..=(RADIUS + MAX_HEIGHT);
        assert!(altitude_range.contains(&start));
        assert!(altitude_range.contains(&end));
        // Both radii came from f32 coordinates in the baked format.
        assert!(
            start + RADIUS * f64::from(f32::EPSILON) >= end,
            "uphill river reach"
        );
    }
    let lake_mesh = assets.join("world/lake_surface.bin");
    let bytes = fs::read(&lake_mesh).expect("missing inland-lake surface output");
    let vertices = bytes
        .strip_prefix(b"WLWATER1")
        .expect("missing baked lake header");
    // One lake tile consists of two outward-facing triangles.
    assert!(!vertices.is_empty() && vertices.len() % (6 * 3 * 4) == 0);
}

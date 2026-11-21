use std::{fs, path::PathBuf, process::Command};

struct OutputDirectory(PathBuf);

impl Drop for OutputDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn generator_bakes_legacy_assets_in_its_working_directory() {
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
    check("world/river_surface.bin");
    let lake_mesh = assets.join("world/lake_surface.bin");
    let bytes = fs::read(&lake_mesh).expect("missing inland-lake surface output");
    // One lake tile is two triangles, six unindexed vertices.
    assert_eq!(bytes.len() % (6 * 3 * 4), 0, "invalid lake surface records");
}

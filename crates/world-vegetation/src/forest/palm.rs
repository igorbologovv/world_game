//! Fallback geometry for coconut palm sites while their authored mesh loads.

use bevy::prelude::*;

use super::{tree_height, MeshBuilder, Species};

pub(super) fn procedural(lod: usize) -> (MeshBuilder, MeshBuilder) {
    let height = tree_height(Species::Palm, 0);
    let (bark, green) = Species::Palm.colors();
    let bark = Vec3::from_array(bark).extend(1.0).to_array();
    let green = Vec3::from_array(green).extend(1.0).to_array();
    let crown = Vec3::new(-1.15, height * 0.78, 0.0);
    let mut trunk = MeshBuilder::default();
    let rings = [12, 8, 4][lod];
    let path: Vec<_> = (0..=rings)
        .map(|ring| {
            let t = ring as f32 / rings as f32;
            (
                Vec3::new(crown.x * t * t, crown.y * t, 0.0),
                0.44 - 0.23 * t,
            )
        })
        .collect();
    trunk.tube_path(&path, [9, 6, 4][lod], bark);

    let mut leaves = MeshBuilder::default();
    for i in 0..14 {
        let a = i as f32 * 2.399_963;
        let (sin, cos) = a.sin_cos();
        let radial = Vec3::new(cos, 0.0, sin);
        let lift = (i % 3) as f32 * 0.38 - 0.34;
        let length = height * (0.44 - lift * 0.15);
        let direction = radial + Vec3::Y * lift;
        leaves.leaf(
            crown + direction * (length * 0.39),
            direction,
            length,
            length * 0.31,
            green,
        );
    }
    (trunk, leaves)
}

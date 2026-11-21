use std::path::Path;

use bevy::{asset::AssetPlugin, prelude::*};
use world_core::runtime::{WorldAssets, WorldSpacePlugin};
use world_diagnostics::FrameStatsPlugin;
use world_player::PlayerPlugin;
use world_regions::{Region, PALM_COAST_REGION, STARTER_REGION};
use world_terrain::TerrainPlugin;
use world_vegetation::ProceduralForestPlugin;
use world_water::WaterPlugin;

const WORKSPACE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

fn main() {
    let Some(region) = spawn_region(std::env::args().skip(1)) else {
        eprintln!("Expected an optional --spawn palm-coast or --spawn starter-river.");
        std::process::exit(2);
    };
    let assets = WorldAssets::resolve(Path::new(WORKSPACE_ROOT));
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .build()
            .disable::<TransformPlugin>()
            .set(asset_plugin(&assets)),
    );
    install_world(&mut app, assets, region);
    app.run();
}

fn spawn_region(mut args: impl Iterator<Item = String>) -> Option<Region> {
    match (args.next().as_deref(), args.next().as_deref(), args.next()) {
        (None, None, None) => Some(PALM_COAST_REGION),
        (Some("--spawn"), Some(id), None) => [PALM_COAST_REGION, STARTER_REGION]
            .into_iter()
            .find(|region| region.id.0 == id),
        _ => None,
    }
}

fn asset_plugin(assets: &WorldAssets) -> AssetPlugin {
    AssetPlugin {
        file_path: assets.directory().to_string_lossy().into_owned(),
        ..default()
    }
}

fn install_world(app: &mut App, assets: WorldAssets, region: Region) {
    app.add_plugins((
        WorldSpacePlugin::new(assets),
        PlayerPlugin::new(region.spawn),
        TerrainPlugin,
        WaterPlugin,
        ProceduralForestPlugin,
        FrameStatsPlugin,
    ));
}

#[cfg(test)]
mod tests;

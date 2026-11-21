mod starter_scene;

use std::path::Path;

use bevy::{asset::AssetPlugin, prelude::*};
use world_core::runtime::{WorldAssets, WorldReady, WorldSpacePlugin};
use world_diagnostics::FrameStatsPlugin;
use world_player::PlayerPlugin;
use world_regions::STARTER_REGION;
use world_terrain::TerrainPlugin;
use world_vegetation::ProceduralForestPlugin;
use world_water::WaterPlugin;

const WORKSPACE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

fn main() {
    let assets = WorldAssets::resolve(Path::new(WORKSPACE_ROOT));
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .build()
            .disable::<TransformPlugin>()
            .set(asset_plugin(&assets)),
    );
    install_world(&mut app, assets);
    app.run();
}

fn asset_plugin(assets: &WorldAssets) -> AssetPlugin {
    AssetPlugin {
        file_path: assets.directory().to_string_lossy().into_owned(),
        ..default()
    }
}

fn install_world(app: &mut App, assets: WorldAssets) {
    app.add_plugins((
        WorldSpacePlugin::new(assets),
        PlayerPlugin::new(STARTER_REGION.spawn),
        TerrainPlugin,
        WaterPlugin,
        ProceduralForestPlugin,
        FrameStatsPlugin,
    ))
    .add_systems(Startup, starter_scene::spawn.after(WorldReady));
}

#[cfg(test)]
mod tests;

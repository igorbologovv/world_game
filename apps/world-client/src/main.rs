mod palm_guide;

use std::path::Path;

use bevy::{asset::AssetPlugin, prelude::*};
use world_core::{coordinates::terrain_aligned_direction, runtime::{WorldAssets, WorldSpacePlugin}};
use world_diagnostics::FrameStatsPlugin;
use world_player::PlayerPlugin;
use world_regions::{Region, PALM_COAST_REGION, STARTER_REGION};
use world_terrain::TerrainPlugin;
use world_vegetation::{ForestClearing, ProceduralForestPlugin};
use world_water::WaterPlugin;

const PALM_MESH: &str = "models/vegetation/coconut_palm/tree_lod0.glb";
const WORKSPACE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

fn main() {
    let Some(region) = spawn_region(std::env::args().skip(1)) else {
        eprintln!("Expected an optional --spawn palm-coast or --spawn starter-river.");
        std::process::exit(2);
    };
    let location = if region.id == PALM_COAST_REGION.id {
        "Palm Coast"
    } else {
        "Starter River"
    };
    let workspace = Path::new(WORKSPACE_ROOT);
    let assets = prefer_checkout_palm_assets(
        WorldAssets::resolve(workspace),
        &workspace.join("assets"),
        std::env::var_os("BEVY_ASSET_ROOT").is_some(),
    );
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .build()
            .disable::<TransformPlugin>()
            .set(asset_plugin(&assets))
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: format!("Planetary World | {location}"),
                    ..default()
                }),
                ..default()
            }),
    );
    if region.id == PALM_COAST_REGION.id && !assets.directory().join(PALM_MESH).is_file() {
        warn!(
            "Coconut palm model missing from {}",
            assets.directory().display()
        );
    }
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

fn prefer_checkout_palm_assets(
    assets: WorldAssets,
    checkout: &Path,
    explicit_override: bool,
) -> WorldAssets {
    // Launching this build from an older checkout can find its terrain assets
    // before finding this checkout's vegetation.
    if !explicit_override
        && !assets.directory().join(PALM_MESH).is_file()
        && checkout.join(PALM_MESH).is_file()
    {
        WorldAssets::new(checkout.to_path_buf())
    } else {
        assets
    }
}

fn asset_plugin(assets: &WorldAssets) -> AssetPlugin {
    AssetPlugin {
        file_path: assets.directory().to_string_lossy().into_owned(),
        ..default()
    }
}

fn install_world(app: &mut App, assets: WorldAssets, region: Region) {
    // The authored coastal spawn must open into a traversable clearing
    // rather than a dense wall of procedurally planted palms.
    // Clearing uses physical meters and does not affect the rest of Mars.
    if region.id == PALM_COAST_REGION.id {
        app.insert_resource(ForestClearing {
            direction: terrain_aligned_direction(region.spawn.direction),
            radius: 55.0,
        });
    }
    let mut player = PlayerPlugin::new(region.spawn);
    if region.id == PALM_COAST_REGION.id {
        player = player.with_first_person_view();
        palm_guide::install(app);
    }
    app.add_plugins((
        WorldSpacePlugin::new(assets),
        player,
        TerrainPlugin,
        WaterPlugin,
        ProceduralForestPlugin,
        FrameStatsPlugin,
    ));
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod performance;

//! Farm construction and management over the authoritative economy service.

use std::sync::{mpsc, Mutex};

use bevy::{
    input::mouse::{MouseScrollUnit, MouseWheel},
    math::DVec3,
    prelude::*,
};
use prost::Message;
use world_core::{
    construction::{ConstructionEdit, ConstructionGrid, CONSTRUCTION_BLEND_M},
    coordinates::terrain_aligned_direction,
    planet::RADIUS,
    runtime::{
        GridCell, GridTransform, PlanetRoot, ReferenceFrames, SettlementMode, WorldCamera,
        WorldMovement, WorldReady,
    },
    SurfaceHeights,
};
use world_regions::PALM_COAST_REGION;

mod pb {
    include!(concat!(env!("OUT_DIR"), "/moontrade.economy.v1.rs"));
}

const MAP: &str = "main_world";
const FARM: &str = "farm";
const ORIGIN_CELL: i32 = 500_000;

pub struct SettlementPlugin;

impl Plugin for SettlementPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Settlement::default())
            .insert_resource(Connection::new())
            .add_systems(Startup, setup.after(WorldReady))
            .add_systems(
                Update,
                (poll_network, controls).chain().after(WorldMovement),
            );
    }
}

fn grid() -> ConstructionGrid {
    ConstructionGrid {
        origin: terrain_aligned_direction(PALM_COAST_REGION.spawn.direction),
        origin_cell: ORIGIN_CELL,
    }
}

#[derive(Resource)]
struct Connection {
    send: Option<mpsc::Sender<Request>>,
    receive: Mutex<mpsc::Receiver<Reply>>,
}

enum Request {
    Snapshot,
    Quote(i32, i32),
    Place(i32, i32, String),
    Workers(String, i32),
    Upgrade(String),
}

enum Reply {
    Snapshot(pb::SnapshotResponse),
    Quote(i32, i32, pb::QuoteBuildingResponse),
    Error(String),
}

impl Connection {
    fn new() -> Self {
        let (send, requests) = mpsc::channel();
        let (replies, receive) = mpsc::channel();
        let token = std::env::var("EPIC_GAME_V_TOKEN")
            .ok()
            .filter(|value| !value.is_empty());
        if let Some(token) = token {
            std::thread::spawn(move || {
                let client = reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_secs(15))
                    .build()
                    .expect("HTTP client configuration is valid");
                let base = std::env::var("EPIC_GAME_V_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:3011".into());
                while let Ok(request) = requests.recv() {
                    let result = match request {
                        Request::Snapshot => post::<_, pb::SnapshotResponse>(
                            &client,
                            &base,
                            &token,
                            "snapshot",
                            pb::SnapshotRequest { section_mask: 0 },
                        )
                        .map(Reply::Snapshot),
                        Request::Quote(x, y) => post::<_, pb::QuoteBuildingResponse>(
                            &client,
                            &base,
                            &token,
                            "buildings/quote",
                            pb::QuoteBuildingRequest {
                                map_code: MAP.into(),
                                building_type_code: FARM.into(),
                                tile_x: x,
                                tile_y: y,
                                tile_z: 0,
                            },
                        )
                        .map(|quote| Reply::Quote(x, y, quote)),
                        Request::Place(x, y, expected_total_cost) => {
                            post::<_, pb::PlaceBuildingResponse>(
                                &client,
                                &base,
                                &token,
                                "buildings/place",
                                pb::PlaceBuildingRequest {
                                    map_code: MAP.into(),
                                    building_type_code: FARM.into(),
                                    tile_x: x,
                                    tile_y: y,
                                    tile_z: 0,
                                    expected_total_cost,
                                },
                            )
                            .and_then(|_| {
                                post::<_, pb::SnapshotResponse>(
                                    &client,
                                    &base,
                                    &token,
                                    "snapshot",
                                    pb::SnapshotRequest { section_mask: 0 },
                                )
                            })
                            .map(Reply::Snapshot)
                        }
                        Request::Workers(building_id, assigned_workers) => {
                            post::<_, pb::SetWorkersResponse>(
                                &client,
                                &base,
                                &token,
                                "buildings/set-workers",
                                pb::SetWorkersRequest {
                                    building_id,
                                    assigned_workers,
                                },
                            )
                            .and_then(|_| {
                                post::<_, pb::SnapshotResponse>(
                                    &client,
                                    &base,
                                    &token,
                                    "snapshot",
                                    pb::SnapshotRequest { section_mask: 0 },
                                )
                            })
                            .map(Reply::Snapshot)
                        }
                        Request::Upgrade(building_id) => post::<_, pb::UpgradeBuildingResponse>(
                            &client,
                            &base,
                            &token,
                            "buildings/upgrade",
                            pb::UpgradeBuildingRequest { building_id },
                        )
                        .and_then(|_| {
                            post::<_, pb::SnapshotResponse>(
                                &client,
                                &base,
                                &token,
                                "snapshot",
                                pb::SnapshotRequest { section_mask: 0 },
                            )
                        })
                        .map(Reply::Snapshot),
                    };
                    let _ = replies.send(result.unwrap_or_else(Reply::Error));
                }
            });
            Self {
                send: Some(send),
                receive: Mutex::new(receive),
            }
        } else {
            Self {
                send: None,
                receive: Mutex::new(receive),
            }
        }
    }

    fn request(&self, request: Request) {
        if let Some(send) = &self.send {
            let _ = send.send(request);
        }
    }
}

fn post<Q: Message, R: Message + Default>(
    client: &reqwest::blocking::Client,
    base: &str,
    token: &str,
    path: &str,
    request: Q,
) -> Result<R, String> {
    let response = client
        .post(format!("{base}/economy/v1/{path}"))
        .bearer_auth(token)
        .header("content-type", "application/x-protobuf")
        .body(request.encode_to_vec())
        .send()
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.bytes().map_err(|error| error.to_string())?;
    if !status.is_success() {
        let detail = pb::ErrorResponse::decode(body)
            .map(|error| error.message)
            .unwrap_or_else(|_| status.to_string());
        return Err(detail);
    }
    R::decode(body).map_err(|error| error.to_string())
}

#[derive(Resource)]
struct Settlement {
    x: i32,
    y: i32,
    distance: f64,
    yaw: f64,
    quote: Option<(i32, i32, pb::QuoteBuildingResponse)>,
    buildings: Vec<pb::Building>,
    selected: usize,
    balance: String,
    status: String,
    previous_camera: Option<(GridCell, Transform, WorldCamera)>,
}

impl Default for Settlement {
    fn default() -> Self {
        Self {
            x: ORIGIN_CELL,
            y: ORIGIN_CELL,
            distance: 38.0,
            yaw: 0.0,
            quote: None,
            buildings: Vec::new(),
            selected: 0,
            balance: String::new(),
            status: "Press B to manage the settlement".into(),
            previous_camera: None,
        }
    }
}

#[derive(Component)]
struct Preview;
#[derive(Component)]
struct PlacedFarm;
#[derive(Component)]
struct SettlementText;

#[derive(Resource)]
struct FarmAssets {
    floor: Handle<Mesh>,
    barn: Handle<Mesh>,
    roof: Handle<Mesh>,
    plot: Handle<Mesh>,
    soil: Handle<StandardMaterial>,
    wall: Handle<StandardMaterial>,
    roof_mat: Handle<StandardMaterial>,
    crop: Handle<StandardMaterial>,
    preview: Handle<StandardMaterial>,
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    root: Res<PlanetRoot>,
) {
    let assets = FarmAssets {
        floor: meshes.add(Cuboid::new(5.0, 0.25, 5.0)),
        barn: meshes.add(Cuboid::new(2.0, 1.7, 1.8)),
        roof: meshes.add(Cuboid::new(2.4, 0.18, 2.2)),
        plot: meshes.add(Cuboid::new(0.65, 0.15, 2.0)),
        soil: materials.add(Color::srgb(0.33, 0.26, 0.17)),
        wall: materials.add(Color::srgb(0.68, 0.47, 0.28)),
        roof_mat: materials.add(Color::srgb(0.48, 0.13, 0.09)),
        crop: materials.add(Color::srgb(0.18, 0.52, 0.17)),
        preview: materials.add(StandardMaterial {
            base_color: Color::srgba(0.2, 0.88, 0.42, 0.45),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        }),
    };
    root.spawn(
        &mut commands,
        (
            Preview,
            Mesh3d(assets.floor.clone()),
            MeshMaterial3d(assets.preview.clone()),
            Visibility::Hidden,
        ),
    ).with_children(|farm| {
        farm.spawn((Mesh3d(assets.barn.clone()), MeshMaterial3d(assets.preview.clone()),
            Transform::from_xyz(-1.1, 0.95, -0.8)));
        farm.spawn((Mesh3d(assets.roof.clone()), MeshMaterial3d(assets.preview.clone()),
            Transform::from_xyz(-1.1, 1.9, -0.8)));
        for x in [0.5, 1.5] { for z in [-1.2, 1.2] {
            farm.spawn((Mesh3d(assets.plot.clone()), MeshMaterial3d(assets.preview.clone()),
                Transform::from_xyz(x, 0.20, z)));
        }}
    });
    commands.insert_resource(assets);
    commands.spawn((
        SettlementText,
        Text::new("Press B to manage the settlement"),
        TextFont {
            font_size: FontSize::Px(18.0),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(18.0),
            top: Val::Px(18.0),
            ..default()
        },
    ));
}

fn poll_network(
    mut commands: Commands,
    connection: Res<Connection>,
    mut state: ResMut<Settlement>,
    heights: Res<SurfaceHeights>,
    root: Res<PlanetRoot>,
    frames: ReferenceFrames,
    assets: Res<FarmAssets>,
    old: Query<Entity, With<PlacedFarm>>,
) {
    let Ok(receiver) = connection.receive.lock() else {
        return;
    };
    while let Ok(reply) = receiver.try_recv() {
        match reply {
            Reply::Error(error) => {
                state.status = error;
                state.quote = None;
            }
            Reply::Quote(x, y, quote) => {
                if x == state.x && y == state.y {
                    state.status = format!(
                        "Farm: {} KE-TSE (base {}, earthwork {}, clearing {})",
                        quote.total_cost,
                        quote.base_cost,
                        quote.earthwork_cost,
                        quote.vegetation_cost
                    );
                    state.quote = Some((x, y, quote));
                }
            }
            Reply::Snapshot(snapshot) => {
                state.balance = snapshot.ke_tse_balance;
                state.buildings = snapshot.buildings;
                state.selected = state.selected.min(state.buildings.len().saturating_sub(1));
                state.status = format!(
                    "{} farms · {} KE-TSE · {} free cells",
                    state.buildings.len(),
                    state.balance,
                    snapshot.build_budget.map_or(0, |budget| budget.free_cells)
                );
                let edits = state
                    .buildings
                    .iter()
                    .filter(|building| building.planet_x != 0.0)
                    .map(|building| ConstructionEdit {
                        center: DVec3::new(building.planet_x, building.planet_y, building.planet_z),
                        half_width: f64::from(building.footprint_width) / 2.0,
                        half_height: f64::from(building.footprint_height) / 2.0,
                        blend: CONSTRUCTION_BLEND_M,
                        elevation: building.pad_elevation,
                    })
                    .collect();
                heights.set_construction_edits(edits);
                for entity in &old {
                    commands.entity(entity).despawn();
                }
                let Some(frame) = frames.parent_frame(root.entity()) else {
                    continue;
                };
                for building in &state.buildings {
                    if building.building_type_code != FARM || building.planet_x == 0.0 {
                        continue;
                    }
                    let center =
                        DVec3::new(building.planet_x, building.planet_y, building.planet_z);
                    let (cell, local) = frame.translation_to_grid(center);
                    let rotation = Quat::from_rotation_arc(Vec3::Y, center.normalize().as_vec3());
                    spawn_farm(
                        &mut commands,
                        &root,
                        &assets,
                        cell,
                        Transform::from_translation(local).with_rotation(rotation),
                    );
                }
            }
        }
    }
}

fn spawn_farm(
    commands: &mut Commands,
    root: &PlanetRoot,
    assets: &FarmAssets,
    cell: GridCell,
    transform: Transform,
) {
    root.spawn(commands, (PlacedFarm, cell, transform))
        .with_children(|farm| {
            farm.spawn((
                Mesh3d(assets.floor.clone()),
                MeshMaterial3d(assets.soil.clone()),
            ));
            farm.spawn((
                Mesh3d(assets.barn.clone()),
                MeshMaterial3d(assets.wall.clone()),
                Transform::from_xyz(-1.1, 0.95, -0.8),
            ));
            farm.spawn((
                Mesh3d(assets.roof.clone()),
                MeshMaterial3d(assets.roof_mat.clone()),
                Transform::from_xyz(-1.1, 1.9, -0.8),
            ));
            for x in [0.5, 1.5] {
                for z in [-1.2, 1.2] {
                    farm.spawn((
                        Mesh3d(assets.plot.clone()),
                        MeshMaterial3d(assets.crop.clone()),
                        Transform::from_xyz(x, 0.20, z),
                    ));
                }
            }
        });
}

fn controls(
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: MessageReader<MouseWheel>,
    mut mode: ResMut<SettlementMode>,
    mut state: ResMut<Settlement>,
    connection: Res<Connection>,
    heights: Res<SurfaceHeights>,
    frames: ReferenceFrames,
    mut camera: Query<(Entity, GridTransform, &mut WorldCamera), Without<Preview>>,
    mut preview: Query<(GridTransform, &mut Visibility), (With<Preview>, Without<WorldCamera>)>,
    mut labels: Query<&mut Text, With<SettlementText>>,
) {
    let Ok((camera_entity, mut camera, mut view)) = camera.single_mut() else {
        return;
    };
    if keys.just_pressed(KeyCode::KeyB) {
        mode.active = !mode.active;
        if mode.active {
            state.previous_camera = Some((*camera.cell, *camera.transform, *view));
            *view = WorldCamera::Settlement;
            connection.request(Request::Snapshot);
            connection.request(Request::Quote(state.x, state.y));
            if connection.send.is_none() {
                state.status = "Set EPIC_GAME_V_TOKEN to enable authenticated construction".into();
            }
        } else if let Some((cell, transform, previous)) = state.previous_camera.take() {
            *camera.cell = cell;
            *camera.transform = transform;
            *view = previous;
        }
    }
    if !mode.active {
        if let Ok((_, mut visibility)) = preview.single_mut() {
            *visibility = Visibility::Hidden;
        }
        if let Ok(mut text) = labels.single_mut() {
            **text = "Press B to manage the settlement".into();
        }
        return;
    }
    let dx = i32::from(keys.just_pressed(KeyCode::ArrowRight))
        - i32::from(keys.just_pressed(KeyCode::ArrowLeft));
    let dy = i32::from(keys.just_pressed(KeyCode::ArrowUp))
        - i32::from(keys.just_pressed(KeyCode::ArrowDown));
    if dx != 0 || dy != 0 {
        state.x = (state.x + dx).clamp(0, 999_995);
        state.y = (state.y + dy).clamp(0, 999_995);
        state.quote = None;
        state.status = "Calculating construction cost…".into();
        connection.request(Request::Quote(state.x, state.y));
    }
    for event in wheel.read() {
        let amount = match event.unit {
            MouseScrollUnit::Line => event.y as f64,
            MouseScrollUnit::Pixel => event.y as f64 / 60.0,
        };
        state.distance = (state.distance * (1.0 - amount * 0.08)).clamp(12.0, 180.0);
    }
    if keys.pressed(KeyCode::KeyQ) {
        state.yaw -= 0.025;
    }
    if keys.pressed(KeyCode::KeyE) {
        state.yaw += 0.025;
    }
    if keys.just_pressed(KeyCode::Enter) {
        if let Some((x, y, quote)) = &state.quote {
            connection.request(Request::Place(*x, *y, quote.total_cost.clone()));
            state.quote = None;
            state.status = "Placing farm…".into();
        }
    }
    if keys.just_pressed(KeyCode::Tab) && !state.buildings.is_empty() {
        state.selected = (state.selected + 1) % state.buildings.len();
    }
    if let Some(building) = state.buildings.get(state.selected) {
        if keys.just_pressed(KeyCode::KeyU) {
            connection.request(Request::Upgrade(building.id.clone()));
        }
        for (key, amount) in [
            (KeyCode::Digit0, 0),
            (KeyCode::Digit1, 1),
            (KeyCode::Digit2, 2),
            (KeyCode::Digit3, 3),
            (KeyCode::Digit4, 4),
            (KeyCode::Digit5, 5),
            (KeyCode::Digit6, 6),
            (KeyCode::Digit7, 7),
            (KeyCode::Digit8, 8),
            (KeyCode::Digit9, 9),
        ] {
            if keys.just_pressed(key) {
                let amount = if key == KeyCode::Digit0
                    && (keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight))
                {
                    10
                } else {
                    amount
                };
                connection.request(Request::Workers(building.id.clone(), amount));
            }
        }
    }
    if keys.just_pressed(KeyCode::KeyG) {
        connection.request(Request::Snapshot);
    }
    let Some(frame) = frames.parent_frame(camera_entity) else {
        return;
    };
    let origin = grid();
    let direction = origin.direction(state.x, state.y);
    let elevation = state
        .quote
        .as_ref()
        .map(|(_, _, quote)| quote.pad_elevation)
        .or_else(|| heights.sample(direction))
        .unwrap_or(0.0);
    let center = origin.position_at(
        f64::from(state.x) + 2.5,
        f64::from(state.y) + 2.5,
        elevation,
    );
    let up = center.normalize();
    let east = DVec3::Y.cross(up).normalize();
    let north = up.cross(east);
    let (sin, cos) = state.yaw.sin_cos();
    let eye = center + up * state.distance + (east * sin + north * cos) * state.distance * 0.55;
    let (cell, local) = frame.translation_to_grid(eye);
    *camera.cell = cell;
    *camera.transform = Transform::from_translation(local)
        .looking_at(local + (center - eye).as_vec3(), up.as_vec3());
    if let Ok((mut visual, mut visibility)) = preview.single_mut() {
        let (cell, local) = frame.translation_to_grid(center + up * 0.18);
        *visual.cell = cell;
        *visual.transform = Transform::from_translation(local)
            .with_rotation(Quat::from_rotation_arc(Vec3::Y, up.as_vec3()));
        *visibility = Visibility::Inherited;
    }
    if let Ok(mut text) = labels.single_mut() {
        let selected = state
            .buildings
            .get(state.selected)
            .map(|building| {
                format!(
                    "Selected farm: level {}, workers {} | Tab to change",
                    building.level, building.assigned_workers
                )
            })
            .unwrap_or_default();
        **text = format!("Farm settlement  ·  {} KE-TSE\n{}\n{}\nArrows: move  Wheel: zoom  Q/E: rotate  Enter: build\n0–9: workers  Shift+0: 10  U: upgrade  G: refresh  B: exit",
            state.balance, state.status, selected);
    }
    let _ = RADIUS;
}

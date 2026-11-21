# Epic_Game_V

Standalone Rust economic game backend extracted from `moontrade-server/economy-service`.
It includes building placement and upgrades, worker assignment, population growth,
resource balances, hourly production/consumption, and its own PostgreSQL schema and
seed rules. No other MoonTrade repository is needed to build or run it.

The backend lives in `Epic_Game_V/` inside the world-client repository. The Bevy
workspace is rooted in the parent directory and the client lives in
`apps/world-client/`. The backend keeps its own Cargo manifest and lockfile.
Run backend commands from this directory.

## Start

Requires Docker with Compose v2.

```bash
cd ~/world-client/Epic_Game_V
cp -n .env.example .env
# Set FIREBASE_PROJECT_ID in .env to the Firebase project used by your client.
./run.sh
curl http://127.0.0.1:3011/health
curl -f http://127.0.0.1:3011/internal/health/db
```

The game is on port **3011** and PostgreSQL on **15433**, bound to localhost.
`GAME_PORT` and `POSTGRES_PORT` in `.env` can change these ports. Update
`DATABASE_URL` and `ECONOMY_SERVICE_ADDR` too when running Cargo on the host.
The database initializes automatically on first startup and persists in the
project's `epic-game-v_postgres-data` Docker volume. `docker compose down`
stops the project while retaining that data.

```bash
docker compose logs -f game db
docker compose down
```

## Authentication and players

Gameplay uses the original Firebase ID tokens in `Authorization: Bearer <token>`.
The first authenticated gameplay request creates the player's account and economy
rows directly in this database, with the original 2,000 MT starting balance,
10 population, and 100 building cells. Existing balances are never reset by
registration. Firebase remains the external identity provider; the trading server
is no longer needed for registration.

The seed data includes `main_world`, the `farm` building, food, metal, tool,
production rules, and upgrade costs. A farm costs 500 MT and occupies 5 by 5 cells.
This directory contains the backend API; the world client is in the parent directory.
Database scripts include schema and game rules, not an export of existing player
saves. Original database files and volumes are not changed.

## API

All gameplay endpoints accept and return binary `application/x-protobuf` messages.
The contract is in `proto/economy/v1/economy.proto`; its original package and HTTP
paths are retained for client compatibility.

| Method | Path | Request → response |
| --- | --- | --- |
| GET | `/health` | Plain text health |
| GET | `/internal/health/db` | Database connectivity status |
| POST | `/economy/v1/snapshot` | `SnapshotRequest` → `SnapshotResponse` |
| POST | `/economy/v1/settle` | `SettleRequest` → `SettleResponse` |
| POST | `/economy/v1/population/increase` | `IncreasePopulationRequest` → `IncreasePopulationResponse` |
| POST | `/economy/v1/buildings/place` | `PlaceBuildingRequest` → `PlaceBuildingResponse` |
| POST | `/economy/v1/buildings/set-workers` | `SetWorkersRequest` → `SetWorkersResponse` |
| POST | `/economy/v1/buildings/upgrade` | `UpgradeBuildingRequest` → `UpgradeBuildingResponse` |

## Run with Cargo

Requires Rust 1.96 or newer, `protoc`, OpenSSL development libraries, and pkg-config.

```bash
docker compose up -d db
cargo run --locked
```

Cargo reads `.env` from the project directory. For a separately managed, empty
PostgreSQL database, initialize it with `DATABASE_URL=... ./scripts/init-db.sh`.
The database user needs permission to create `pgcrypto` and `btree_gist` extensions.
SQL files run in filename order; they retain the source repository's numbering.

The database integration test exercises new-player provisioning, placement,
overlap rejection with currency rollback, worker limits, hourly production,
upgrades, and population growth. Use an initialized, disposable database:

```bash
cargo test --locked
TEST_DATABASE_URL=postgres://.../epic_game_test cargo test --locked -- --ignored
```

## Back up the database

```bash
mkdir -p db/backups
docker compose exec -T db sh -c 'pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" -Fc' > db/backups/epic_game.dump
```

Backups and local credentials are excluded from Git and Docker build contexts.

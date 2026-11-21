#!/usr/bin/env bash
# Isolate Bevy 0.20 migration from the working Bevy 0.14 terrain checkout.
# Run with: bash tools/setup_bevy20_terrain.sh
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
terrain_source="${BEVY_TERRAIN_SOURCE:-$HOME/bevy_terrain}"
terrain_target="${BEVY_TERRAIN_TARGET:-$HOME/bevy_terrain-020}"

if ! git -C "$terrain_source" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "Expected Git checkout at $terrain_source" >&2
  exit 1
fi
if [[ "$project_root" == "$terrain_source" || "$project_root" == "$terrain_target" ]]; then
  echo "Source, target and game worktrees must be distinct" >&2
  exit 1
fi

if git -C "$terrain_target" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "Using existing isolated checkout: $terrain_target"
elif [[ -e "$terrain_target" ]]; then
  echo "Target path already exists but is not a Git worktree: $terrain_target" >&2
  exit 1
else
  branch="migration/bevy-020"
  if git -C "$terrain_source" show-ref --verify --quiet "refs/heads/$branch"; then
    git -C "$terrain_source" worktree add "$terrain_target" "$branch"
  else
    git -C "$terrain_source" worktree add -b "$branch" "$terrain_target" HEAD
  fi
fi

# Change only the isolated terrain worktree and THIS Bevy 0.20 game worktree.
python3 - "$terrain_target/Cargo.toml" "$project_root/Cargo.toml" "$project_root" "$terrain_target" <<'PY'
from pathlib import Path
import os
import sys

terrain_manifest, game_manifest = map(Path, sys.argv[1:3])
root, terrain_root = map(Path, sys.argv[3:5])
terrain = terrain_manifest.read_text()
terrain_new = terrain.replace(
    'bevy = "0.14.0"',
    'bevy = "0.20.1"',
).replace(
    'big_space = { version = "0.7", optional = true }',
    'big_space = { version = "=0.14.0-rc.0", optional = true }',
)
if terrain_new == terrain and 'bevy = "0.20.1"' not in terrain:
    raise SystemExit("Unrecognized bevy_terrain dependency layout; no manifest changes")
if terrain_new != terrain:
    terrain_manifest.write_text(terrain_new)

game = game_manifest.read_text()
new_path = os.path.relpath(terrain_root, root)
game_new = game.replace(
    'bevy_terrain = { path = "../bevy_terrain",',
    f'bevy_terrain = {{ path = "{new_path}",',
)
if game_new == game and f'path = "{new_path}"' not in game:
    raise SystemExit("Unrecognized world_game bevy_terrain path")
if game_new != game:
    game_manifest.write_text(game_new)
print("Updated only:", terrain_manifest, "and", game_manifest)
PY

echo
echo "Original checkout untouched: $terrain_source"
echo "Migration checkout:         $terrain_target"
echo "Game checkout:              $project_root"
echo
echo "Both the terrain renderer and big_space changed APIs substantially."
echo "The next cargo check is EXPECTED to report source-level porting errors."
echo "Run:"
echo "  cd '$project_root'"
echo "  CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo check -p world-core"

# Windows setup (world-client)

These instructions apply to the Bevy `world-client` at the repository root, **not** the `Epic_Game_V` backend.

## Prerequisites

- Windows 10/11, PowerShell, Git.
- Rust installed using [rustup](https://rustup.rs/), with a Windows MSVC toolchain.
- Visual Studio C++ build tools and a Windows SDK for native Rust dependencies.
- A local copy of the `bevy_terrain` crate alongside this repository: the root `Cargo.toml` uses `bevy_terrain = { path = "../bevy_terrain", features = ["high_precision"] }`. It is not included in this repository.

Required folder layout:

```text
C:\Personal\
  world_game\
    Cargo.toml
    src\
  bevy_terrain\
    Cargo.toml
```

The actual parent folder can be different; the two sibling directories are what matters. Obtain the compatible `bevy_terrain` source from the project maintainer if you do not already have it.

## Build and run

From PowerShell inside `world_game`:

```powershell
.\scripts\windows-build.ps1
.\scripts\windows-build.ps1 -Run
```

The script checks that Cargo and the sibling crate exist, runs `cargo build --locked`, and optionally starts the client with `cargo run --locked --bin world-client`. The root Cargo manifest also sets `default-run = "world-client"`, so `cargo run --locked` starts the client by default. To run the world generator instead, use `cargo run --locked --bin generate_world`. Alternatively, run those Cargo commands manually from the repository root. Do not run them from `Epic_Game_V` when building the client.

If PowerShell blocks the script, run it for the current process with:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows-build.ps1
```

## Updating your Windows checkout

The Windows support branch now includes the `feature/river-water-shader` work: WASD movement, Shift sprint, procedural forest, and performance changes.

```powershell
git fetch origin
git switch feature/windows-support
git pull --ff-only
.\scripts\windows-build.ps1 -Run
```

If the local branch does not exist yet, run `git switch --track origin/feature/windows-support` instead of `git switch feature/windows-support`.

## Known upstream build blocker: missing `src/ocean.rs`

The merged `feature/river-water-shader` version declares `mod ocean;` in `src/main.rs`, but the source branch does **not** contain `src/ocean.rs` or `src/ocean/mod.rs`. As a result, Rust reports `error[E0583]` until the actual optimized ocean module is provided. This is not a Windows-only problem and was already present in the river branch.

Obtain the missing `src/ocean.rs` from the river branch's author, or commit a compatible implementation as a separate fix. Do not create an empty file: the code uses `ocean::OceanMaterial`, `ocean::load`, and `ocean::cull_patches`. The Windows launcher cannot fix missing application source code.

## Notes

- Bevy shaders and terrain assets are loaded relative to the asset directory; launch from the repository root (the script does this).
- Missing generated world data is a separate runtime/setup issue, not a Rust module-resolution error.
- The Windows launcher built an earlier version successfully in a user test, but this merged revision has not been validated. The missing ocean module currently prevents compilation.

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

## Local checkout and the missing `ocean` module

If you see `error[E0583]: file not found for module ocean` from `src/main.rs`, check that you are on an up-to-date branch:

```powershell
git fetch origin
git switch master
git pull --ff-only
git switch -c feature/windows-support --track origin/feature/windows-support
```

If `feature/windows-support` already exists locally, use `git switch feature/windows-support` and `git pull --ff-only` instead of creating it again. Preserve or commit local changes before switching branches.

The current upstream `master` defines `OceanMaterial` directly in `src/main.rs` and does **not** declare `mod ocean;`. An older or locally modified `main.rs` can cause that error; adding a blank `src/ocean.rs` is not a fix.

## Notes

- Bevy shaders and terrain assets are loaded relative to the asset directory; launch from the repository root (the script does this).
- Missing generated world data is a separate runtime/setup issue, not a Rust module-resolution error.
- This guide and script have not been validated on a Windows machine by CI.

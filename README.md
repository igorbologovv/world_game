# Planetary world

Bevy 0.20 client and offline generator for a planet with a 3,389.5 km reference radius.
The runtime adds meter-scale terrain relief to the continental heightmaps and
reprojects the shipped 100 km river and lake meshes to the current radius.

```sh
cargo run --locked -p world-client
cargo test --workspace
```

The client starts in first person at Palm Coast, facing a sunlit coconut palm
grove. The **Coconut palm** label points to the closest palm at the spawn.
Press **V** to open the map. To start at the original river region, run `cargo run --locked -p world-client -- --spawn starter-river`.

Scroll out in the map view and right-drag to rotate the globe. Hold **Ctrl**
and **left-click** a visible location to teleport the explorer there. The map
zooms in at the destination. Dry ground lands the explorer, and ocean or river
targets put the explorer in flight above the surface. A plain click starts
normal travel.

Use `cargo run` to launch Linux development builds with their shared Bevy
library. Give each worktree its own `CARGO_TARGET_DIR` if you set it manually.
The client uses assets packaged beside the executable or assets from the
checkout that built it. Set `BEVY_ASSET_ROOT` to override the asset source.

Keep `bevy_terrain` beside this checkout. Run
`cargo run --locked -p world-generator` to regenerate assets in the current
working directory. For an isolated bake, see the
[running and testing instructions](docs/ARCHITECTURE.md#running-and-testing).

[Architecture and crate ownership](docs/ARCHITECTURE.md) · [Agent development rules](AGENTS.md)

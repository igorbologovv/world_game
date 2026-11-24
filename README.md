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

## Forest canopy transition experiment

This branch retains the original rendering for comparison:

- **F9** cycles the existing forest-density and canopy-color diagnostics.
- **F10** compares the original forest-canopy shading with the improved
  multiscale optical-density and crown-shadow handoff. Improved is the default.
- **F11** cycles four canopy diagnostics: source weights (six local cascades from blue through cyan, green,
  yellow, orange, red; magenta: global cubemap), sampled
  coverage versus displayed opacity, crown lighting, and geometry-to-canopy
  handoff. Press again to return to the normal view.

Compare at a fixed camera position after all local layers and the globe atlas
have loaded. Toggle F10 with the same sun angle; measure full-frame GPU time.
Diagnostic displays color the **terrain canopy only**, not tree meshes. The
global atlas retains its normal mip streaming, and no artificial distance fog is
introduced. The extra diagnostics are not a substitute for actual in-game GPU
validation.

[Architecture and crate ownership](docs/ARCHITECTURE.md) · [Agent development rules](AGENTS.md)

# Planetary world

Bevy 0.20 client and offline generator for the spherical world.

```sh
cargo run --locked -p world-client
cargo test --workspace
```

The client starts at Palm Coast facing a coconut palm grove. To start at
the original river region, run `cargo run --locked -p world-client -- --spawn starter-river`.

Scroll out in the map view and right-drag to rotate the globe. Hold **Ctrl**
and **left-click** a visible location to teleport the explorer there. The map
zooms in at the destination. Dry ground lands the explorer, and ocean or river
targets put the explorer in flight above the surface. A plain click starts
normal travel.

Keep `bevy_terrain` beside this checkout. Run
`cargo run --locked -p world-generator` to regenerate assets in the current
working directory. For an isolated bake, see the
[running and testing instructions](docs/ARCHITECTURE.md#running-and-testing).

[Architecture and crate ownership](docs/ARCHITECTURE.md) · [Agent development rules](AGENTS.md)

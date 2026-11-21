# Planetary world

Bevy 0.14 client and offline generator for the spherical world.

```sh
cargo run --locked -p world-client
cargo test --workspace
```

Keep `bevy_terrain` beside this checkout. Run
`cargo run --locked -p world-generator` to regenerate assets in the current
working directory. For an isolated bake, see the
[running and testing instructions](docs/ARCHITECTURE.md#running-and-testing).

[Architecture and crate ownership](docs/ARCHITECTURE.md) · [Agent development rules](AGENTS.md)

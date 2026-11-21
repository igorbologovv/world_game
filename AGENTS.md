# Agent development rules

These rules apply to every agent working in this repository.

1. **Assign every agent a specific crate or subsystem.** Agree on a single owner and the files it covers before starting. Two agents must not edit the same subsystem concurrently. Use [the ownership table](docs/ARCHITECTURE.md#ownership) when the assignment also includes assets.
2. **Use separate Git worktrees and feature branches.** Start from an agreed integration commit and keep the primary working tree available to its owner. Place worktrees beside this checkout so the existing `../bevy_terrain` dependency still resolves. Observe any branch restriction in the current task.
3. **Keep changes within the assignment.** Call other subsystems through their documented public APIs. Request a change from the owning agent when a feature needs work in another crate. Keep implementation modules, render resources, and ECS systems private to their crate.
4. **Coordinate shared contract changes explicitly.** Before editing `world-core` or changing a public interface, agree with the affected owners on semantics, compatibility, and the order in which changes will land. The integration owner coordinates edits to `apps/world-client`, the workspace `Cargo.toml` and `Cargo.lock`, shared generated formats, and assets used by multiple subsystems. Workspace configuration changes require a reason tied to the task.
5. **Test and document each crate.** Keep tests with the owning crate. Document every public API and run package checks and tests before submitting changes. A shared contract or composition change also requires workspace checks and the relevant application integration test.
6. **Integrate through review.** Submit focused commits from the agent's feature branch. The integration owner reviews each change, resolves overlap with the affected owners, and reruns the workspace checks on the combined result. Never fix a conflict by overwriting another agent's work.

See [Architecture](docs/ARCHITECTURE.md) for the dependency rules, plugin phases, and validation commands.

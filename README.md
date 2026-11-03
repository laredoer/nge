# nge

A game engine written in Rust whose game view is a native component inside [NSH](https://nsh.dev): levels, HUDs and menus are declared in TSX, performance-sensitive logic is written in Rust, and edits to the TSX hot-reload without a restart.

Development and testing happen on macOS; iOS builds. Android and the web come later.

## Features

- 2D: batched sprites and atlases, camera (follow, bounds, shake), tilemaps, frame animation and tweens, particles, text
- 3D: meshes, PBR materials, directional and point lights, glTF
- 2D physics (rapier2d): rigid bodies, collisions, contact queries, fixed timestep with render interpolation
- Audio (kira): sound effects and music
- Input: touch, keyboard, gamepads
- Pause and time scale, asynchronous `asset:/` loading

The design and the acceptance notes for each feature are in [docs/](docs/) (in Chinese); start with [engine-design.md](docs/engine-design.md).

## Running the examples

You need Rust (the version is pinned in `rust-toolchain.toml`) and the nsh 0.3 command line. For the platformer:

```sh
cd examples/platformer
nsh dev --server-only App.tsx      # one terminal: the TSX dev server
cargo run --release -p platformer  # another terminal: the game
```

More examples are in [examples/](examples/): `physics` (physics and sound), `scene` (animation, camera, pause), `scene3d` (3D scene nodes), `dodge` (keyboard and gamepad) and others.

Tests: `cargo test --workspace`.

## Dependencies

The renderer, scene sync and platform layer are written here; underneath they use open-source crates such as wgpu, bevy_ecs (only its ECS), rapier2d, kira, glyphon, glam, gltf and objc2.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

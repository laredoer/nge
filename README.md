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

You need Rust (the version is pinned in `rust-toolchain.toml`) and the nsh 0.4 command line. For the platformer:

```sh
nsh run examples/platformer            # macOS: edits to App.tsx hot-reload, edits to Rust rebuild and restart
nsh run examples/platformer --target ios  # the iOS simulator
```

On an iPhone: `nsh package --target ios --sign <certificate> --profile <profile>` in the example's directory, then `tools/resign-ios.sh` (works around an nsh 0.4.0 signing issue, see [docs/native-components.md](docs/native-components.md)) and install the `.ipa`.

More examples are in [examples/](examples/): `physics` (physics and sound), `scene` (animation, camera, pause), `scene3d` (3D scene nodes), `dodge` (keyboard and gamepad) and others; each runs with `nsh run examples/<name>` (`--window-size 480x800` sets the window size).

`nsh run` builds each example in its own `target/` (5–20 GB each). To share one build directory between them, set `CARGO_TARGET_DIR`, e.g. `export CARGO_TARGET_DIR=$PWD/target/nsh-hosts`.

Tests: `cargo test --workspace`.

## Dependencies

The renderer, scene sync and platform layer are written here; underneath they use open-source crates such as wgpu, bevy_ecs (only its ECS), rapier2d, kira, glyphon, glam, gltf and objc2.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

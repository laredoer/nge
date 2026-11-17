# nge

A game engine written in Rust whose game view is a native component inside [NSH](https://nsh.dev): levels, HUDs and menus are declared in TSX, performance-sensitive logic is written in Rust, and edits to the TSX hot-reload without a restart.

Development and testing happen on macOS; iOS builds; the web (WebGPU) builds for `examples/sanguo`. Android comes later.

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
tools/nsh-web.sh package --target web -o out examples/sanguo  # a static site in out/sanguo
```

The web build needs the `wasm32-unknown-unknown` target, `wasm-bindgen-cli` 0.2.129 and Homebrew's `llvm@20`; serve `out/sanguo` with any static server and open it in a recent Chrome or Edge (see [docs/web.md](docs/web.md)).

On an iPhone: `nsh package --target ios --sign <certificate> --profile <profile>` in the example's directory, then `tools/resign-ios.sh` (works around an nsh 0.4.0 signing issue, see [docs/native-components.md](docs/native-components.md)) and install the `.ipa`.

More examples are in [examples/](examples/): `physics` (physics and sound), `scene` (animation, camera, pause), `scene3d` (3D scene nodes), `dodge` (keyboard and gamepad) and others; each runs with `nsh run examples/<name>` (`--window-size 480x800` sets the window size).

`nsh run` builds each example as its own Cargo workspace, and with the stock Cargo each gets its own `target/` (5–20 GB each). [huacnlee/cargo](https://huacnlee.github.io/cargo/) shares compiled dependencies between them and keeps the build output under a size limit.

Tests: `cargo test --workspace`.

## Dependencies

The renderer, scene sync and platform layer are written here; underneath they use open-source crates such as wgpu, bevy_ecs (only its ECS), rapier2d, kira, glyphon, glam, gltf and objc2.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

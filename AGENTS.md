# Dark Engine — working agreement

Internal Dark Wagon Studio engine (Rust). Top-down 2D pixel action RPG, Windows + Linux.

## Where truth lives

- **`docs/PLAN.md`** is locked truth for scope, stack, architecture rules and milestones.
  Read it before working in an area it covers. Change it deliberately, in the same change as the code.

## Rules

1. Simulation crates never depend on presentation (`winit`, `wgpu`, FMOD, Spine, egui).
   `tools/check-sim-deps.sh` enforces this; new sim crates go into its list.
2. No separate single-player path. Everything goes through `dark_net::Host`.
3. Gameplay runs in `FixedUpdate` and never reads frame delta.
4. Players and NPCs share one actor model; only the controller differs.
5. Dependency versions are pinned once, in the root `[workspace.dependencies]`.

## Before calling work done

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
bash tools/check-sim-deps.sh
```

Trivial changes (typo, comment, formatting, import fix) need no review pass; everything else does.

## Running

```
cargo run -p dark-player                          # single-player, no project
cargo run -p dark-player -- --project ../adventurer  # the game project's meadow scene
cargo run -p dark-player -- --host 7777           # co-op host
cargo run -p dark-player -- --join 127.0.0.1:7777 # join
cargo run -p dark-host -- --port 7777 --project ../adventurer   # headless host with the world
cargo run -p dark-player -- --project ../adventurer --host 7777 --clients 1 --net-sim 60,20,5
                                                  # host plus one lagged local client
```
`--day-secs <n>` shortens the day for testing; `--player <uuid>` keeps a stable identity.
In the game: WASD/arrows walk, Shift runs, Space jumps, J attacks (again to combo), K dodges,
E talks to an NPC in reach (number keys answer when a conversation offers choices; on its last
line E ends it; walking away ends it any time) or packs up your tent, Z sleeps (when every player online sleeps,
the night passes), 1–8 use the hotbar (food, drink, the cloak on or off, tent, firewood, charm,
soap), R relieves yourself, Q asks whoever is in reach to join you (again near your follower sends
it away; with nobody near you leave your party), F1
toggles the collision overlay (`--overlay` starts with it on), F2 switches language (`--lang ja`
starts in Japanese). Hosts take `--save <file>` (carry the whole world on; it is saved at each new
day and on quitting, Ctrl+C for `dark-host`; without `--player` the save's host plays on as
themselves) and `--world-seed <n>`. The story is the project's `story.ron`.

The world simulation, fast-forwarded:
```
cargo run -p dark-cli -- simulate ../adventurer --seed 3          # one year's chronicle
cargo run -p dark-cli -- simulate ../adventurer --runs 500        # balance statistics
```

Checking visuals without a person at the screen:
```
cargo run -p dark-player -- --project ../adventurer --autopilot --screenshot out.png --frames 290
cargo run -p dark-player -- --project ../adventurer --autopilot-fight --screenshot out.png --frames 150
cargo run -p dark-player -- --project ../adventurer --autopilot-camp --screenshot out.png --frames 190
cargo run -p dark-player -- --project ../adventurer --autopilot-party --screenshot out.png --frames 240
cargo run -p dark-cli -- preview-sheet ../adventurer sheets/grass_props.sheet.ron out.png
cargo run -p dark-cli -- preview-spine ../adventurer sheets/goblin.spine.ron attack_down 20 out.png
```

Spine skeletons (4.1 JSON exports) must be baked for the host after every export; with
`DARK_TEST_PROJECT=<abs path to ../adventurer>`, `cargo test` fails on a stale bake:
```
cargo run -p dark-cli -- bake-spine ../adventurer sheets/goblin.spine.ron
```

Sound is FMOD Studio (runtime 2.02.30, loaded at run time; silent without it). To change the
game's sounds: edit `../adventurer/audio/source/` (or re-run `make_placeholder_sounds.py`), then
```
"C:/Program Files/FMOD SoundSystem/FMOD Studio 2.02.21/fmodstudiocl.exe" -script build.js Adventurer.fspro
"C:/Program Files/FMOD SoundSystem/FMOD Studio 2.02.21/fmodstudiocl.exe" -build Adventurer.fspro
```
from `../adventurer/audio/fmod/`. Banks must be built by Studio 2.02.x to load in the 2.02 runtime.

The tent and campfire are placeholder art: `python ../adventurer/placeholder/make_life_art.py`
redraws them.

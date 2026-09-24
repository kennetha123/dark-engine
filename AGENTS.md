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
cargo run -p dark-player -- --project ../adventurer  # the title screen: carry on, or a new year
cargo run -p dark-player -- --host 7777           # co-op host
cargo run -p dark-player -- --join 127.0.0.1:7777 # join
cargo run -p dark-host -- --port 7777 --project ../adventurer   # headless host with the world
cargo run -p dark-player -- --project ../adventurer --host 7777 --clients 1 --net-sim 60,20,5
                                                  # host plus one lagged local client
```
Games are saved in `<project>/saves/*.sav` and the player's own settings (language, sound,
fullscreen) in `saves/settings.ron`; the title screen lists the games and holds the settings, and
takes the keyboard or a pad (`--title` forces it for a screenshot, `--save <file>` or any
scripted flag goes straight into the world; a screenshot or autopilot run ignores the settings).
Its "Play together" opens this player's year to others on port 7777, and lists the games on the
network with how full they are (`1/4` … `4/4`); every host says so on the port beside its own
(`--together` opens the screen on that list for a screenshot). Both sides must hold the same
project — the same settings, the same scenes, the same scene to start in — and the handshake now
checks it: a game in another world is marked as such in the list, and a player who joins one by
address is turned away instead of walking on ground the host does not have.
`--day-secs <n>` shortens the day for testing; `--hour <0-23>` starts the first day at that hour
(for watching a villager's day, or the light); `--player <uuid>` keeps a stable identity.
In the game: WASD/arrows walk, Shift runs, Space jumps, J attacks (again to combo), K dodges,
E talks to an NPC in reach (number keys answer when a conversation offers choices; on its last
line E ends it; walking away ends it any time) or packs up your tent, Z sleeps (when every player online sleeps,
the night passes), 1–8 use the hotbar (food, drink, the cloak on or off, tent, firewood, charm,
soap), Ctrl+1–8 lay one down (walk over it to pick it up again), R relieves yourself, Q asks whoever is in reach to join you (again near your follower sends
it away; with nobody near you leave your party), F1
toggles the collision overlay (`--overlay` starts with it on), F2 switches language (`--lang ja`
starts in Japanese), F11 fills the screen, Esc opens the menu (standing and people's feelings; the world waits while
nobody else is online; Q there saves the year and goes back to the title; `--menu` starts with it
open). A gamepad works alongside the keyboard:
left stick walks, left trigger runs, A jumps, X attacks, B dodges, Y talks, right shoulder asks
someone along, Back sleeps, Start opens the menu, the d-pad is the hotbar (with the left shoulder
for 5–8) and answers conversations. Hosts take `--save <file>` (carry the whole world on; it is saved at each new
day and on quitting, Ctrl+C for `dark-host`; without `--player` the save's host plays on as
themselves) and `--world-seed <n>`. The story is the project's `story.ron`.

The editor (maps, villagers, enemies, exits, text, worlds made from a seed and the places stamped
on them; Play needs `dark-player` built beside it).
The minimap under the map list shows the whole map and moves the view when clicked or dragged:
```
cargo build -p dark-player && cargo run -p dark-editor -- --project ../adventurer
cargo run -p dark-editor -- --project ../adventurer --script "click 234,15; drag 660,700 900,780" --screenshot out.png --frames 5
```
Scripted runs save what they save: point them at a copy of the project (with `Art/` a junction
to the real one, not copied; remove the junction on its own before deleting the copy).

A build to hand to someone without the engine (the exe, a `game` folder with only what the game
loads, and a README; double-clicked, it plays the project beside it):
```
cargo build --release -p dark-player -p dark-cli
cargo run --release -p dark-cli -- package ../adventurer ../builds/Adventurer
```

The country a seed makes, from far above (it also says where the ground changes most):
```
cargo run -p dark-cli -- preview-land 20260923 16000 20 out.png
```
A map made from a seed rather than drawn says `land: (seed: n)` in its scene, and each such scene
carries its own seed. Its `scatter` groups then say what *grows* on it rather than being strewn
over it once at load: `count` is how many of that group belong in each patch of land (64 tiles a
side), and what grows is worked out from the seed as the land is made, so every machine grows the
same wood. A town, a camp or a ruin is drawn as a scene of its own and stamped onto the world with
`places: [(scene: "scenes/camp.ron", at: (x, y))]`: everything in it becomes part of the map it is
stamped on, the ground under it is levelled into the land, and nothing grows on it. The land around each player is shaped as they walk and let go of once nobody
is near it; what the scene draws by hand is kept tile for tile (`Floor` says nothing — level made
ground on purpose with `Level(0)`), and a start or a doorway the land put under water is moved to
the nearest ground somebody can stand on.

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

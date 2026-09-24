# Dark Engine — Plan

Internal Dark Wagon Studio engine for top-down 2D pixel action RPGs, written in Rust.
Targets Windows and Linux. This document holds the locked decisions; change it deliberately.

First game: 4-player co-op, low-fantasy life-sim action RPG on a 365-day clock.

---

## 1. Locked decisions

| Topic | Decision |
|---|---|
| Language | Rust, cargo workspace, edition 2024 |
| Platforms | Windows + Linux (client, editor). Headless host on both. |
| Genre | Top-down 2D pixel **action** RPG (hack-and-slash). Free movement, no tile-stepping for players. |
| Tile size | Per-project, 4–1024 px, default **64 px**. World units are pixels. |
| Height | Tiles have height levels, entities have `z`. **Jumping** between ledges (CrossCode/Alabaster Dawn style). |
| Sprites | Grid slicing, manual rects, Aseprite import, 9-slice. |
| Skeletal | Spine via `rusty_spine` (spine-c). Version pinned to studio's Spine editor. Hitboxes baked for the host. |
| Audio | FMOD Studio API. SDK stored internally (private repo / artifact store). |
| Networking (game 1) | Host-authoritative **listen server**, max **4 players**. Single-player = host with zero remote clients. |
| Networking (later) | SpacetimeDB for persistent backends (cloud saves, accounts, extraction-style games). Dedicated `dark-server` for PvPvE. |
| Scripting | Luau via `mlua` (advanced event commands, mods). |
| Editor | egui in fixed panels (no docking: one simple layout), RPG Maker-style, WYSIWYG (maps drawn by the game's own renderer), no programming needed; separate binary; playtest runs the player as a separate process. |
| Distribution | Internal studio tool. No public API stability promise. |

## 2. Architecture rules (enforced)

1. **Simulation never depends on presentation.** Sim crates (`dark_core`, `dark_time`, `dark_net`, `dark_world`,
   `dark_sprite`, `dark_assets`, `dark_physics`, `dark_sim`, `dark_combat`, `dark_life`, `dark_story`, and later `dark_ai`) must not depend on `winit`, `wgpu`,
   FMOD or Spine. CI checks this with `tools/check-sim-deps.sh`.
2. **Everything goes through the network path.** No separate single-player code path; loopback transport.
3. **Fixed timestep simulation (60 Hz).** Rendering interpolates. Gameplay code never reads frame delta.
4. **Players and NPCs are the same `Actor`.** Only the controller differs (input vs AI).
5. **Host owns the truth.** Clients send intents, host validates and replicates.

## 3. Stack

| Area | Crate |
|---|---|
| Window/input | `winit`, `gilrs` |
| GPU | `wgpu` |
| ECS | `bevy_ecs` (standalone) |
| Math | `glam` |
| Collision | custom controller + `parry2d` |
| Pathfinding | `pathfinding` (A*), flow fields in-house |
| Transport | `renet` + `renet_netcode` (UDP); renet local client for the host's own player |
| Serialization | `serde`, RON (source data), `postcard` (wire/packed) |
| Text | `cosmic-text` with the project's own font file (no system fonts), glyphs cut to hard pixels |
| Images | `image`, `asefile` |
| Hot reload | `notify` |
| Localization | RON string tables per language (`locale/<code>.ron`, see §11); `fluent` if plurals and grammar need it |
| Editor | `egui`, `egui-wgpu`, `egui-winit`, `rfd` (native file dialogs) |
| Profiling/logging | `tracing`, `tracy-client`/`puffin` |

## 4. Game 1 requirements on the engine

### 4.1 Time (modelled on The Forest)
- Clock always runs; paused only in single-player. Esc opens the menu; while nobody but the
  local player is online (a connection in its grace window counts as someone) the world waits
  there: the host still reads and writes the network every tick, and any session change (someone
  joining, a connection lost) moves the world on so it is never missed. Enemies' respawn times
  wait too. Behind the menu the character takes no input. The menu shows the player's standing
  with each faction and how the living people who feel anything about them feel (`StoryView`;
  sent to a client when it changes and once a second, not in every snapshot).
- Default day length **24 real minutes** (tunable).
- **Sleep consensus:** when every *online* player is asleep, skip to morning.
  Offline characters count as asleep. Players inside the reconnect grace window count as **awake**.
- 365-day year. Autosave at each new day. The hero party may reach the Demon Lord earlier.

### 4.2 Sessions
- Persistent `PlayerId` per player (dev: local UUID; later Steam ID). Characters stored in the host's world save.
- Drop-in at any time; character wakes where it slept.
- Host leaving ends the session. No host migration.

| Event | Result |
|---|---|
| Join | Character wakes where it slept (new character spawns at a start point). |
| Graceful quit | Character moves to nearest inn and sleeps. |
| Crash / connection loss | 60 s grace. Character stays in place and **can be damaged** (no disconnect-escape). |
| Grace expires | Character moves to nearest inn and sleeps until reconnect. |
| Reconnect in grace | Resume control. |
| Restart before the crash is detected | The new connection takes over the stale one and resumes. |

A crash is only noticed when the transport times out (renet netcode: ~15 s), so the 60 s grace
starts ~15 s after the actual crash. Tune the transport timeout, not the grace, if that feels long.
Graceful quits and rejections are closed by the host, so their final messages always arrive.

### 4.3 Split play
- Players may be in different regions at once. The host runs **multiple maps at full detail** plus off-screen sim.
- Replication is scoped per map from day one.

### 4.4 Parties
- Max party size 8: up to 4 players + NPCs. Parties are social; members may be in different regions.
- Join / leave / kick / betray / merge. NPC loyalty.

### 4.5 World simulation
- **Actors** (players + NPCs) with stats, inventory, relationships, role.
- **Roles & titles:** occupations (beggar, mage, researcher, explorer, …) and transferable titles
  (Hero, Grand Mage, Priest, Demon Lord's generals). Kill the Hero → the title can pass to you.
- **Factions & reputation:** kingdom, church, demon army, guilds. Joining the enemy is a faction change.
- **World director:** HTN/GOAP planners for key actors. The hero party (warrior hero, grand mage, priest)
  follows a year-long plan, adapts to losses, recruits replacements (possibly players).
- **Simulation LOD:** full sim near players; off-screen travel and fights on a world graph, resolved statistically.
- **Storylets:** quests/dialogue gated on world state; endings evaluated from final state.
- **Fast-forward tool:** headless run of 365 days in seconds for balancing.

### 4.6 Low-fantasy life simulation (`dark_life`)
Realism is a pillar: monsters and magic exist, but bodies and weather are real.
- **Needs:** hunger, thirst, fatigue, bladder, bowel, hygiene. Failing a need has consequences
  (debuffs, soiling yourself, social reaction).
- **Intoxication:** alcohol raises drunkenness → drowsiness, impaired movement/aim, passing out.
- **Temperature:** regions have climate + weather + time-of-day temperature. Clothing has insulation and
  cold/heat resistance. Heat sources: fire, heating magic. Exposure → hypothermia/heatstroke.
- **Shelter:** sleeping in the wilderness needs a **tent/camp** (placeable structure). Inns are safe;
  anywhere else a sleeper can be attacked.
- **Relationships:** friendship, romance, marriage proposals, marriage. Intimate scenes are
  **fade-to-black** (mature rating, no explicit content).
- NPCs have the same needs and schedules (eat, sleep, work, drink); needs run at LOD off-screen.

### 4.7 Combat (`dark_combat`, `dark_ai`)
- Per-frame hitboxes/hurtboxes (sprite editor; Spine bounding-box attachments).
- Frame data (startup/active/recovery), cancel windows, input buffer, combo state machines.
- Dodge with i-frames, knockback, poise/stagger, hitstop, screen shake, rumble.
- Air attacks when jumping; projectiles; AoE; aim by mouse/right stick; optional lock-on.
- Enemy AI via behavior trees / HFSM, A* + flow fields, boss phase timelines.

## 5. Networking foundation (current scope)

Built (M0):
- `dark_net::Host` wraps a renet server. The host's own player is a renet in-process local client
  (single-player is this alone); remote players connect over UDP via `renet_netcode` (dev: unsecure).
- Channels: control (handshake, owned by `dark_net`), events (reliable ordered), state (unreliable).
- Session lifecycle: `InGame → Grace(60 s) → Offline`; reconnect in grace resumes; stale-connection
  takeover; hello timeout and rejection linger so no connection can squat a slot.
- Stable `PlayerId`. Sleep consensus rule in `dark_time` (not yet wired to characters).
- **The handshake checks that both sides hold the same world**, not only the same wire version.
  `Project::fingerprint` folds the scene the game starts in, every scene it can walk into from
  there, the tile size they are built on and the definitions the simulation reads (`combat.ron`,
  `life.ron`) into one number; the client sends it in `Hello` and a host with a world of its own
  refuses any other with `DifferentWorld`. `combat.ron` is in it because the looks a snapshot
  names are a list built from it, so an enemy added on one side renames everybody. A scene nothing
  can walk into is left out, so a packaged build — which carries only what the game can reach — is
  the same world as the project it was made from. Two things make this worth refusing over: maps are
  numbered by walking the ways out from the starting scene, so a different set of scenes can make
  the same number mean two different maps, and a scene made from a seed (§24.4) is a whole country
  in one number — a client with an older copy would predict against ground its host does not have.
  Art is deliberately not in it, nor sound, fonts, languages, the window's resolution or the
  game's name: a player with a different tree sees a different tree, which is no reason to refuse
  a game. A host with no project (`dark-host` without `--project`, the world simulation, the
  tests) says `0` and asks nobody — worth knowing, because such a host will accept a player whose
  maps it does not have at all; it is a development mode, not a way to play. Only the host may be
  the one that does not care: a *client* that sends `0` is turned away. The beacon (§23) carries the same number, so
  the title screen marks a game in another world rather than letting a player pick it and be
  turned away.

Built (M2.5):
- Characters (`dark_world::characters`): one model for players and NPCs. `control(state, grounded,
  input)` is a pure function, run by the host for every character and by a client to predict its own.
- Host (`dark_world::replication`): a session join spawns a character (or wakes the sleeping one);
  remote players send numbered inputs with the last 8 repeated; the host applies each input exactly
  once, in order, one per tick. When none is waiting and the player is connected, the character
  *holds*: no control, no physics, not even gravity, and nothing is acknowledged, so the host's
  state after input N is exactly the client's prediction after N; the queue that builds up is a
  jitter buffer. A hold lasts at most 30 ticks; after that the character steps with no input, so a
  stalled client cannot freeze it mid-air. A gap waits 3 ticks before being skipped; a queue past
  20 drops its oldest inputs but carries their presses forward.
  Inputs must lie within 240 of the last applied; messages over 16 inputs are refused. A reconnect
  wakes the same character with a fresh input queue. Snapshots at 20 Hz carry the characters in the
  recipient's map only, the day and hour, the last input applied and the host's queue depth,
  which the client uses to run its ticks up to 2 % fast or slow to keep that queue short.
- The host's own player feeds its character from `LocalInput` directly: its session goes through
  the `Host`, its input has no network to cross. It lives on a separate in-process renet server so the
  UDP transport never touches it.
- Client (`dark_world::ClientSession`): predicts its own character with the host's code and physics,
  rewinds to each snapshot and replays unacknowledged inputs, smooths any correction; others are
  interpolated 100 ms behind. `last_correction()` reports prediction error: 0.00 px while walking,
  jumping and attacking, with 60 ms latency, 20 ms jitter and 5 % loss (live) and 40/20/10 % (tests).
- Tools: `--net-sim latency,jitter,loss` on clients, `--clients n` on a host, `dark-host --project`
  for a headless world, time-based `--autopilot` (same route at any frame rate), and an end-to-end
  test (`dark_world::net_tests`) running a host and clients over real UDP.
- Tick order: HostReceive, NetReceive (sessions, inputs), Control (controllers, enemy AI,
  followers, asking people along, using items), Physics, Fight (hits, deaths, standing), WorldStep
  (sleep consensus, bodies' minutes, world simulation, followers' reactions and parties),
  NetSend (snapshots), HostSend.

Deferred: Steam lobbies + relay (needed before release), SpacetimeDB backend, dedicated server, lag compensation.

## 6. Workspace layout

```
crates/
  dark_core      app, schedules, fixed timestep            (sim)
  dark_time      calendar, day length, sleep consensus     (sim)
  dark_net       transport, channels, sessions             (sim)
  dark_platform  winit window + input                      (presentation)
  dark_render    wgpu renderer (window or offscreen)       (presentation)
  dark_view      a map's sprites, shared by game and editor (presentation)
  … added per milestone: dark_assets, dark_sprite, dark_tilemap, dark_physics, dark_combat,
    dark_ai, dark_world, dark_life, dark_ui, dark_audio, dark_spine, dark_script, dark_data
apps/
  dark-player    client (window, render; hosts or joins)
  dark-host      headless host (sim + transport only)
  dark-editor    content editor (M8, see §18)
tools/           dev scripts (dependency guard, packaging)
docs/            this plan
```

## 7. Milestones

| # | Milestone | Exit criteria |
|---|---|---|
| M0 | Foundations | Workspace, CI (Windows + Linux + headless), fixed timestep, ECS app, window + wgpu clear, sim-dependency guard, net skeleton |
| M1 ✅ | Renderer | Configurable tile size/resolution, sprite batching, slicing, animation (done; see §9) |
| M2 ✅ | Movement + world | Height levels, jumping, controller, tile collision, Y+Z sorting, multiple loaded maps (done; see §10) |
| M2.5 ✅ | Net foundation | Loopback + renet, 4 clients, prediction, per-map scoping, grace/reconnect, conditioner (done; see §5) |
| M2.6 ✅ | Character + talk slice | 8-way directional sheets, run, jump animation, NPCs, speech bubbles, EN/JA string tables (done; see §11). Pulled forward from M7 (dialogue) and M9 (localization) on request |
| M3 ✅ | World sim prototype | Headless: 365-day clock, sleep consensus, hero party plan, off-screen sim, title transfer, fast-forward (done; see §12) |
| M4 ✅ | Combat slice | Combos, dodge, hitstop, 2 enemies with AI, FMOD, replicated (done; see §13) |
| M5 ✅ | Actors, roles, parties, life sim | Mixed parties, betrayal, factions, needs, temperature, tents, intoxication (done; see §14, §15) |
| M6 ✅ | Spine | Rendering + baked hitboxes (done; see §16) |
| M7 ✅ | Narrative + save | Storylets, dialogue, relationships/marriage, endings, full world save (done; see §17) |
| M8 ✅ | Editor MVP | Maps + height, database, roles/factions, calendar, storylets, hitbox/enemy editors, multi-client playtest, manual slice overrides (done; see §18) |
| M9 | Polish + ship | Steam lobby/relay, lighting, particles, Luau, localization, export |
| M10 | Open world | Visible-set drawing, spatial sim, chunk streaming, made land, world authoring. Size is the game's choice (§24.0); the design holds to 100 km (see §24) |

## 8. Risks

- **World simulation scope** is the biggest risk. M3 is headless and text-only on purpose.
- **Open world (§24):** the engine draws and simulates whole maps today, so a large world needs
  culling, spatial queries and streaming before it needs more land. Staged so each piece pays off
  on the maps that already exist.
- **Host CPU:** up to 4 full-detail regions + off-screen sim + life needs for many NPCs. Budget per tick from M3.
- **FMOD / Spine licensing:** confirm the studio's FMOD tier; one Spine license per animator.
- **Linux:** test Wayland and X11 early; ship `libfmod.so` with rpath.

## 9. Asset pipeline (as built in M1)

- A game project is a folder with `project.ron` (name, tile size, internal resolution), `sheets/*.sheet.ron`
  and `scenes/*.ron`. Paths are relative to the project. Game art stays in the game project, never in
  the engine repo. First project: `adventurer` (Desire Fantasy packs, 16 px grid, 640×360).
- Sheet slicing modes: `Grid`, `Character` (RPG Maker layout; `$` prefix = one character, `(N)` = N frames,
  rows down/left/right/up), `Auto` (islands separated by transparent gaps), `Manual`.
- `Auto` copies each island's own pixels into a freshly packed atlas. On packed sheets one sprite's box
  often encloses a neighbour (a character standing in a pine's shadow); drawing boxes from the source
  would draw both. Islands whose pixels or shadows genuinely touch stay merged; the editor (M8) needs
  manual split/merge overrides and stable frame names, because auto frame indices shift when slicing
  options change.
- Rendering: internal target at project resolution, whole-number upscale with letterbox, pixel-snapped
  camera and sprite corners, stable sort by (layer, feet y), one draw call per run of same-texture sprites.
  Texture arrays deferred until draw calls matter.
- Verification tools: `dark-cli preview-sheet` (frame boxes and pivots), `dark-cli image-info` (alpha
  histogram), `dark-player --screenshot <png> --frames <n> [--autopilot]` (deterministic captures), and
  `DARK_TEST_PROJECT=<abs path> cargo test -p dark_assets --test project_smoke`.

## 10. Movement and height (as built in M2)

- `dark_physics` (sim): `Terrain` is a grid of `Floor` / `Level(n)` / `Wall` per tile; a level is
  `level_height` px (default: the tile size). Props add `Collider`s (circle or box footprint with a base
  elevation and a height). Bodies are circular footprints with `elevation` and vertical speed.
- Rules: a body stands on the highest tile its footprint overlaps and may never overlap a tile higher
  than its feet + `step_up` (2 px). So it walks off a ledge only when fully clear of it, and a jump that
  clears a rim lands on it. Default jump apex ≈ 20.6 px at 60 Hz: one 16 px level per jump. Props block only while
  the feet are inside their height range, so low bushes and rocks can be jumped over. Blocked moves
  bisect to end flush against walls and slide along them.
- `dark_world::maps` (sim): the host loads the start scene and every scene reachable through `exits`,
  and simulates all of them in the same tick. Bodies carry `MapId`, `BodyState`, `MoveIntent`,
  `PreviousBody`; controllers write intents before the `Physics` system set.
- Scene files gain `terrain` (ordered `fill` rectangles plus optional `top_frame` / `face_frame`),
  per-prop `colliders`, scatter `collider` and `levels`, and `exits`. Scatter only lands on flat ground
  wide enough for the prop's footprint. RON files may write `Some(x)` as plain `x` (implicit_some).
- Prop sheets use `pivot: Footprint`: the bottom-centre of the opaque pixels, so feet (collision and
  draw order) are at the object's base, not the centre of a box that includes a soft shadow.
- Drawing with height: world positions are on the ground plane; sprites draw `elevation` px higher.
  A raised tile's top is sorted by its north edge, its cliff face by its south edge, so characters draw
  correctly on top of plateaus, in front of cliffs, and behind them.
- Loading validates: scene paths are normalised; player spawns and exit arrivals must be clear of
  walls and props and not inside the destination's exits; scatter keeps clear of arrivals and checks
  the whole collider footprint is flat.
- Low props (height below the jump) can be stood on: their top is ground once the feet are level
  with or above it, so jumping onto a rock lands on it instead of shoving the body sideways.
- Corner slip: walking straight into a corner the footprint clips by up to `corner_slip` (4 px)
  slides around it instead of stopping dead.
- Bodies make room for each other (`dark_world::crowd`, after bodies move and take their exits):
  **a body is pushed back at most as far as it just moved**. Walk into someone and you are put
  back where you leaned on them, so you slide around them; stand still and nobody can shove you,
  so a villager left somewhere stays there and a crowd cannot push a player around; two walkers
  meeting are each put back as far as they closed, so they come to rest touching instead of
  bouncing. The push slides through the map (`World::slide`), so it can never shove anyone
  through a wall; feet more than 0.9 of a level apart in height pass each other (a ledge, a jump);
  the dead, the dormant and anyone standing in an exit take no part — a guard posted in a doorway
  must not cork it. The whole push is capped at 6 px a tick, above the fastest a body travels
  (a dodge, the heaviest knockback), so a roll cannot carry anyone through a body and a body
  thrown into a crowd is still put back rather than flung. Two bodies in the same spot part along
  x, the earlier `NetId` west, as soon as one of them walks.
- A client predicts the same push from its own body and the others as the newest snapshot showed
  them (kept in `NetId` order, updated before the replay, doorways skipped as on the host), so it
  is exact against anyone standing still that the host replicates. Against a body that is itself
  moving, the client's picture is a round trip old, so the push differs a little and the host's
  correction is smoothed as any other.
- Drawing order with height: raised tops are floors in `layer::TERRAIN + level`, below everything
  standing on them; a north cap (the strip of top someone behind the plateau can overlap) and the
  cliff faces are drawn in the world layer, sorted by the north and south edges.
- Characters (`SpriteKind::Character`) draw only their opaque pixels; the game draws a blob shadow
  (`SpriteKind::Blob`) on the surface below, which stays down and shrinks when jumping. The pack's
  baked shadows are too faint to read as a jump shadow.
- Silhouettes: sprites drawn after, and overlapping, a character write their compact rank into an
  R16Float occlusion mask (max blend, soft shadows excluded); covered character bodies then draw a
  shaded blue silhouette where the mask holds a later rank. Works for any number of characters.
- Known gaps: plateau sides are only a rim line; no slopes/stairs; the crowd is still every
  character against every character the host has loaded, plus a scan of the map's exits (the
  collider lookup was the same and is now a grid, §24.3); a height
  difference above one tile shows only one tile of north cap. Bodies that overlap while neither
  moves stay overlapped until one walks: players spawning on one spot, two arriving through the
  same door (an arrival counts as no travel), someone landing on a sleeper, and a body pressed
  into another against a wall, whose travel is what it managed, which is nothing. A character
  standing still is an obstacle nothing can move, so until NPCs walk (§11) a villager authored in
  a narrow gap blocks it; doorways are exempt, narrow gaps are not.

## 11. Characters, talk and text (as built in M2.6)

- Facing is eight-way. A sheet without diagonal art shows a diagonal as its sideways cardinal
  (`Facing::cardinal`), so four-way RPG Maker sheets keep working.
- Sheets may set `downscale: n` for art drawn at n× (RPG Maker MZ 3× art); it is shrunk before
  slicing, so one texel is one art pixel like everything else. `Directional` slicing reads a grid
  where each row is a direction and each run of columns an action, and makes `<action>_<dir>` clips.
- The controller picks, in order: attack (own sheet, if the look has one), `jump` in the air (plays
  once from takeoff), `run` while run is held and moving (80 px/s walk, 150 px/s run), `walk`,
  `idle`. A missing clip falls back to the next (run → walk, jump → walk/idle).
- Looks: `CharacterSheets` holds every look the loaded maps use (the start player first, then NPCs
  in map order), indexed by a replicated `LookId`. Host and clients build the same list from the
  same project files.
- NPCs: scenes list `npcs` (sheet, position, facing, `lines`). They are ordinary characters with an
  `Npc` component and no player; they replicate like anyone else. Interact (E) within 32 px and
  8 px of height turns the nearest NPC to the speaker and starts its conversation from the top;
  each press moves it on one line: a plain key the NPC says, `(reply: key)` the one talking says
  back. While a conversation is under way the press goes to it, whoever else is nearer. The
  press after the last line closes it, and the next press starts it again. One conversation per
  NPC at a time (plain or a storylet): another player waits, then hears it from the start. One
  side speaks at a time; a line stays up (`HELD_TICKS`) until the next press. The conversation
  ends when the talker is more than 96 px away or in another map, asleep, out cold or dead, or a
  storylet takes over. The dialogue window shows `E ▼` on a held line with no choices. Remarks
  said in passing (a follower's) last 5 s. An NPC nobody is speaking with turns back to its own
  facing.
  `Speech` carries who it is said `to`.
- Faces and names: a look may name a `face` (RPG Maker 4×2 faceset and index; painted art, so
  resized smoothly to 72 px) and a `name` key. The local player's own conversation (lines by them
  or to them) shows in a dialogue window at the bottom, face on the left; everyone else's speech
  shows as bubbles over heads. Faces and names sit on the look for now; actors own them from M5.
- Clips can be `flip_x` (a mirrored copy, pivot mirrored too), for side-view art: Pitaya's attack
  is her battler's swing row, mirrored to face right; up and down use the side poses.
- Lines are string-table keys, never text: the host replicates the key in `Speech`, each client
  shows it in its own language. `locale/<code>.ron` holds each language (project `languages`, first
  is the fallback), with `locale/<code>.editor.ron` (text written in the editor, §18) read over it;
  a missing string falls back to the first language, then shows the key.
  Adventurer ships `en` and `ja`; F2 or `--lang` switches.
- Text: `dark_render::TextSystem` shapes and wraps with cosmic-text (words for Latin, between
  characters for Japanese), rasterises each glyph once into a 1024² atlas and draws glyph sprites.
  Coverage is thresholded so a pixel font at its design size stays crisp; adventurer uses
  DotGothic16 (OFL) at 16 px. Interface sprites use `layer::UI`, which never occludes characters.
- Known gaps: NPCs do not walk yet;
  talking is not predicted, so a bubble appears one round trip after the press; the atlas never
  evicts (about 4000 glyphs); no kinsoku (Japanese line-start rules) yet; lines are one-shot, not
  conversations or choices (M7 storylets).

## 12. World simulation (as built in M3)

- `dark_sim` (sim crate, no ECS): the whole world over the year, stepped in in-game hours.
  Integer maths and its own seeded generator (SplitMix64, saved with the world), so a seed gives
  the same year on every machine and a saved world replays exactly.
- The calendar (`world.ron` `calendar`, M8): seasons through the 365 days, each from its first
  day (no two on one day) to the next one's, with a warmth added to every region's air (days
  before the earliest are the latest season's), and events on a day for 1 to 365 days. Storylets can wait for either (`Season(id)`,
  `During(id)`). A world carried on from a save follows the project's calendar as it is now
  (so carrying on a save needs the project's `world.ron` to read).
- The world is authored in the project's `world.ron`: regions (kind, danger 0–10, inn) joined by
  roads (walking hours), factions (`hostile` for the demon army), titles with succession rules,
  actors (role, faction, power, home, titles; `boss` for those who guard their home), the hero
  party (leader title, roles, starting members, goal, lieutenants) and `tuning`. Names are
  string keys. Scenes name their `region`.
- Titles pass on when their holder dies: to the killer if `to_killer` and the killer is a person
  of the world (not a boss, not of a hostile faction: a demon general who slays the Hero does not
  become the Hero), else to the strongest free member of the appointing faction with the given role (never a
  player), else vacant. Kill the Hero and you are the Hero.
- The hero party is directed by a small HTN planner (`dark_sim::htn`): ordered methods, first
  applicable wins, plans are shallow and remade when they run out and every morning between
  journeys (a journey is seen through). Its reasoning in priority order: done if the goal fell;
  wait if no member is alive and nobody can be recruited; follow a player who holds the leader
  title while they are online; recruit the leader and each missing role (never a boss or anyone
  hostile; title holders first, then strength); march on the goal when only the road and a margin are left of
  the year; recover at the nearest inn when wounded; strike the goal when ready; hunt the weakest
  lieutenant when ready; otherwise train in a settlement. Defeating a lieutenant weakens the goal.
- Off-screen, travel and fights are statistical: monsters meet travellers by danger, fights are
  won with chance S^k/(S^k+E^k) and wound (or kill) by the odds; winners gain power. Regions a
  player is in are marked `detailed` and roll no random encounters (the full simulation owns
  them); players are actors too (by `PlayerId`, in the world's `player_faction`), can hold
  titles, lead the party, and are invited once when they are where the party lacks a role. Only
  online players make a region detailed or lead.
- On the host (`dark_world::WorldSimPlugin`): each tick the simulation catches up with the clock
  after learning where players are. Sleep consensus (§4.1) is live: Z lies down, pressing again or
  moving gets up; when every online player sleeps (offline counts as asleep, grace as awake) the
  clock skips to morning, everyone wakes and the world simulation runs the skipped hours.
  `--save <file>` carries a world on from that file (the clock resumes at its time) and autosaves
  at each new day (since M7 the whole world, §17).
- Fast-forward: `dark-cli simulate <project> [--seed n] [--runs n] [--lang code]` prints a year's
  chronicle, or statistics over many seeds (500 years in about a second). Adventurer is tuned so
  the hero party alone defeats the Demon Lord in about two years of three, around day 300.
- Known gaps: one directed party (other actors wait at home; no demon army raids, no NPC needs or
  schedules — those are M5's `dark_life`); players cannot kill people of the world in play yet
  (only monsters; the API exists: `kill`). A player's actor is made the tick they arrive.
  Sleeping works anywhere; tents and inns (§14) only make it warmer, more
  restful and, at an inn, safe.

## 13. Combat and audio (as built in M4)

- `dark_combat` (sim crate): movesets from the project's `combat.ron` and the fighter state
  machine. An attack is startup (the tell), active (a circular hitbox `reach` ahead, `radius`
  wide) and recovery, with a lunge while striking. Pressing again chains the combo (from
  `chain_from` into recovery; or within 24 ticks after it ends); presses are buffered 10 ticks;
  a dodge (with invulnerable ticks) or the next attack cancels recovery. Hits cost health and
  poise: only a hit that empties poise staggers (hitstun, knockback fading over it), so heavy
  enemies shrug off light hits. Death is an action too; everything is integer ticks.
- The fighter runs inside the character controller (`control`), so a client predicts its own
  attacks and dodges exactly (prediction tests still measure 0.00 px). Hits are the host's:
  `dark_world::combat` checks every active hitbox against characters of the other side (players
  and friendly NPCs against `Hostile`), in the same map and within 12 px of height, each swing
  hitting a target once. Being hit reaches a client in the next snapshot (a correction, by
  design). Friendly NPCs cannot be hurt yet. Players get up at the start 3 s after dying;
  enemies lie 2.5 s, go dormant (not replicated) and return to their post after `respawn`.
- Enemies are characters with an AI controller (`dark_world::ai`, rule 4): guard the post,
  chase a player in sight (straight when the line is clear, else A* over tiles via
  `dark_world::nav`, searching at most every 20 ticks; a goal inside a prop aims for the nearest
  open tile), attack in range, back off, return home past the leash. An enemy that cannot find
  its way home in 3 searches slips away and is back at its post a second later. Enemies never
  take exits (`StaysInMap`); a look without hurt or death clips holds its frame instead.
  `combat.ron` defines enemy kinds (look, moveset, sight, leash, range, speed, back-off,
  respawn); scenes place them (`enemies`), validated like NPCs.
- Hitstop is presentation, not simulation: the sim never pauses (it would break prediction), so
  the view holds attacker and victim frozen for the attack's `hitstop` frames when a hit counter
  changes, with a red flush, a spark and screen shake when the local player is involved. Health
  bars over enemies and the hurt; the local player's top left; dead enemies fade.
- Clips can be `flip_x` and carry their own `pivot` (battler rows stand at different places in
  their cells). Pitaya's combo is lunge, swing and spin slash from her battler sheet.
- `dark_audio` (presentation): FMOD Studio loaded at run time with `libloading` (one of two places
  `unsafe` is allowed), so the engine builds and runs silently without the SDK. One-shot events
  by path at world positions (pixels / tile size = metres, north ahead), listener at the camera.
  The project names the runtime library per platform, the banks and the runtime version.
  The view derives cues from replicated state: swing (or an enemy's wind-up tell), dodge, hit
  (hurt for the local player) and death.
- Adventurer's audio is built headless: `audio/source/make_placeholder_sounds.py` synthesises
  placeholder WAVs, `audio/fmod/build.js` (run by FMOD Studio 2.02's command line) rebuilds the
  project's events from them, and `fmodstudiocl -build` writes the banks. The runtime is FMOD
  2.02.30 (from the studio's FMOD for Unity), so banks must come from Studio 2.02.
- `--autopilot-fight` fights the nearest enemy (combos, dodges wind-ups) for screenshots.
- Per-frame boxes: a sheet's `boxes` (drawn in the editor's Sheets tab, §18) give each frame of
  a clip its own hit and hurt circles; a clip with any uses them instead of the moveset's circle.
- Known gaps: no air attacks, projectiles, AoE, aiming or lock-on; no rumble (the pad is read,
  never driven — see §20); no flow fields
  (A* per enemy is fine at this count); no boss phases; enemy tells rely on startup length and
  the wind-up sound, there are no tell animations; being hit is not predicted (the victim sees it
  a round trip late, like any host decision); players have no lag compensation for their hits;
  no Linux FMOD library in adventurer yet (the engine supports it).

## 14. Life (as built in M5)

- `dark_life` (sim crate, no ECS): a `Body` stepped one game minute at a time. Needs (hunger,
  thirst, fatigue, bladder, bowel, hygiene) are integers in sixtieths of a per mille, so a rate
  "per hour, in ‰" adds exactly per minute; drink is ‰ of a drink in the blood; the core
  temperature is hundredths of a degree. Bladder or bowel at 1000 ‰ is an accident (relieved,
  soiled, filthy until washed); starving, parched, freezing, hypothermic and heatstroke cost
  health by the hour; fatigue at 1000 ‰ or a blackout (5 drinks) knocks the body out until it has
  rested and sobered. The core drifts by (felt − comfort edge) / `chill` per minute (the remainder
  carried, so a little cold adds up), where felt is
  the air (an inn's indoor air), clothes, a tent, a fire nearby and heating magic.
  `Body::condition` says what that does to the character: speed (40–100 %), a drunken wobble
  (0–4), out cold, and statuses for the HUD. Every number is a tuning knob in `Rates`.
- The project's `life.ron`: items (consume, wear, warming magic, set down a tent or campfire,
  soap), the starting pack (the hotbar order) and what is worn, each region's climate (air
  coldest at 05:00, warmest at 17:00, straight lines between), and the sheets that draw tents and
  fires. Item icons are cells of an icon sheet, resized smoothly like faces.
- On the host (`dark_world::LifePlugin`): every player's character gets a `Life` (body and
  pack). Each tick its surroundings are worked out (region air, inn area or tent within 16 px,
  burning fire within 48 px, clothes); each game minute its body steps, health lost goes through
  `Fighter::suffer` (no stagger, no hit flash), accidents are said aloud, and the body's condition
  is written to the replicated `CharacterState::impaired`. When the night passes because everyone
  slept, bodies sleep through every skipped minute (air following the hour). Out cold counts as
  asleep for sleep consensus. A player who dies gets up with a fresh body; a player who is away
  (offline) has their body kept as it was and, if killed, gets up only when back.
- Prediction stays exact: `control` applies speed and a stagger (a triangle wave on the
  replicated `sway` tick counter, sideways, in plain arithmetic), and out cold lies still in the
  look's death pose, all from replicated state, so the client runs the same.
- Inputs: hotbar slots 1–8 and relieve (R) are presses in `TickInput`. Using an item, relieving
  yourself (only with something to relieve) and packing up your own tent (interact with no NPC in
  reach) are host decisions, like talking. Structures need clear, open ground ahead, level with
  the feet, and 14 px from any other; they reach only those within 8 px of their height. A
  campfire burns its minutes (through a passing night too) and is gone.
- Inns: scenes list `inns` (an area and a bed). There are no interiors yet, so the area by the
  door is the rooms: sleeping there is indoors, and enemies leave inn sleepers alone. A player who
  quits or whose grace window runs out is put to bed at the nearest inn (§4.2).
- Dropping and picking up (`dark_world::drops`): Ctrl and a hotbar number lays one of that slot
  down, in front of the character when that is clear, open ground level with its feet (the same
  test a tent must pass: no prop, wall, ledge or doorway), else at its feet. Not while jumping,
  fighting, asleep or held. The last of a worn item comes off as it is dropped, and more of the
  same dropped within 10 px joins the pile already there rather than making another.
  A drop goes to whoever comes within 17 px of it and within 12 px of its height — except the
  one who laid it down, until they step out of its reach, so dropping something where you stand
  does not hand it straight back. Only characters with a pack (players, so far) pick things up;
  what is left lying is saved with the world (`SavedDrop`, read as empty in older saves).
- Money and loot: `life.ron` names the item that is **money** (`money: "coin"`), carried like
  anything else and counted beside the hotbar; there is nothing to spend it on until shops.
  An enemy in `combat.ron` lists what it **drops** (an item, the fewest and most of it, and a
  chance out of a hundred); when it falls, the host rolls each one and lays what falls where the
  body is — on clear ground, joining a pile of the same already there, as a player's own drop
  does — for anyone to pick up. An enemy back at its post leaves its loot again the next time it
  falls. The rolls are the host's own and are never replayed, so a simple seeded shuffle of the
  tick and the body is enough.
- Replication: snapshots carry the tents, fires and dropped items in the recipient's map and the
  recipient's own body and pack (nobody else's). The HUD shows six need gauges (fuller is worse),
  the air's temperature tinted by how it feels, statuses, the hotbar with the worn item
  outlined, and the money beside it. A drop is drawn on the ground as its hotbar icon.
- Adventurer: bread, water, ale, a wool cloak (worn), a tent, firewood, a warming charm, soap and
  coins (its money); a demon soldier leaves its pay and sometimes rations, a goblin a few coins
  and sometimes stolen ale, a brute a heavy purse and often firewood;
  spring climates (the grove is cold at night without the cloak); the Meadowbrook inn is the
  Town01 house; the tent and campfire are placeholder art drawn by
  `placeholder/make_life_art.py`. `--autopilot-camp` pitches camp and gets drunk for screenshots.
- Scattered props now also keep 12 px out of hand-placed props' footprints (a house).
- Known gaps: NPCs have no bodies or schedules yet (the model is shared, only players get one);
  no weather or seasons (climate by hour only); no social reaction to soiling or drunkenness;
  items cannot be bought (storylets give them, enemies leave them, and they can be dropped and
  picked up); money buys nothing yet; what an enemy drops is written in `combat.ron` by hand
  (the editor's enemy form has no list for it); a drop lies where it fell for ever, and nobody
  but a player takes any notice of it; there is no gamepad button for dropping; no interiors;
  relieving yourself anywhere is fine.

## 15. Factions, parties and loyalty (as built in M5)

- `dark_sim::social` (in the world simulation, saved with it): every actor has a standing with
  every faction (−1000 to 1000): its own faction +500 and the other side −500 to start. Killing a
  person costs 400 with their faction; killing a friendly person is murder and also costs 150 with
  every other friendly faction; slaying one of the hostile side (a monster in play) earns 5 with
  every friendly faction. Numbers are `social` in `world.ron`.
- Parties besides the directed hero party: a player leads, players and NPCs follow (at most 8,
  at most 4 players). An NPC follows only if alive, not a boss, in no party (nor the hero
  party), in the same region, on the same side, and the leader's standing with its faction is at
  least its `trust`. Players join by both agreeing; two parties merge into the first's. A leader
  who leaves hands the party to the next player; a party with no player left breaks up. The hero
  party's director never takes someone who follows a player.
- Loyalty (NPC followers): starts at 300 plus half the leader's standing with its faction,
  grows 10 a day (less a tenth of any bad standing), and moves with what the game reports. Below
  0 a follower deserts; below −300 it betrays: it leaves, every faction in the party holds it
  against them (−600, once per faction), and it is an outlaw: nobody takes it in again and the
  hero party's director passes it over. A follower who leaves is remembered: asked again it comes
  back as loyal as it left, or, if it left unhappy, refuses (a grudge) until that fades (10 a
  day), so sending someone away and asking again mends nothing. Anyone can betray a party or defect to another faction; going over to
  the hostile side drops standing with every friendly faction to −500, costs titles those factions
  appointed (they pass by appointment), leaves the hero party and betrays a party of the old side.
  Chronicle lines for all of it.
- In play (`dark_world::PartyPlugin`): scene NPCs may name their world `actor` (a `Person`).
  Recruit (Q) near one asks them along (they answer aloud: yes, or why not), near your own
  follower sends it away; with no person of the world in reach, near another player it invites
  them (Q back within 10 s accepts), and with nobody near it leaves your party. It works at most
  every half second. Followers (`Companion`) are made to match the world's parties
  every tick: they walk after their leader (straight or by A*), come along through exits, fight
  enemies near the leader, and can be hurt; enemies go after them as after players. A person of
  the world who dies is dead in the world too and gone once the body has lain. A follower who
  betrays becomes an enemy where it stands (an AI brain, no return from death).
- Social reaction: followers within 120 px watch their leader's body (§14): an accident costs
  150 loyalty, each waking game minute drunk, soiled or filthy costs 1, an enemy their leader
  slays within 120 px of them earns 15, and they say so (at most every 30 s, an accident always).
  Slaying a monster earns the player standing. A person dead in the world or an outlaw is not
  standing in the scene as a friend when the host starts (a world carried on from a save).
- Replication: snapshots carry the recipient's party (the other members' ids); the HUD lists
  those in the map, top right, with health bars. `--autopilot-party` asks Borin along and fights.
- Adventurer: Borin (kingdom, follows a newcomer) and Brother Oswin (church, `trust: 60`: about a
  dozen monsters slain first) stand in the meadow.
- Known gaps: a player betrays a party only by defecting (§17: then they are on the enemies'
  side and can fight their old friends); the world API `betray` has no way in play;
  map NPCs do not walk home when sent away, have no schedules, and are not moved by the world
  simulation (one recruited by the hero party off-screen still stands in the meadow); standing is
  not shown; followers do not share the leader's needs (NPCs have no bodies yet); kills by followers or other players
  earn their followers nothing; a save from before M5 loads with every standing at 0 and default
  `social` numbers.

## 16. Spine (as built in M6)

- The runtime is `rusty_spine` 0.7.1 (spine-c transpiled to Rust, so nothing to build), pinned
  exactly: it reads Spine **4.1** exports, the editor version the studio's art comes from. Only
  presentation and tools use it (`dark_spine`, `dark-player`, `dark-cli`); the sim-dependency
  guard keeps it out of simulation crates. `dark_spine::raw` is the second place `unsafe` is
  allowed (the runtime's page pointers and bounding-box vertex math), beside FMOD's.
- A look's sheet may be a `*.spine.ron`: skeleton JSON, atlas, a baked file, `scale` (Spine
  units to world pixels), `texture_scale` (how small the pages are drawn), how animations are
  named (`pattern`, default `{dir}_{action}`; `actions` maps engine actions such as `walk` to
  Spine's; `directions` maps the eight facings, compass points by default), which actions loop,
  and the `strike_event`.
- Baked for the host: `dark-cli bake-spine <project> <sheet>` poses every clip a tick at a time
  and writes each clip's length in ticks, its events (the strike event as `strike`), and per tick
  the circle around any bounding box named `hitbox` or `hurtbox` (drawn on the ground where the
  blow lands or the body stands, feet-relative). Loading the sheet builds plain clips from that,
  one frame per tick, so the controller, prediction and replication are unchanged and the
  headless host never runs Spine. A clip that plays once is `floor(duration × 60) + 1` ticks (its
  last key shown), a looping one `round(duration × 60)`; an event is on the first tick at or past
  its key. Baking is deterministic; with `DARK_TEST_PROJECT` set, a test re-bakes every sheet and
  fails on a stale bake, and the smoke test checks each bake's skeleton hash.
- The host uses the bake: an attack whose clip has a `strike` lands on the tick that frame is
  drawn (an attack's clip steps once on its first tick, so the drawn frame runs one ahead of the
  fighter's tick) and recovers until the clip has played out; every direction must agree, or the
  look is refused. A clip with baked hitboxes hits only on frames that have one, where the box
  is; a baked hurtbox replaces the footprint.
- The view poses each skeletal character exactly at its replicated clip and frame (no blending)
  and draws it as triangle meshes. The renderer draws meshes in the main pass, sorted among the
  sprites by layer and feet (`Renderer::render_with`), with the same shader attributes per vertex;
  their pages are smoothly filtered (`create_texture_smooth`), shrunk once to the size they are
  drawn at, turned from premultiplied to straight alpha, and their colour bled into transparent
  texels so filtering draws no dark fringes. Bubbles and health bars sit at the skeleton's baked
  height.
- `dark-cli preview-spine <project> <sheet> <clip> <tick> <out.png>` draws a pose on the CPU.
- Adventurer: the goblin from Echoes Below (eight-way top-down animations, `OnEventAttack` on
  its strike frame) guards the meadow beside the sprite enemies.
- Skeletal characters are bodies like sprite characters: covered by a prop drawn after them,
  they show as a silhouette (their triangles are drawn again in the silhouette pass). Meshes
  themselves never hide anyone.
- Known gaps: blend modes other than normal
  draw as normal; no mixing between animations (turns and changes snap, as with sprites); no
  death animation for the goblin (it holds its hit pose while it fades); binary `.skel` exports
  are not read yet (JSON only); a skeleton's root is taken as its feet (the goblin's stand a few
  pixels lower); the goblin has no bounding boxes, so its hits use its moveset's circle.

## 17. Save, storylets and endings (as built in M7)

- The whole world is saved (`dark_world::WorldSave`, one RON file, version 1): the world
  simulation (clock, actors, titles, standing, parties), every player's character (map by scene
  name, position, facing, health, body and pack), the structures set down, where the world's
  people stand, the story's state, and who hosted. Enemies return to their posts; fights and
  speech in progress are not kept. It is written at each new day and when the host quits (the
  player's window closing; Ctrl+C on `dark-host`), through a partial file renamed into place.
  Loading puts every character back asleep where it was, to wake when its player joins; the
  one who hosted plays their own character again unless `--player` says otherwise. A save from
  before M7 (the world simulation alone) still loads.
- `dark_story` (sim crate): the project's `story.ron`. A storylet is a conversation with one
  person of the world (`with`), offered while its conditions hold, told always, daily or once;
  when several are open the highest `priority` is told; it counts as told once the player
  answers or hears it out, so walking away loses nothing. Nodes are lines the person says, each
  with choices the player may answer with (themselves conditional); a node without choices ends
  the conversation. Conditions: standing, titles, the day, who lives, flags, the person's
  affinity, marriage (the player's and the person's), whether the person follows or would,
  the hero party, the player's faction, the goal's fate, `Not`.
  Effects: flags, standing, affinity, follow, join the hero party, defect, marry, fade to black,
  give items. `Story` (flags per player, affinity per person and player, marriages, what was
  told when) is plain data changed only by choices, saved with the world.
- Relationships: affinity (−1000 to 1000) is how a person feels about a player; storylets raise
  and test it. Marriage joins two unmarried actors; the intimate scene is a fade to black, never
  shown.
- In play (`dark_world::StoryPlugin`): interact near a person with a storylet open for you and
  they tell it instead of their plain lines. Their line stays up in your dialogue window; the
  choices open to you now come in your snapshots only and show numbered above it; a number key
  answers (while choices show, number keys answer instead of using the hotbar; the input names
  the node's own choice index, so a list that changed meanwhile never picks the wrong one). Your
  line shows, then theirs. Walking away ends it; on the last line interact does (and that press
  starts nothing else). One conversation per person at a time: another player waits. Nobody
  asleep, out cold or dead starts one; the person turns to the speaker. Deeds the game carries out: the fade (counted per player,
  replicated), items into the pack, and sides: a player who serves a hostile faction is on the
  enemies' side (they can fight their old friends and be fought; enemies leave them be), kept in
  step with the world every tick, a saved defector included. The demon army's people make pacts
  but follow nobody, a follower never fights its own leader, and killing a player earns no
  standing.
- Endings: when the year is over, the story's endings are tried in order (player conditions hold
  if any player meets them); the first that holds is shown to everyone as a card. `dark-cli
  simulate` prints each year's ending and tallies them.
- The renderer draws interface layers (`layer::UI` and up) in a pass after silhouettes, so none
  shows through a window or bubble.
- Adventurer: Borin warms to you day by day, grows fond, courts and proposes (a wedding night at
  the inn, faded to black), and greets you as your spouse after; Brother Oswin calls a player the
  church trusts to the Hero's side; Tessa in the grove, a cultist in secret, offers the Demon
  Lord's pact. Five endings: the dark pact, the player as Hero, the hero party's victory, a
  married life, darkness.
- Known gaps: conversations are one person and one player at a time (another player waits);
  storylets are not predicted (a round trip before the line shows); no gifts, jealousy or
  divorce; affinity shows only as a number in the Esc menu (§4.1); endings are one card, not a sequence; people do not keep their
  day's routine between saves (they have none yet); a save written mid-conversation loses it;
  a save carries its world as it was, so actors added to `world.ron` later are not in it (start
  a new world to meet them); the save is written inside a tick (a short hitch once a day).

## 18. Editor (M8, stage 1: maps)

- `dark-editor` (app, presentation) makes a game's content by pointing, clicking and typing:
  nothing is written by hand and nothing needs programming. It opens a project folder (asked
  for with a native dialog, or `--project`).
- The window, RPG Maker-style, in fixed panels: tools and Save / Undo / Redo / Play along the
  top; the project's maps (and New map) with a palette of every prop sheet's pictures on the
  left; the map in the middle; what is selected (or the map's own properties) on the right; a
  status line below. Tools: Select (click, drag to move, Delete), Terrain (paint ground, hills
  1–3 and walls; the right button paints plain ground), Props (click a palette picture, then
  the map; solid or not), Villager, Enemy (a kind from `combat.ron`), Exit and Inn (drag a
  box), Player start, Erase. The mouse wheel zooms (1×–6×); middle or right drag pans. Ctrl+S,
  Ctrl+Z, Ctrl+Y.
- WYSIWYG: the map is built exactly as the game builds it (`dark_world::Map::preview`: terrain,
  scattered props, colliders), laid out by `dark_view::MapView` (the game's own code), and drawn
  by `dark_render` into an offscreen target that egui shows (read as plain bytes,
  `Renderer::shared_view`, so its colours are the game's; GLES cannot, and shows them dark).
  Characters stand where they will,
  in their idle pose, with silhouettes behind props (a Spine character, which has no still
  frame, is a marked spot); exits, inns, the player start, names and the selection are drawn
  over it. Scattered props keep clear of where other maps' exits land, as in the game. The Collision toggle shows the game's own overlay.
- Text: a villager's name and lines are typed as plain text in the language chosen in the
  toolbar; the editor makes the string keys (`name.<map>.<n>`, `npc.<map>.<n>`) and writes the
  text to `locale/<code>.editor.ron`, which the game loads over the hand-written
  `locale/<code>.ron` (never rewritten, so its comments stay). If a table does not read, the
  editor refuses to save text rather than lose it.
- Saving writes the scene as RON (terrain painted tile by tile is packed back into rectangles);
  comments in a hand-written scene are not kept. After saving, the editor loads the maps as the
  game will and reports anything the game would refuse (a spawn inside a wall, say) in the
  status line. Opening another map, making a new one or closing with changes unsaved asks
  first (Save, Don't save, which drops the typed text too, or Cancel). Play saves, checks the
  game loads the map (and says why not), then starts `dark-player` (built next to the editor)
  on this map from its player start, in the language being written. Showing a field never
  edits the map (a value out of the field's range stays until changed).
- Undo keeps whole scene snapshots (200 steps); a burst of typing in one field, a drag or a
  paint stroke is one step. Text edits are not undone.
- Checking without a person at the screen: `--script` feeds clicks, drags, keys and typing as
  if a person did them, and `--screenshot <png> --frames <n>` saves the whole window.
- Stage 2, the database: a Database tab beside Maps (tabs along the top; one Save writes
  everything changed, text first; Undo follows the open tab). Categories on the left, their
  things in a list, a form for the chosen one: items (name, icon with its picture, what using it
  does), the starting kit (hotbar order, what is worn), climates (°C), movesets (health, poise,
  each attack of the combo, the dodge), enemies (look, attack sheet, moveset, how it thinks),
  people, factions, regions, roads, titles (who is next), the hero party, and Balance (bodies,
  the world's year, standing and loyalty). Names are typed as text; ids are typed once, to make
  something (tidied to lower case), and pickers show names with ids. Something still used (by a
  map, a person, the story…) cannot be deleted, and the editor says what uses it. Saving
  rewrites `life.ron`, `combat.ron` and `world.ron` (their comments are not kept; git has
  them), then runs the game's own checks, the world simulation's included; a file that did not
  read is never written. Maps pick from the database as it is (a new enemy kind is placeable at
  once).
- Stage 3, the story: a Story tab. Conversations are listed by the person they are with; the
  chosen one is drawn as a flowchart (a column per step from the start): each box is a line the
  person says with the player's answers listed under it, each answer's arrow going to the line
  that follows or to "end"; ★ marks a line that does something. Clicking a box opens it on the
  right: its text (typed), what reaching it does, and its answers (text, the line that follows
  or "a new line…", which makes and links one, conditions, what choosing does). When it is
  offered (with whom, how often, priority, conditions) folds above. Conditions and effects are
  picked from plain-English lists (each condition can be turned round with "not"); factions,
  people, titles and items are picked by name. "Read it through" follows the conversation as a
  player would. The year's endings are listed in the order they are tried. Saving rewrites
  `story.ron` and checks it as the game does, and that everything it names exists.
- Stage 4, sheets: a Sheets tab. A new sheet is made from a picture (browsed for in the
  project). The picture is shown cut as the sheet says, every frame outlined and numbered;
  clicking frames picks them, in order, to make a clip of. On the right: the picture, how large
  it is drawn, how it is cut (a grid; an RPG Maker character; each sprite found; rectangles by
  hand, the manual override; rows of directions by columns of actions), where frames stand,
  and the clips, the slicing's and the sheet's own (frames, speed, looping, mirrored). A clip
  plays, or steps frame by frame, large, standing on its feet: pressing on it places where
  that frame hits (red) or can be hit (blue), dragging sizes the circle. These are the sheet's
  `boxes` (per clip, per frame, from the feet); combat uses them as it uses a skeleton's baked
  boxes, instead of the moveset's circle (§13). Skeleton sheets show whether their bake is up
  to date, with a Bake button (the same as `dark-cli bake-spine`). Saving a sheet writes it and
  the maps load it again. Skeletons are drawn on maps in their idle pose.
- Stage 5, the year and the rest: a Calendar in the Database (the year's 365 days, coloured
  by season, events outlined; a click chooses a day, which a season can start on or an event
  be put on; seasons warm or cool every region's air, in °C; events last some days). Storylets
  can wait for a season or an event ("It is a season", "During a calendar event"); a season or
  event the story uses cannot be removed. Play takes a number of players: more than one hosts
  and opens a window for each other player, joined (`--host 7777 --clients <players − 1>`). Maps: the
  scattered groups are edited in the map's settings (pictures, from the palette; how many; how
  far apart; Shuffle; solid; on which ground), a prop's footprint part by part (round or a
  box, from its foot, low enough to jump over or not), and "Pick it there…" opens the map an
  exit leads to, where a click sets where it arrives (written to the exit's map at once).
- Stage 6, finding your way (§24.1): a minimap under the map list shows the whole map at once —
  the raised land, exits, inns, villagers, enemies and the player's start — with the part being
  worked on outlined; a click or a drag in it looks there, and panning the map view is kept to
  the map with half a screenful of slack.
- Known gaps: text edits are not undone (Ctrl+Z in a field undoes typing); ids cannot be
  renamed once made; the editor's own interface is in English only; a sheet saved again leaves
  its old picture on the GPU until the editor closes (the renderer frees no textures); the map
  view still does not zoom out past 1:1, which waits on §24.2.

## 19. Packaging a build (M9, first piece)

`dark-cli package <project> <out> [--exe <dark-player>]` writes a folder to hand to someone
without the engine: the game exe (named after the project), a `game` folder beside it, and a
README. Started with no `--project`, the game plays the project it finds in `game/` beside the
exe (or beside it), so the exe is double-clicked. A built game opens no console window; a
development build still does, and the log reaches a pipe or file either way. A built game also
does not hold the shell that started it, so a script that runs one directly and then reads what
it wrote must wait for it (`cargo run --release`, or `Start-Process -Wait`); the screenshot
commands in AGENTS.md go through `cargo run`, which waits.

- The game starts in the project's `start_scene` (`scenes/meadow.ron` when unset), which
  `--scene` overrides in play; a package holds that scene and everything reachable from it, so
  it starts where the project says.
- Only what the game loads is copied: the project file, the font with any licence beside it
  (a font is handed on under its licence), the FMOD runtime the project has for each platform
  (a platform without one is silent there, not an error) and its banks, each language's strings
  **and the editor's own `locale/<code>.editor.ron`**, `world.ron`, `story.ron`, `life.ron`,
  `combat.ron`, the starting scene and every scene its exits reach, the sheets those scenes'
  ground, props, scatter, people and enemies name, the set-down structures' sheets, items'
  icons, faces, and each sheet's picture — or, for a skeleton, its export, atlas, the atlas's
  pages as Spine's own reader names them, and the bake.
  A project folder holds whole art packs; Adventurer packages to about 24 MB of a 175 MB folder.
- Anything named but missing stops the package: a build broken that way only shows itself in
  play. So does a path that leads out of the project, which would be copied from, or over,
  somewhere else. The out folder must be empty or hold a package already (its README marks it).
- Known gaps: no zip (send the folder); the package is for the platform the exe was built for;
  the world is kept only with `--save`, which is a command line, not a menu; playing together
  is typed the same way (§5's Steam lobby is the M9 answer), over the internet it still needs
  the host's UDP port open, and connections are not yet authenticated.

## 20. Gamepads (M4's gap, filled)

`gilrs` in `dark-player` only (the simulation never learns where input came from: a pad fills the
same `TickInput` the keyboard fills, and both work at once — a key held leads, else the stick).
Pads are plugged and unplugged while the game runs. A pad plays once a button on it is pressed,
and the one pressed last is the one playing: a device nobody has touched — a wheel, a flight
stick, a pad resting off-centre — never steers, and plugging one in does not take control.
Unfocused, the pad does nothing: it belongs to no window, so the game reads it only while it is
the window being played.

| | |
|---|---|
| Left stick | Walk (the tilt is how fast, up to walking pace) |
| Left trigger, or the stick pressed in | Run |
| A / South | Jump |
| X / West | Attack (again to combo) |
| B / East | Dodge |
| Y / North | Talk, answer, pack up the tent |
| Right shoulder | Ask someone along |
| Right stick pressed in | Relieve yourself |
| Back / Select | Sleep |
| Start | The menu (as Esc) |
| D-pad up/right/down/left | Hotbar 1–4, or 5–8 with the left shoulder held; answers a conversation as the number keys do |

- The d-pad does not walk. It would turn the character as the slot it chose was used — planting
  a tent behind them — and answering with it would walk them out of the conversation.
- A stick nearer the middle than 0.25 is the stick resting, not a wish; past that the tilt is
  spread over the whole range again, so the slowest walk is a slow walk and not a quarter pace.
- On Linux pads are read through udev, so a build there needs `libudev-dev`; the headless host
  needs none of it (it has no input at all), and CI installs it for the workspace build. `gilrs`
  is in the sim-dependency guard's forbidden list: input is presentation.
- Known gaps: no rumble; the buttons cannot be changed; a pad the machine has no mapping for
  reports nothing the game understands, so it does not play (it says so in the log); menus are
  not walked with the d-pad (the menu only shows, it has nothing to choose).

## 21. People's days and lives (in build)

Two systems meet in every villager: a **day** (what they do on an ordinary day) and a **life**
(what happens to them across the year, whether a player is there or not). Both are data, written
in the editor; neither needs scripting.

### 21.1 The day (built)

- A scene's NPC carries `day`: hours and places (`DayEntry { from, at, sleep }`). The entry in
  force is the latest whose hour has come, so the last of the day runs through midnight, as a
  season does (§12). No entries: they stand where they were placed, as before.
- `dark_world::routine` walks them there: straight while the line is clear, else A* round
  (`dark_world::nav`, the same steering the enemies use), at 0.7 of walking pace, and lies them
  down when the entry says `sleep` and they have arrived. The way is worked out when the hour
  changes and at most every 20 ticks after that, not every tick; somewhere with no way to it is
  given up on after three tries, with a warning, until the hour changes.
- A routine never fights another controller for the character. Anyone in a conversation — saying
  a line, or with a conversation open — stands still, or they would walk out of the very
  conversation they are in; so do followers (`Companion`, who are led instead), the held, the
  hurt and the out cold. Whoever keeps a day `StaysInMap`: a day is written in one scene's
  coordinates, so its keeper never takes an exit out of it.
- Nobody talks to, or asks along, a villager who is asleep.
- Loading checks every day: its hours are hours, and its places are clear ground, level with
  where that villager was placed, and not in a doorway. A bad day is a map that will not load,
  not a villager pressing into a cliff all afternoon.
- Adventurer: Borin is home until mid-morning, in the village by nine, out in the field after
  noon, home for the evening, asleep by his door at night.
- Known gaps: the day is written per person, not per role, so two bakers repeat themselves; the
  places are points in one scene (nobody walks between maps); nothing is *done* at a place yet
  (no baking, eating or working, only standing there); a villager walks the same day whether or
  not anyone is in the map to see it; a blow wakes a sleeper only until they are free to act,
  when they lie down again; a routine NPC's saved position (`SavedPerson`) is overwritten by
  wherever the hour says they should be. The editor writes a villager's day — an hour, a place
  and whether they sleep there — and Play starts the game at an hour of the designer's choosing
  (`--hour`), since a day is only worth watching at the hour it happens.

### 21.2 The life (designed, not built)

A person's life is a chain of **stages** on the year's timeline. A stage says when it begins
(a day of the year, and/or conditions on the world as storylets use them), and what it changes
about the person: their role, where they live, what they say, a lasting hurt, or their death.
Stages advance in the world simulation, so they happen whether or not a player is in the region.

The baker of Aldmoor, as an example: *baking* from day 1; *burned* on day 85 (week 13), which
opens a storylet to help him to the hospital; *healed* if that is done; *amputated* if the week
passes unhelped; *beggar* six weeks later, his shop closed; *dead* by week 30, a grave with his
name in the town's graveyard. Arrive in week 13 and there is a quest; in week 14 the chance is
gone and the man has one hand; in week 20 he begs; in week 30 you meet only the stone.

- Quests are not a new system: a storylet's condition names a stage, and its effects move the
  person to another stage. The branch not taken is the stage timing out.
- Most villagers have no life written and simply keep their day. A light generated layer ages
  and occasionally buries the rest, so a long game shows churn among the unwritten.
- Authoring: the Calendar view (§18 stage 5) with one row per person, stages as blocks dragged
  along the year, branches drawn as the Story tab draws them.
- `dark-cli simulate` fast-forwards the year, so "what is Aldmoor like in week 30" is a second's
  work to answer without playing.

## 22. The title screen (M9)

A game is started and carried on from the game itself, not from a command line. `dark-player`
with a project and nothing else to do opens the title screen; anything told what to play — a
joined game, a co-op host, a scripted or screenshot run, a `--save` named on the command line —
goes straight in, and `--title` forces the screen for a picture of it.

- The screen is the project's name and a list: **carry on** (the newest game, with its name and
  the day it reached), **a new year**, **your games** (each save, newest first), **settings**
  and **leave**. Up and down choose, Enter (or Space, or E) takes, left and right nudge a
  setting that has a range, Esc goes back; on a gamepad the stick or d-pad moves and nudges, A
  takes, B or Start goes back. It is drawn by the game's own renderer in the project's font and
  language: no second interface toolkit in the player.
- Settings: the **language** (the project's, in turn; F2 in play changes and keeps it too), how
  **loud** the game is (tenths, shown as a bar, set on FMOD's master bus — taking the row goes
  up and comes round to silence, left and right go either way), and **the whole screen** or a
  window (F11 in play does the same, and reads the window rather than the setting, so one put
  back by anything else is not fought over). They are the player's, not the project's, and are
  kept beside the games in `<project>/saves/settings.ron`, written beside and renamed over so a
  crash mid-write loses nothing; a file that will not read is simply a new player's settings.
- A scripted run — a screenshot, an autopilot — neither reads nor writes them, so a picture is
  the same whoever takes it and whatever they last set.
- Games live in the project's `saves` folder, one `.sav` each (`dark_world::WorldSave`). A new
  game takes the first free name — `game`, `game-2`, … — so none writes over another, and the
  folder is made when a game is first saved. A file that will not read is left out of the list
  and left alone on disk.
- In the Esc menu, **Q** saves the year and goes back to the title, where it is the game to
  carry on. In someone else's game (§23) Q says goodbye to the host and goes back to the title
  without saving anything: the year is theirs, not this player's. A run the command line chose
  has no title to go back to, and the menu's line is shown only when it can be taken.
- Known gaps: no key or pad rebinding; loudness is one bus, not music and sound apart (there is
  no music yet), and an older FMOD runtime without the bus calls simply plays as mixed; a game
  installed somewhere the player cannot write keeps no settings (it says so in the log only);
  a game cannot be deleted or renamed
  from the screen; at most twelve games are listed, the newest by the day they were written
  (older files stay in the folder and still play); no picture behind the title, and nothing plays
  (§1's music and ambience are not built); starting a game reloads the scene from disk, which is
  a moment's pause.

## 23. Playing together, from the screen (M9)

Co-op starts where the game does, not on a command line. **Play together** on the title screen
opens this player's own year to others, and lists the games on the network to join — each with
how many are in it out of how many it holds, `1/4` until it is `4/4` and closed.

- **Opening a year** carries the newest game on (or starts a new one when there is none) with
  the host bound to port 7777, and everything else as in §22: the same world, the same save, and
  the same Q in the menu to save it and come back.
- **Finding games.** A host answers a small question on the port beside its own (7778 for 7777):
  the wire version, the game's name, the world it is being played in (§5), how many are playing
  and how many it holds (`dark_net::beacon`). The **version and the name are written outside** the
  rest of the answer, in a shape that is fixed for ever, and everything after them belongs to that
  version. This is not tidiness: put the version inside the part that changes with the version and
  the first change of shape makes an older game unreadable, so it would vanish from the list
  instead of being shown as one this build cannot play with. A game of another version is
  therefore seen and named, its port worked out from the one it answered on, and nothing more is
  claimed about it — not even how full it is. A player looking broadcasts the question every two seconds while the list
  is up, keeps what answers, and forgets a game that has been quiet for six seconds, so one that
  closes falls off the list while they are still reading it; what is picked stays on the game it
  was on as the list moves, and a game that closes under the choosing drops it to the line below
  the games rather than onto a stranger's. Away from the list nothing is asked and the socket is
  given back. `--together` opens the screen on that list for a picture of it.
- Every host answers — one opened from the title, one from `--host`, and `dark-host` — but only
  games on port 7777 are ever asked, so a host on another port has to be joined by address. A
  host with no local player of its own reads `0/4`.
- **Nothing that arrives is trusted.** Anyone on the network can send anything to these ports, so
  each call reads at most a frame's worth of packets, a name is cut to one line of forty letters,
  the list holds at most thirty-two games (the one that answered longest ago gives way, so a burst
  of made-up ones cannot keep a real game off it — a steady stream of them still can, since
  nothing proves who a game belongs to), and a host answers only nearby machines: a home network,
  a link-local address, itself, or the shared range a mesh network hands out. Further off is
  somebody else's business, and would make this game a way to shout at strangers. That shared
  range is also what an internet provider hands out behind its own network, so a player there
  answers their provider's other customers. A packet that will not
  read, or is not ours, is dropped; an error on the socket is about that one packet — Windows
  reports an oversized packet and a vanished host that way — and never ends the reading.
- A game of another wire version, one being played in another world, or a full one, is **shown
  but not joinable**, and says which it is: `(full)`, `(another world)`, or `(another version)`.
  `(another world)` means the two hold different copies of the project — different scenes, or a
  different scene to start in — and the host would turn this player away at the door (§5); saying
  so in the list is kinder than letting them pick it and find out. None of this is a guard: the
  world a host plays in goes out in the clear, so anyone on the network could echo it back. It is
  there to stop honest mistakes, as the rest of this section is. The host's own limit still
  decides: the list is only what
  the host last said, and a game shown as `3/4` may be full by the time the player picks it.
  Occupancy counts a player who has dropped out until their slot is given up, so a game someone
  left can read one higher than it plays for up to a minute (§5's reconnect grace).
- Joining loads this project's maps and sheets again and connects as `--join` does; both sides
  play the same project. The joined player's menu offers to leave, not to save; the goodbye is
  said once and then carried by the frames that follow — up to a second and a half, longer than
  the client's own leaving timeout — so the host hears a quit rather than reading a crash and
  holding the slot, and the window keeps drawing while the word travels — the menu says so, and
  Q does nothing more until it has gone.
- **When it does not work, the screen says so**, under the list it happened on, in the language
  the screen is in, until the player goes to another list: the game would not open (another is
  already on the port — and only then; a save that will not read says so instead), it cannot be
  reached, the network cannot be looked at, the host turned this player away, or the game they
  were in has ended. The last two come back to the title on
  their own — a host that leaves, or a game found full, no longer strands a player on a dark
  screen.
- Known gaps: the list is this network only (a friend elsewhere still needs `--join <ip:port>`,
  a forwarded port or Tailscale; Steam's relay is the answer when we ship on Steam); no address
  can be typed on the screen, because the player app reads keys, not text; the port is fixed at
  7777, so two games cannot be opened on one machine; the name in the list is the project's, not
  the host's own, and no password or invitation guards a game — anyone on the network can join.

## 24. The open world: chunks and streaming (§24.1–§24.4 built, §24.5–§24.6 designed)

The world is one continuous outdoors the player walks across without a loading screen. **How big
is the game's choice, not the engine's** — the first game may want ten or sixteen kilometres, and
the design must not be the reason a later one cannot have a hundred. Interiors stay as they are:
a house, a cave or a dungeon is a scene behind a door (§10), loaded in a moment, and the rules
below are about the outdoors only.

**Nothing that costs time may grow with the world.** Every frame and every tick is paid for by
what is near the players, never by how much world exists. That is the rule the rest of §24
serves, and each piece below names the thing that breaks it today. Hold to it and the size stops
being an engine question: the same code runs a four-kilometre world and a hundred-kilometre one,
and only the numbers in a couple of types change.

### 24.0 What each size costs

At 16 px to the tile with a tile about a metre (the adventurer project):

| Across | Tiles a side | Pixels a side | Tiles in all | What it needs on top of the last row |
|---|---|---|---|---|
| **4 km** | 4 000 | 64 000 | 16 M | Nothing new: it is inside today's map cap of 4096 tiles. Only §24.2 (draw what is seen) and §24.3's collider grid, both of which the engine wants anyway. |
| **10–16 km** | 10–16 000 | 160–256 000 | 100–256 M | **Done:** the cap is lifted, the land costs only what is shaped in it, a frame costs what is on the screen, the land is made from a seed as the players walk, and what nobody is near is let go of. Still wanted: what grows on the land (§24.5). Coordinates stay plain absolute `f32`. |
| **100 km** | 100 000 | 1 600 000 | 10 000 M | `Spot { chunk, at }` coordinates and rebasing (§24.4). Everything else is the same code. |

Two things to read off that table.

- **Ten to sixteen kilometres needs no new coordinate system.** An `f32` at 256 000 px is still
  exact to a sixty-fourth of a pixel, which is far finer than a game drawn at 16 px to the tile
  can see. What *did* break at that size was the engine's habit of nudging a sort key by a
  hundredth of a pixel — a shadow drawn behind its owner's feet — which an `f32` loses at
  262 144 px, and **16 km is 256 000 px: six per cent under the cliff**. **Built:** a sprite now
  carries a whole-number sub-layer beside its sort key, and ties are settled by it, so the fine
  order holds however far out the world goes. A test checks both halves — that the old nudge
  really does die out there, and that the sub-layer does not. The interface settles some of its
  own ties the same way. Where it does so by a character's world y — a speech bubble, the health
  bar over someone's head — the hundredths were gone a kilometre from the origin and those now
  use the sub-layer too. The parts drawn in a corner of the screen each have a place of their own
  (`over::` in the player), far enough apart that a part's own pieces settle themselves between
  two of them: they used to share one number and settle by the order they were drawn in, which
  held only because nothing could be added to a number that large.
- **Only the top row needs `Spot`.** Chunk streaming, made land, spatial queries and area-of-
  interest networking are the same at every size. `Spot` is a coordinate type behind which the
  rest is unchanged, so building the middle row first does not have to be undone to reach the top
  one — but the systems it touches (physics, drawing, the wire, sound) should be written so their
  position type can change without them being rewritten.

**Storage decides the same way.** Even the middle row is 100–256 million tiles: at two bytes each
that is a quarter of a gigabyte of ground nobody drew. So at any size past a few kilometres:

- **The land is not stored. It is worked out.** Ground, height, woods, rivers and roads come from
  the world seed, the same answer on every machine, worked out for the piece being walked on and
  let go behind. What a designer makes by hand, and what players change, is stored — and that is
  small, because it is only the places that have someone's hand in them.

### 24.1 Seeing where you are (built)

Before anything streams, a designer has to be able to find their way. The editor's map view shows
a screenful and never zooms out past 1:1, so on a map of any size one is lost immediately.

- A **minimap** in the maps panel: the whole map fitted into a small box, whatever its size — the
  raised land, the exits, the inns, the villagers, the enemies and the player's start — with the
  part being worked on outlined. Clicking or dragging in it looks there.
- The view is now **kept on the map**: its middle stays on the map, so the very edge of one can
  be worked on with that edge down the middle of the screen, and panning or zooming can no longer
  wander off into nothing and leave a designer hunting for their own map.
- The fitting is `apps/dark-editor/src/minimap.rs`, apart from the interface and tested without a
  window as `scene_ops` is: a map fits its box at any size up to the 100 km ceiling, a door 44 px
  wide is still drawn, and a
  click in the box leaves the view looking at the place that was clicked — the click, the clamp,
  the rounding and the view's own mapping are each harmless alone and meet there. Hand-placed
  props are marked; scattered undergrowth is not, or the map would be nothing but dots.
- Zooming out below 1:1 is now affordable — §24.2 made the map view cost the size of its panel
  rather than the size of the map — and wants only the camera itself changed, since `View::zoom`
  is a whole number today.

### 24.2 Drawing only what is on the screen (built)

Until this was built, the renderer was handed every sprite of the whole map every frame and
sorted them all, and `dark_view::MapView` built one flat list per map at load. Nothing was culled
anywhere. At meadow's 7 500 tiles that was invisible; at a thousand times that it is the whole
frame.

- `MapView` is now a **grid of pieces** 256 px square. A piece holds the sprites standing in it
  and the bounds they cover once drawn — which reaches past the piece, since a tree hangs above
  its own feet. A frame asks the pieces the camera reaches and then takes, sprite by sprite, what
  is really drawn inside it, with a pixel of grace for the whole pixels the renderer rounds to.
- The pieces to ask are **worked out from the camera**, never searched for: the rows and columns
  the view falls in, widened by how far sprites hang beyond the piece holding them — down and
  right at the near edge, up and left at the far one. Both ways, or a tree whose feet are just
  below the screen loses its canopy. So the looking costs the size of the view, and a map of any
  size is never walked.
- A sprite **larger than a piece** — the ground, a great tree — is held once by the map and
  pointed at by every piece it crosses, so it is drawn once however many of those pieces show.
  Holding such a sprite in one piece would make every view ask far beyond itself to be sure of
  finding it; drawing it always would make its cost grow with the world.
- **The order is kept, exactly.** The renderer's sort is stable, so sprites that tie keep the
  order they were given in — and the ground, the land, what stands on it and a sprite too wide
  for one piece all tie by design. Each sprite carries the place it had when a map was one list,
  and a frame hands them over in that order, so what is drawn can never depend on which piece a
  thing happened to fall in.
- The player's camera is worked out **before** the map is copied rather than after, since what it
  can see is what decides. The editor's map view does the same.
- Measured, in `dark_view`'s tests: the same screenful copies **the same sprites** from a map four
  screens across and from one 128 000 px across holding four million of them. A screenful walked
  over a field of trees, a quarter of a piece at a time, misses none of them — and that sweep is
  itself tested, by blinding the view to each side of its widening in turn and checking the sweep
  notices. A test that passes by where its steps happen to land is worth nothing. On the first
  game's meadow, which is two and a half screens wide, a frame copies about a fifth fewer — the
  saving is small exactly where the map is small, and that is the point.
- Proved unchanged: the autopilot, camp and fight screenshots are **byte for byte** what the old
  renderer drew.
- Not done here: the editor still poses every skeleton in the scene each frame, and the draw path
  still asks the colliders what each character stands on (a linear scan, §24.3). A map's pieces
  are also all allocated at load, which is fine for a map and is what §24.4's chunks replace.
- The **silhouette pass** is sized the same way and is worse than the sort: it tests every plain
  sprite in the frame against every character, both ways round, and it ranks them through a
  16-bit float mask that is only exact to 2048. That is a correctness cliff, not a slow frame —
  past it, silhouettes are wrong. Culling is what keeps the frame under it.
- This is worth doing before any streaming: it makes a frame cost the size of the screen rather
  than the size of the map, and it is what lets the editor zoom out. Lighting and particles (M9)
  want it first as well; a light per map-sized sprite list is the same bill again.

### 24.3 Room for the simulation (the grid and the budget built)

The host walks every body in every loaded map each tick, and several systems are quadratic in
everything the world holds:

- **Colliders** were a linear scan per query (`dark_physics`), and one body's tick makes tens of
  them; props scale with area, so the scan grew with the world. §10 called this out as a known
  gap in M2. Now every collider is listed in each 64 px square its footprint touches, and a query
  asks only the squares its own footprint reaches — the terrain lookup beside it was already O(1).
  Pushing a body out of a prop moves it while the props are being asked about, so that one query
  looks a little beyond the footprint — the longest push any prop in the map can give — and asks
  again if the body travels further than that.
  - A query answers **exactly as the scan did**: candidates come back in the order they were
    added, each named once, so what holds a body up and which prop it is pushed out of cannot
    change. A world can be told to look at every collider in turn, as it used to, and the tests
    play the same moves both ways: twelve thousand questions of a scattered world, forty walks
    of sixty steps each through props that overlap, hang off the map and are wider than a square,
    a body pushed sixty pixels along a row of props, and eight thousand knots of small props with
    a body shoved about inside each — every one landing in the same place. The knots are what
    find anything here: the trouble needs a prop to arrive late in a crowded place, which no case
    anyone writes by hand will produce. Taking away either half of the asking-again — the asking,
    or the rule that a prop the pass has gone past stays gone past — is noticed by them.
  - The world's collider list is no longer public: props are added through it, so a grid cannot
    go stale behind the queries' backs. What used to scan every collider — where a dropped thing
    lands, where a tent may be pitched, where a spawn is clear, whether a companion may stand
    behind their leader, and whether a tile can be walked — now asks the same question of what
    is near.
  - Measured: with the same props under the query, a map four times as wide asks about the same
    handful; with a hundred times the props, it asks about a hundredth of them.
- **Paths** were A* over the whole tile grid, exploring all of it when there was no way through
  (`dark_world::nav`) — and each tile asked the colliders, so the two costs multiplied. A search
  now has a **budget of twelve thousand tiles**, each of which costs up to sixteen questions of
  the map. That is set above the whole tile count of any map the game has (the largest is 7 500),
  so **today it can refuse nothing that exists**: a search that runs out has looked at more tiles
  than the map holds. On a map far larger it will refuse a long way round — a wall with a gap at
  its far end costs some thirty thousand tiles — and the walker then makes for its target
  directly, as it already does when there is no way at all. Searching chunk by chunk, with a
  coarse graph of the ways between them, is §24.4's answer; this is the bound until then.
- **Crowds, blows and snapshots** each build a list of everyone in the world and then filter it
  by map (`crowd`, `combat`, `replication`). The crowd is the quadratic one — everyone against
  everyone; the blows are every attacker against everyone; the snapshots are every player against
  everyone, every third tick. They become queries **by chunk** — which means they wait for §24.4,
  because the key they filter on today is `MapId` and that is the thing §24.4 replaces.
- **People far from every player** need the same treatment the abstract world simulation gives
  places nobody is in (`Region::detailed`). That is an analogy, not code to reuse: `dark_sim`'s
  regions are named places joined by travel times, with **no coordinates at all**, so distance in
  chunks is a new index over them. §24.5's stamps are where a chunk learns which region it is in.

### 24.4 The ground itself (built: held in patches, made from a seed, shaped and let go of)

- **The land is no longer written down where nothing has been done to it.** Terrain was one
  array of every tile in the map — a quarter of a gigabyte at 16 km, before anyone had shaped a
  thing. It is now held in patches of 64×64 tiles, made the first time something is put in one,
  and level everywhere else; levelling ground that was never shaped costs nothing at all. The
  same for what a map looks like: the pieces of §24.2 are made where something stands and are a
  pointer each where nothing does.
- So a map may now be **20 000 tiles a side** (the old cap was 4096), and the size of a world is
  no longer a question about what a grid of tiles costs.
- **A piece of a map is made when the camera reaches it**, and let go once five hundred others
  have been made since. Making a map used to walk every tile in it and keep a sprite for each: a
  quarter of a million shaped tiles came to 400 MB before the game had drawn a frame. A view now
  keeps only what it takes to make a piece — which props and ways out belong to which piece, and
  the pictures to draw them with — and walks the sixteen tiles a side of a piece when it is
  needed. The map-wide ground sprite is looked at every time rather than listed against every
  piece it crosses, which would have been a million entries at 16 km.
- **Measured**, on this machine, in a release build, with each run checked to have drawn its
  frame: a world 16 km across — 256 million tiles, a quarter of a million a side — takes
  **231 MB** with nothing shaped in it and **238 MB** with 400 hills of some quarter of a million
  raised tiles, against **255 MB** for the first game's meadow of 7 500 tiles. Four hundred frames
  cost 3.4, 3.2 and 3.5 seconds. So shaping a quarter of a million tiles costs seven megabytes
  more than shaping none, and a world thirty-four thousand times the size of meadow costs less
  than meadow does — because what is held is what is being looked at, and meadow carries the
  game's own art.
- The numbers above are what the engine costs, not what a game does: this test world has almost
  no art in it. What they compare is the same engine against two world sizes.
- A **chunk** is 64×64 tiles — 1024 px at 16 px to the tile. Sixteen kilometres is 250 chunks a
  side, a hundred is 1563. A chunk holds its tiles' heights, the props standing on them, and what
  lives there. The size of a chunk is a budget, not a world limit.
- **The land is made, not read** (`dark_land`, built): a seed and a tile give the same answer on
  every machine, worked out in whole numbers so a host and a client cannot disagree — one that did
  would have players falling through different rocks. Six lattices are laid over one another, the
  broadest two kilometres across and each next half the width and half the weight, so a country
  has highlands, lowlands and the water between, down to copses of rough ground. What comes out is
  water, level plain, or one of three steps up. **Measured** over five countries, twenty-five
  kilometres of each: **17%** water (13–21% by seed), **55%** level plain, and the three steps
  12%, 13% and 4%. A hillside never rises more than one step at a time, so there are no cliffs a
  walker can never get up.
- **It is shaped around the players as they walk** (`dark_world::land`, built): the patches within
  two of each player — 128 tiles, three screenfuls — are made if they have not been made already,
  and never made twice. The same patches are made the same way on every machine, so a client
  predicts against the ground the host has. A map says `land: (seed: n)` in its scene to be made
  this way; one without it is drawn by hand as before.
- **What is drawn by hand wins, tile by tile.** A scene's own fills are remembered as drawn — one
  bit a tile, only for the patches that have any — and the made land is laid underneath them. So a
  hand-drawn rock stays a rock, and the 4 095 tiles around it are still the land's own.
  **`Floor` is the exception and says nothing**: it is the ground every map starts as, filled by
  the acre, and a made world would be wiped flat by one such fill. To level made ground on purpose
  — to make room on it for something built — a scene draws **`Level(0)`**, which stands at the
  same height and *is* an opinion; a later `Floor` over a drawn tile takes the opinion back, as
  later fills have always won. Filling with `Floor` where nothing has been put still costs
  nothing at all.
- **Land nobody is near is let go of**, on the host and on every client alike (a client shapes and
  releases around the one player it predicts). A map keeps 4 096 made patches — 32 MB — and past
  that drops the ones more than eight patches (512 tiles) from every player; patches holding
  hand-drawn tiles are never dropped, because nothing can make those again. Nothing is lost by it: the seed
  makes the same patch again, tile for tile. Without this a map only grew — **measured** at 0.4 ms
  and 8 KB a patch, a player running makes some 3 000 patches an hour, so an evening of four
  players would have held a few hundred megabytes of ground nobody was standing on.
- **What making the land costs, measured** (release build, this machine): 0.4 ms a patch of
  ground. Walking makes five patches at a time when a player crosses into the next one — a
  **2.98 ms** tick with bare ground, about once every five seconds of running, against a 16.6 ms
  budget — and the 25 patches of arriving somewhere new cost 10 ms, which happens at map load,
  not in a tick. **Growing a wood on it as well** (§24.5, three groups: trees, rocks, grass)
  takes that worst tick to **3.49 ms** and the average step from 126 µs to 151 µs — half a
  millisecond for everything standing on the ground. So it is all done in `FixedUpdate` where it
  is asked for, and worker threads are still not needed. They become the answer when rivers,
  roads and towns are on the land too and a patch costs more than a tick can spare; the shape of
  the code (a patch at a time, made from a seed, never twice) is what makes that a change of
  scheduling rather than a change of design.
- **Where a scene puts people, the land may have put a lake.** A start or an arrival that cannot
  be stood on is moved to the nearest ground that can be, looked for by asking the land itself
  rather than by shaping it — eight ways out, a tile at a time, as far as two coarse lattices —
  and only what is settled on is then shaped, so a long search costs no ground. A villager or an
  enemy placed in water is still refused at load: that is a scene to fix, not a spawn to nudge.
- **A piece of the drawn map knows which land it was drawn over.** Each piece (§24.2) remembers
  whether the patch beneath it had been made, and is drawn again when that changes — so land
  shaped or let go of far away costs nothing, and a piece is never left showing level grass over a
  hill, or a hill that has been let go of.
- `dark-cli preview-land <seed> <tiles> <tiles-per-pixel> <out.png>` draws the country a seed
  makes from far above, and says where in it the ground changes most — somewhere worth standing to
  see what a seed made.
- Known gaps here: only the land's shape is made. What grows on it (trees, rocks, grass), where
  the roads and rivers run, and where anyone lives are still §24.5's work, so a made world is at
  present a country with trees, rocks and grass on it (§24.5) but nobody living in it. The
  clearing kept around a start is still worked out before a start in water is moved. The **editor** draws a
  made map as the flat ground it is before anyone walks it, and shows a start where the scene puts
  it rather than where the game will move it. A walk around
  a large lake can also cost more than the 12 000 tiles a path is allowed (§24.3), and nothing in
  the first game walks a made world yet. That two machines hold the same scenes, and so the same
  seeds, is now checked where it belongs — in the handshake (§5) — and a player holding another
  world is turned away rather than left to walk on ground their host does not have.
- An authored chunk is a patch laid over what was made; a chunk a player has changed is a smaller
  patch again, in the save.
- When a patch does become too dear for a tick, it is made on **worker threads**, in a ring ahead
  of each player, and let go behind. Count it honestly: crossing one chunk of the ring makes
  **five** new ones, a chunk is 1024 px, and a player walks at 80 px a second and runs at 150.
  That is about 23 chunks a minute walking, 44 running, and up to four players going four ways —
  call it **200 a minute**, so a chunk would have **tens of milliseconds**, not seconds.
- What a resident chunk costs has to be **measured, not assumed**. The heights are 8 KB and the
  colliders a few more; the sprites are the part that varies, because a wooded hillside emits one
  for every raised tile, every rim and every prop, and that is what today's `MapView` already does
  for whole maps. The ring's size follows the measurement, not the other way about.
- **Where a thing is** stays an absolute `f32` position up to about sixteen kilometres (§24.0),
  and every system here is written to work that way first. What must change at any size is the
  **sort keys**: a shadow is drawn behind its owner's feet by taking a hundredth of a pixel off
  its sort key, and an `f32` cannot hold that nudge past 262 144 px, which 16 km very nearly is.
  A whole-number sub-layer beside the sort key replaces the epsilon and the cliff goes away. (The
  interface already nudges sort keys at 3e9, where a whole unit is lost; that works only because
  the sort is stable. Decoration there; the ground under the player's feet here.)
- **Past sixteen kilometres**, where an `f32` is no longer fine enough to hold a position (an
  eighth of a pixel at 1 600 000 px: movement judders and a blocked move stops resolving against
  a wall), a position becomes `Spot { chunk, at }` — which chunk, and where in it. A client
  rebases on its own player's chunk; the host cannot, because its players may be 50 km apart, so
  it rebases **per body**, and the collider grid is keyed by chunk rather than by one origin.
  Nothing else in §24 changes. That is the point of naming it here: physics, drawing, the wire and
  sound should each take a position from one type, so the day it becomes a `Spot` is a day of
  changing that type and not of rewriting them.
- **The save** stops being one file of everything and becomes the world's own state (§17) plus the
  chunks that differ from what the seed makes. §17's save is flat lists of characters, structures,
  drops and people with no place-key; the list that grows with the world is the people, and what
  bounds it — how many are remembered, and where an absent person is kept — has to be decided with
  §24.3's distant people, not after them.
- **Textures have to be let go**, which nothing in the renderer can do today (§18's own gaps say
  so): a world crossing biomes loads sheets for ever otherwise. Letting a chunk go must let its
  art go, which means a texture handle that can be freed and re-used without the ones after it
  shifting. Spine rigs are loaded up front the same way and need the same budget.
- **The packager** (§19) collects what to ship by reading every scene's sheets. Made land names no
  sheets in any scene, so what the generator can choose has to be declared somewhere the packager
  reads, or a built game ships without its ground.
- `MapId` stays as it is: it is the interiors' limit, and the outdoors will not be a scene.

### 24.5 Making a world by hand (what grows on it, and places stamped on it: built)

- **What grows on made land is worked out, not written down** (`dark_world::grow`, built). A
  drawn map's props are strewn over it once, when it loads, and remembered; a made map cannot be,
  because there is no end to strew things over and the ground does not exist yet. So a scene's
  `scatter` groups say what *may* grow — which sheet, which frames, how dense, what footprint,
  which levels it likes — and the seed says *where*, a patch of land at a time, in whole numbers.
  Nothing is stored: the host and every client grow the same wood without a word between them,
  and a player who walks away and comes back finds the same tree.
- On a made map a group's **`count` is how many it wants in a patch** (64 tiles a side), because
  a made map has no end to fill. Spacing is kept **by construction**: the world is cut into cells
  as wide as the group's `min_spacing` and at most one thing grows in each, which is what makes
  the question answerable a tile at a time by anyone, without knowing what grew anywhere else.
- It comes and goes with its land. A patch's footprints go into the collision world when the
  patch is made and are taken out again when it is let go of (§24.4), so a world walked across
  for an hour does not hold every tree ever seen. That needed `dark_physics` to be able to **take
  a collider away**: a place left empty is used again before the list grows, and the numbers of
  the props that stayed never change.
- Nothing grows in water, on ground its group does not like, in a doorway, or where the scene
  puts somebody — and a great tree is kept further off than a tuft of grass, because what must
  stay clear is the clearing *plus* the thing's own footprint. Otherwise a player begins the game
  inside a trunk.
- Making the land and growing on it are **one call** (`Map::make_around`), and nothing else may
  make land. A patch is only ever made once, so ground made by a pass that did not plant stays
  bare for ever while the drawing shows a wood on it — a wood a body walks straight through.
  That is what happened at every spawn, and then again at every start the water moved, until the
  shore search was made to change nothing at all and the making was left to one door. A test
  holds the line: everything drawn on made land has a footprint, and every made patch has grown.

- **A designer works on a place rather than a map** (built). A town, a camp, a ruin is drawn as
  a scene of its own and *stamped* onto the world at a spot: `places: [(scene: "…", at: (x, y))]`.
  At load every stamped place is laid into the map it is stamped on — the ground it drew, what
  stands on it, the people who live there, their days, its inns and its own ways out, all moved to
  where it was put down. Afterwards **nothing downstream can tell a stamped town from one drawn
  where it stands**: spawning, talking, sleeping, pathfinding, saving and the map graph go on
  exactly as they were written. A place may stamp places of its own, four deep, so a town can be
  built of quarters; deeper than that is refused. A scene is a place or a map, never both: one
  stamped into a town *and* walked into through a door would be built twice, and the second one
  would hold a second copy of everyone who lives there. A place is put down on a whole tile, may
  not bring a player start, land or scatter groups of its own, and may not stamp a person of the
  world who is already on the map — two bodies with one name confuse every part of the game that
  knows them. Each of those is refused with a word about what is wrong, not dropped in silence.
- What a place does not bring is a player start, a seed, scatter groups or a ground sheet: those
  belong to the world it is stamped on. A world is the seed plus a list of places, as promised.
- **A place brings its own region**, and standing in it is standing there (`world.ron`, §12). This
  is the seam between the map the player walks and the world the simulation runs: a made world is
  one map, so without it a country would be one region from end to end — one danger, one set of
  rumours, one inn, every town the same place. Which region somebody is in is now decided by where
  they stand: the smallest stamped place holding them, else the map's own region.
- **Its edges are blended into the land.** The ground under a place is levelled to whatever the
  land makes of its middle — water included, so a camp is never half in a lake — and for four
  tiles beyond it each ring may differ by one more step than the ring before, so the ground walks
  out without leaving a wall a walker cannot climb or a pit they cannot get out of. It walks out
  to **what is already there** — another place's ground, or a hillside somebody drew — and only
  to the raw land where nothing has been drawn: a quarter stamped inside a town steps out onto
  the town, or the two leave a wall between them. Four tiles is always enough: made land rises a
  step at a time and there are only three steps. All of it counts as drawn by hand, so the land does not take it back when its patch is
  made, and **nothing grows on drawn ground** — a town square with saplings coming up through it
  is not a town square.
- What a place drew is laid over the levelling afterwards, not under it: the levelling is about
  the land, and a hut's floor is not.

**Still to come in §24.5:**

- The editor gains a **world view**: the whole world as the seed makes it, zoomed out to biomes
  and roads, with the authored places marked, and stamps placed and moved in it. §24.1's minimap
  is the small version of the same thing. **What the editor does today** (built): a map says it is
  made from a seed and which seed, and is no longer held to what a brush can cover, so a world
  scene can be born there; the land is made as the panel looks at it, a few patches a frame, so a
  designer sees the country rather than a green field; and a place is stamped by choosing it and
  clicking, moved by dragging it, and removed like anything else. A stamped place's ground, props
  and people are drawn where they land, its doors and inns faintly, so a designer can see what a
  town covers before putting it there.
- A place's own ways out cannot lead back into the map it is stamped on: an arrival is written in
  the scene it leaves from, and a place does not know where it was put down. A door from a town
  into a hut and back again works; a door into the open world beside the town does not.
- Roads and rivers, which are what make a country's places feel joined rather than scattered.
- Going from a stamp to the world view and back is how a designer moves about 100 km.

### 24.6 Playing in it together

- The host sends each player **what is near them**, not everything in the map they are in: an area
  of interest by distance, with chunks acknowledged once and only changes after that.
- The wire changes with it: a snapshot carries a body's absolute position today, and it comes to
  carry a `Spot`. That is a protocol version, and the beacon (§23) says which version it is.
- **Sound** is placed in absolute world pixels, listener and emitters alike. It rebases with the
  drawing, or the two disagree.
- A client keeps the chunks around itself, made from the same seed as the host's, so joining a
  game does not ship a world over the wire.

### Milestones

M10 is this section, and it splits by what needs chunks and what does not.

- **First, on the maps that exist today**, changing no file format: §24.2 (drawing only what is
  seen — **built**), then §24.3's collider grid and its path budget (**built**). Each of these is a fault the engine
  already has; meadow is simply small enough to hide them.
- **Then the world itself**: §24.4 (chunks, made land, `Spot`), and with it the rest of §24.3 —
  crowds, blows, snapshots and distant people, all of which are keyed on chunks that do not exist
  until §24.4 does.
- **Then** §24.5 (authoring) and §24.6 (company).
- **The size is chosen last, not first.** Everything above is the same work for four kilometres
  or a hundred; what the chosen size decides is only whether §24.4 ends with absolute positions
  or with `Spot` (§24.0). A game that turns out to want ten kilometres pays for none of the
  hundred-kilometre machinery, and a game that later wants a hundred does not start again.

### Known gaps and risks

- Made land is not land *worth walking to*. This says where the tiles come from, not what is out
  there; that is the game's problem, and §21 (people's days and lives) is the start of the answer.
  It is also the real argument for ten kilometres over a hundred: the engine will carry either,
  and the smaller one is the one a studio this size can fill.
- Made land has to agree exactly on every machine, or players fall through different rocks.
  Whole-number generation is the plan, and it has to be tested against itself on both platforms.
  Agreeing on the rocks is only half of it: the physics that slides a body along them is `f32`
  throughout, and a client predicts with it. That is true today and harder at 1 600 000 px.
- Streaming and the world simulation meet at a seam: a villager whose chunk is not resident must
  still eat, sleep and be where they should be when it is.
- The crowd, the blows and the snapshots are quadratic today. They want fixing before the world
  grows, not after, or the first large world is blamed for a fault that is already there.
- None of this helps one enormous room: the budget is what is near a player, and a thousand
  enemies standing together is still a thousand enemies.

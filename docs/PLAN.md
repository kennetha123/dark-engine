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

## 8. Risks

- **World simulation scope** is the biggest risk. M3 is headless and text-only on purpose.
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
- Drawing order with height: raised tops are floors in `layer::TERRAIN + level`, below everything
  standing on them; a north cap (the strip of top someone behind the plateau can overlap) and the
  cliff faces are drawn in the world layer, sorted by the north and south edges.
- Characters (`SpriteKind::Character`) draw only their opaque pixels; the game draws a blob shadow
  (`SpriteKind::Blob`) on the surface below, which stays down and shrinks when jumping. The pack's
  baked shadows are too faint to read as a jump shadow.
- Silhouettes: sprites drawn after, and overlapping, a character write their compact rank into an
  R16Float occlusion mask (max blend, soft shadows excluded); covered character bodies then draw a
  shaded blue silhouette where the mask holds a later rank. Works for any number of characters.
- Known gaps: plateau sides are only a rim line; no slopes/stairs; collider lookup is a linear scan
  per map (fine at hundreds, needs a grid at thousands); a height difference above one tile shows
  only one tile of north cap.

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
- Known gaps: characters pass through each other (no body-body collision); NPCs do not walk yet;
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
- Known gaps: no air attacks, projectiles, AoE, aiming or lock-on; no gamepad, so no rumble; no flow fields
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
- Replication: snapshots carry the tents and fires in the recipient's map and the recipient's own
  body and pack (nobody else's). The HUD shows six need gauges (fuller is worse), the air's
  temperature tinted by how it feels, statuses, and the hotbar with the worn item outlined.
- Adventurer: bread, water, ale, a wool cloak (worn), a tent, firewood, a warming charm and soap;
  spring climates (the grove is cold at night without the cloak); the Meadowbrook inn is the
  Town01 house; the tent and campfire are placeholder art drawn by
  `placeholder/make_life_art.py`. `--autopilot-camp` pitches camp and gets drunk for screenshots.
- Scattered props now also keep 12 px out of hand-placed props' footprints (a house).
- Known gaps: NPCs have no bodies or schedules yet (the model is shared, only players get one);
  no weather or seasons (climate by hour only); no social reaction to soiling or drunkenness;
  items cannot be found, bought or dropped (storylets can give them); no interiors; relieving
  yourself anywhere is fine.

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
- Known gaps: text edits are not undone (Ctrl+Z in a field undoes typing); ids cannot be
  renamed once made; the editor's own interface is in English only; a sheet saved again leaves
  its old picture on the GPU until the editor closes (the renderer frees no textures).

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

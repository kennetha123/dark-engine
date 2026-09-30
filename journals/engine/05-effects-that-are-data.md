# Journal engine/05 — Effects that are data

Status: Materialized.
Date: 2026-09-27. Depends on: `journals/engine/04` (the mesh pass and the sorted stream).

## Goal

Give the game particle effects that are **written as data and tuned without a compiler** — rain,
snow, smoke, sparks, dust — and give the editor somewhere to tune them. Not a node graph.

## Current state (evidence, verified 2026-09-27)

There are no effect tools at all.

| Piece | Where | What it is |
|---|---|---|
| Sparks | `apps/dark-player/src/fx.rs` | A `Vec<Spark>` of position, lift and lifetime, hand-written |
| Hit feel | same | Hitstop, flashes, screen shake, sound triggers — all Rust |
| Materials | `crates/dark_render/src/sprite.rs` | Five hardcoded modes: `PLAIN`, `BLOB`, `BODY`, `CIRCLE`, `RECT` |
| Lighting | `crates/dark_render/src/model.rs` | One directional light, models only. Sprites are unlit |
| Weather | — | Nothing. No rain or snow anywhere in the workspace |

Every effect is Rust that has to be recompiled to change a number.

**What the renderer can already carry.** `Sprite` has `color` (an RGBA multiplier) and `lift`,
but **no rotation and no free scale** — `repeat` tiles a texture rather than scaling it. `Mesh`
has arbitrary vertex positions, uvs and colours, and already interleaves with sprites in the
sorted stream by `layer` and `sort_y` (`build_batches`). So a particle that turns or grows has to
be a mesh, not a sprite.

## Decisions

**A particle system, not a shader graph.** A node-graph material editor is a graph model, an
editor, WGSL codegen, hot reload, a preview and a material system underneath — months, against a
game that needs making. The five hardcoded draw modes are already a small fixed-function material
system; growing *that* into a named, parameterised set is the cheap 80%, and it is where §16's
toon ramp and outlines belong. Recorded here so the choice is not made again by accident.

**An effect the world reacts to is not an effect.** The campfire's warmth is `life.ron` and lives
in the simulation; the flame is an effect and lives in presentation. Rain making you cold is a
climate, not a particle. This is what keeps rule 1 true and keeps effects free to use frame delta
when gameplay may not (rule 3).

**Effects are not networked and not deterministic.** Two players seeing different raindrops is
not a problem, and building determinism for something nobody can check would be machinery for its
own sake. Each emitter carries its own seed so a *replay* of one frame is stable; nothing more is
promised.

**Particles are drawn as meshes.** One quad each, batched into one mesh per effect, sorted where
the emitter stands. That buys rotation, scale and per-particle colour for free, and it rides the
stream `journals/engine/04` already built. One mesh per effect means one `sort_y` per effect,
which is right for a thing at a place; weather sits on a high layer instead.

**`EffectDef` lives in `dark_fx`, not `dark_assets`.** Sheets, skeletons and models are in
`dark_assets` because the simulation reads them — clip lengths drive attack recovery. **Nothing
in the simulation ever reads an effect**, so putting them there would carry presentation data
through every sim crate for nothing.

## Architecture

A new presentation crate, `dark_fx`. It is added to `FORBIDDEN` in `tools/check-sim-deps.sh`, as
`dark_model` was, so no simulation crate can ever reach it.

```
crates/dark_fx/
  src/lib.rs     EffectDef, Range, Fade — what a `*.fx.ron` says
  src/live.rs    Effects, Emitter, Particle — what is running
```

**What a `*.fx.ron` holds.** Flat and small, so the editor can show it as a form and write it
back:

```ron
(
    sheet: "sheets/weather.sheet.ron",
    frames: [0, 1, 2],
    rate: 90.0,                  // born a second; 0 with `burst` is a one-shot
    burst: 0,                    // born at once when it starts
    life: (0.8, 1.4),            // seconds, a range
    area: (320.0, 24.0),         // half-extents of where they are born
    lift: (180.0, 220.0),        // how high they start
    drift: ((-6.0, 6.0), (-260.0, -220.0), (10.0, 30.0)),  // pixels a second: x, lift, y
    gravity: -40.0,              // on lift
    drag: 0.2,
    size: (1.0, 1.0),            // scale at birth and at death
    spin: (-2.0, 2.0),           // turns a second
    colour: [(1,1,1,1), (1,1,1,0)],
    layer: 100,
)
```

**What runs.** `Effects` holds live emitters; each owns its particles and its seed.

```rust
pub struct Effects { live: Vec<Emitter>, next: u64 }
pub struct Handle(u64);

impl Effects {
    /// A one-shot at a place: sparks, a footfall.
    pub fn burst(&mut self, def: &EffectDef, at: Vec2) -> Handle;
    /// One that keeps going until stopped: weather, a campfire.
    pub fn standing(&mut self, def: &EffectDef, at: Vec2) -> Handle;
    pub fn move_to(&mut self, handle: Handle, at: Vec2);
    pub fn stop(&mut self, handle: Handle);
    /// Advances every emitter. Frame delta, because nothing depends on it.
    pub fn update(&mut self, defs: &dyn Fn(Handle) -> &EffectDef, dt: f32);
    /// Appends one mesh per emitter, ready for `Renderer::render_scene`.
    pub fn meshes(&self, ..., out: &mut Vec<Mesh>);
}
```

A particle is `{ at: Vec2, lift: f32, drift: Vec3, age, life, frame, spin }`. Nothing about it
touches the world: no collision, no query, no callback.

**Bounded by construction.** An emitter carries a hard cap on live particles. A rate and a
lifetime that would exceed it drop new births rather than growing without limit, so a bad number
in a file cannot take the frame rate with it.

## Execution phases

1. **The crate and its data.** `dark_fx`, `EffectDef`, the runtime, its tests, the guard entry
   in `check-sim-deps.sh`. **`dark-cli preview-fx`** draws an effect across several moments into
   one picture, the way `preview-model` does, so it can be checked by looking.
2. **Weather in the game.** Region, season and hour choose an effect. This is the biggest thing a
   player will feel in a year-long game, and it is the first real customer.
3. **An Effects workspace in the editor.** A form and a live preview, reusing the offscreen
   renderer. **This is the tool** — not a graph, a parameter panel that shows the result.
4. **The sparks move over**, and `fx.rs` stops being hand-written particles.
5. **Lights** — a campfire glow, a torch. Needs additive drawing, which pairs with the next one.
6. **Named materials with parameters**, absorbing §16's toon ramp and outlines.

Phase 1 is this entry's execution. The rest are separate.

## Verification

- The four gates in `AGENTS.md`.
- `check-sim-deps.sh` must refuse `dark_fx` in a simulation crate — proved by putting it in one
  and watching the check fail, as `dark_model` was.
- Unit tests: a rate births the right number over a second; a particle dies at its lifetime; the
  cap holds; colour and size reach their end values; a one-shot ends and a standing one does not.
- `preview-fx` on a real effect, looked at.

## Files this entry will touch

| Phase | Files |
|---|---|
| 1 | `crates/dark_fx/` (new), `tools/check-sim-deps.sh`, `apps/dark-cli/src/preview_fx.rs` (new), `apps/dark-cli/src/main.rs`, root `Cargo.toml`, `docs/PLAN.md` |

## Risk & rollback

- Phase 1 is additive: a new crate and a new command. Nothing existing changes, so it can sit
  unused.
- The real risk is later: phase 4 touches `apps/dark-player/src/demo.rs`, which another session
  is editing heavily. It waits until that lands.

## Phase 2 in detail — weather (planned 2026-09-27)

The one line above is not enough to build from. The design, worked out against what already
exists, before executing it.

### What is already there

| Piece | Where |
|---|---|
| Seasons of the year, with an `id` | `dark_sim::CalendarDef::season(day)` — day counts from 1 |
| The region a point stands in | `dark_assets::SceneDef::region_at((x, y))` |
| Day and hour, replicated | `Frame::time` — a **zero-based** day and an hour in `0..24` |
| A region's air through the day | `dark_life::Climate`, warmed by `CalendarDef::warmth(day)` |
| A full-screen tint | `Demo::rect`, as `draw_fade` uses it |
| Interiors | **None.** "There are no interiors yet" — every map is under the sky |

So the only things missing are *which sky a place has* and *what that sky draws*.

### Decisions

**The calendar decides the weather, and there is no seed.** A sky is a pure function of
`(day, hour, region)` and the project's own data — a hash of the spell and the region's name,
picking from weights the designer wrote. No roll is stored, nothing is networked, and every
client that holds the same project (which the handshake already checks) sees the same sky by
construction.

The cost is that day 47 in the Marches is wet in *every* year, not just this one. That is the
right trade here and not only an expedient one: the calendar already fixes the seasons, the year
*is* the content, and a player who learns that the rains come in the ninth week has learned
something about the world. The alternative — mixing in the world seed — needs the seed on the
client, which means the handshake, which means `dark_net::protocol`, and a protocol change buys
variety nobody asked for. If it is ever wanted, it is one hash input.

**The sky is world truth and lives in a simulation crate.** `WeatherDef` goes in `dark_sim`
beside `CalendarDef`, in `world.ron`. Only the *picture* of a sky — which `*.fx.ron` it draws,
how it tints the light — is presentation, and that lives in the project's `fx/weather.ron`, read
by `dark_fx`.

This is the opposite of how phase 1's `EffectDef` was placed, and deliberately. Nothing in the
simulation reads an effect; but *whether it is raining* is exactly the kind of thing the
simulation will want, the moment rain is to make anyone cold or wet. Putting the roll in
presentation would make that a seam between two files a designer has to keep in step by hand.
Putting it in `dark_sim` means `life.ron` can one day read the same answer. Nothing in the
simulation reads it today, and that is fine — the placement is about which side of rule 1 the
*fact* belongs on, not about who happens to ask first.

**A spell, not a moment.** The day is cut into spells of a few hours; a spell gets one sky.
Weather that could change every frame is not weather.

### Shapes

`world.ron` gains, in `dark_sim`:

```ron
weather: (
    spell_hours: 6,
    // The first line that fits wins, in the order they are written: the particular ones go at
    // the top and a line naming neither season nor region goes last. Plainly first-come rather
    // than most-particular-wins, because there is no honest answer to whether a season should
    // outrank a region. A sky not named anywhere is clear.
    chances: [
        (season: "winter", region: "marches", skies: {"snow": 4, "clear": 3}),
        (season: "winter", skies: {"clear": 5, "rain": 2}),
        (skies: {"clear": 6, "rain": 2, "fog": 1}),
    ],
),
```

`fx/weather.ron`, read by `dark_fx`:

```ron
(
    change_secs: 6.0,
    skies: {
        "rain": (effect: "fx/rain.fx.ron", tint: (0.72, 0.76, 0.88, 0.22)),
        "snow": (effect: "fx/snow.fx.ron", tint: (0.86, 0.90, 1.0, 0.14)),
        "fog":  (tint: (0.78, 0.80, 0.82, 0.30)),   // a sky with no particles at all
    },
)
```

`"clear"` is never listed: it is the absence of a sky, so a project says nothing to get one.

### The player

A new `apps/dark-player/src/weather.rs` holds all of it — which sky is overhead, the standing
emitter, the cross-fade — and `demo.rs` gains only a field, a load and a draw call. **That is on
purpose**: another session is editing `demo.rs` heavily in this same tree, and three small
insertions can be committed on their own where a large edit could not.

The emitter stands on the camera and is moved to it every frame, and the file's `area` is
authored to cover the screen at the project's resolution. A sky that changes stops the old
emitter — its last drops fall — and starts the new one, with the tint sliding across
`change_secs`.

`--weather <id>` forces a sky, so a screenshot can be taken of one.

### Verification

- The four gates, and `check-sim-deps.sh` still passing with `WeatherDef` in `dark_sim`.
- Unit tests on the roll: the same day, hour and region always give the same sky; weights are
  respected over a year; the most specific line wins; a spell holds for its hours and then may
  change; no chances at all is always clear; day 1 is the first day (the off-by-one between the
  clock's zero-based day and the calendar's first).
- `--autopilot --weather rain --screenshot`, looked at.

### Not in phase 2

- **Sound.** Rain should be audible and is not. It needs FMOD events authored in the project.
- Weather the world reacts to: being cold or wet in the rain. That is `life.ron` reading
  `WeatherDef`, and it is its own piece of work.
- Wind that moves anything but particles; lightning; puddles; snow that lies.
- Interiors, because there are none.

## Phase 3 in detail — the Effects workspace (planned 2026-09-27)

The entry calls this **the tool**. One line is not enough to build a tool from.

### What is already there

| Piece | Where |
|---|---|
| A workspace is an enum variant, a tab, a dispatch arm, two undo arms and a module | `editor.rs` |
| The closest model: a `.ron` editor with undo, a picture and a save | `sheets.rs` |
| Undo that groups a drag into one step | `widgets.rs` — `Tracker` |
| Listing a project's files of a kind | `catalog::sheet_files` |
| **An offscreen `dark_render::Renderer` already shown inside egui** | `viewport.rs` |

### Decisions

**The preview runs the game's own code.** Not egui shapes approximating particles: the editor
runs `dark_fx::Effects` and hands its `Mesh`es to `dark_render`, offscreen, exactly as the player
does — the same blending, the same ordering, the same rounding. A tool whose whole promise is
*this is what you will see* cannot afford a second implementation that drifts. `viewport.rs`
already proved the pattern for maps; effects reuse its renderer rather than raising a second one,
because only one workspace draws in a frame and the map is not one of them while this is open.

**Tuning does not restart the effect.** Dragging `rate` should change the fall you are watching,
not start it again from nothing — otherwise every drag tick is a fresh empty screen and the
number cannot be judged. So `dark_fx` gains `Effects::retune`, which swaps an effect's definition
under its live particles.

**The form is the project's ordinary form.** `Tracker` number fields, as the sheet and database
editors use, so undo, redo and the one-step-per-drag behaviour are the ones a designer already
knows here, and Ctrl+Z means the same thing in every workspace.

**Saving loses the comments**, exactly as `save_sheet_def` already does and says. An effect file
is numbers; the prose about *why* belongs in `journals/` and `docs/PLAN.md`, not beside a
`gravity:` an artist is about to drag.

### Shapes

```
apps/dark-editor/src/effects.rs   EffectEditor: the list, the form, the preview, save
```

`dark_fx` gains two things, both small:

```rust
/// Swaps an effect's definition under its live particles.
pub fn retune(&mut self, kind: EffectId, def: EffectDef, art: Art) -> Result<(), EffectError>;
/// Writes a `*.fx.ron` back. Comments in the file are not kept.
pub fn save(&self, project: &Project, path: impl AsRef<Path>) -> Result<(), EffectError>;
```

`viewport.rs` gains two accessors so the workspace can borrow its renderer:

```rust
pub fn texture(&self, sheet: &str) -> Option<TextureId>;
pub fn render_meshes(&mut self, egui, size, camera, clear, meshes: &[Mesh]);
```

And `catalog::fx_files` lists `fx/*.fx.ron` as `sheet_files` lists sheets.

### The panel

Left: the project's effect files, and a field to make a new one. Middle: the form, every field of
`EffectDef` with the range it may take. Right: the preview — the effect playing, with

- **a background** to choose, because a pale particle on a pale background cannot be judged, and
  the renderer's `clear` colour gives it for free;
- **play, pause and burst**, so a one-shot can be watched as a one-shot;
- **what it costs**: how many are alive against the file's `cap`, so a file asking for more than
  it is allowed says so rather than quietly thinning out.

### Verification

- The four gates, and the commit built out of the index in a worktree.
- Unit tests: `retune` keeps the particles that are alive and changes what the next ones do; it
  refuses art that does not match, leaving the effect as it was rather than half-changed; a
  round trip through `save` and `load` gives back the same definition.
- A scripted editor run into the workspace, screenshotted and looked at.

### Not in phase 3

- Editing `fx/weather.ron` — which sky draws which effect is its own screen.
- A curve editor. `Fade` is two values and a straight line between them; if that is ever not
  enough it is a change to the data, not to the form.
- Making art. The frames come from a sheet, and sheets have a workspace already.

## Phase 5 in detail — night, and the lights that push back (planned 2026-09-30)

The entry's phase 5 reads "**Lights** — a campfire glow, a torch." Investigating it turned up the
thing that line assumes and the game does not have.

### What is already there, and what is not

**There is no night.** The world is the same colour at midnight as at noon: screenshots taken at
hours 0, 6, 12 and 18 in `journals/engine/05` phase 2 came out *byte-identical* apart from the
weather. Shipping lights on their own would be shipping something invisible — a glow against a
fully lit field is nothing. So phase 5 is two things: a night, and the lights that push back
against it.

| Piece | Where | What it gives |
|---|---|---|
| `Renderer::pipeline` takes a blend state | `dark_render/src/lib.rs` | Another blend mode is a parameter, not a rewrite |
| A non-standard blend already exists (`max`, for the occlusion mask) | same | The precedent, and where to put it |
| `MODE_BLOB` — a filled ellipse over a quad | `sprite.wgsl` | A light is this with a soft edge |
| Passes: world → mask → silhouettes → **interface** → blit | `render_with` | Where a light pass goes |
| `layer >= layer::UI` splits world from interface | `sprite.rs` | So night lands over the world and under the writing |
| The hour, replicated | `Frame::time` | What decides how dark it is |
| Campfires, with where they stand | `Frame::structures` | What holds a light |

### Decisions

**Darkness multiplies, light adds, and there is no light buffer.** The usual 2D answer renders
lights into an offscreen target and composites. This does not need one: a full-screen quad that
*multiplies* the world by the night's colour, then each light as a quad that *adds* to it. Two
blend states over the existing instance machinery, no second target, no composite pass, and one
more reason the pipeline builder already takes its blend as an argument.

**A light is a sprite mode, not a new kind of thing.** `mode::LIGHT` is `MODE_BLOB` with a soft
radial falloff. No new vertex format, no new buffer, no new batching — the same trick that let a
particle be a mesh in phase 1. A light carries its colour in `Sprite::color` and its reach in the
quad's size, which is what every other mode already does.

**Between the world and the interface.** The night pass runs after the world and before the
overlay batches, which is where `layer::UI` already cuts. So the HUD stays readable at midnight,
and phase 2's weather wash — drawn on `layer::UI` — lies over the night rather than under it,
which is right: fog is between you and the world, so it is not something a campfire burns through.

**Lights are presentation, and this is the same rule as everything else in this entry.** A
campfire's warmth is a climate in `life.ron` and lives in the simulation; its glow is this and
does not. Nothing in the simulation reads a light. If *seeing* in the dark ever becomes a rule of
the game — a torch that lets you find your way — that is a value in the simulation, and this
stays the picture of it.

**The night is data, and it belongs in `fx/weather.ron`.** That file already says what the sky
looks like; how dark the sky is at three in the morning is the same question. It gains a `night`
curve of hour → colour, so a project says what midnight is rather than the engine deciding.

### Shapes

`fx/weather.ron` grows one field:

```ron
(
    change_secs: 6.0,
    // What the world is multiplied by, through the day. Between the hours given, evenly;
    // white is full daylight, and the darkest entry is midnight. Wraps around the day.
    night: [
        (0.0,  (0.24, 0.26, 0.42)),
        (5.0,  (0.30, 0.31, 0.46)),
        (7.0,  (0.85, 0.82, 0.78)),
        (17.0, (1.00, 0.97, 0.90)),
        (20.0, (0.55, 0.45, 0.45)),
        (22.0, (0.28, 0.29, 0.45)),
    ],
    skies: { … },
)
```

`dark_render::Scene` gains two fields, both of which an empty frame may leave alone:

```rust
/// What the world is multiplied by before the interface is drawn. White is daylight.
pub darkness: [f32; 3],
/// Drawn additively over the darkened world: `SpriteKind::Light`.
pub lights: &'a [Sprite],
```

### The player

`apps/dark-player/src/weather.rs` already owns the sky and already has the hour, so it owns this
too: `Weather::darkness(time)` reads the curve, and the view places a light on every campfire that
is burning. One more field on the `draw` call, not a new system.

### Verification

- The four gates, and the commit built out of the index in a worktree.
- Unit tests on the curve: it wraps around midnight; between two hours it is between their
  colours; a project with no `night` is daylight all day, so nothing changes for a game that does
  not ask for one.
- Screenshots at several hours — **which are currently byte-identical and must stop being so** —
  and one of a campfire at midnight, which is the whole point of the phase.

### Not in phase 5

- Lights that cast shadows, or that anything occludes. A light here is a glow, not a light source.
- A light on anything but a campfire: a torch to carry is an item, and items are their own work.
- Night in the editor's map view. The Effects workspace's backgrounds already stand in for it.
- The weather darkening the day further (a storm at noon). The field is there to grow into.

## Non-goals

- A node-graph shader editor. See Decisions.
- Particles that collide, or that the simulation can see.
- Networked or deterministic effects.
- Additive blending in phase 1 — `Mesh` draws alpha-blended, and a second pipeline is phase 5.

## Execution log

### Phase 1 — the crate, its data and `preview-fx` (2026-09-27)

Built, with the four gates green and `docs/PLAN.md §16.6` written in the same change.

**What landed.** `crates/dark_fx` (`lib.rs`: `EffectDef`, `Range`, `Fade`, `Drift`;
`live.rs`: `Effects`, `Emitter`, `Particle`, `Art`), 21 tests; `dark_fx` in `FORBIDDEN` in
`tools/check-sim-deps.sh`; `apps/dark-cli/src/preview_fx.rs` and the `preview-fx` command.
In the test project: `placeholder/make_fx_art.py` (white particle frames, so each file's own
colour does the tinting), `sheets/fx.sheet.ron`, `fx/rain.fx.ron`, `fx/smoke.fx.ron`.

**Where it differs from the plan above, and why.**

- **`Effects` owns a catalogue.** The sketch had `update(defs: &dyn Fn(Handle) -> &EffectDef, dt)`,
  which makes every caller keep a second map alive and answer for a handle it may no longer hold.
  Instead `add(def, art) -> EffectId` teaches it once and `burst(id, at)` / `standing(id, at)`
  play it. `update(dt)` then takes only the time, which is all it should need.
- **`Art` is handed in; the crate never loads a picture.** `dark_fx` reads a `*.fx.ron` (through
  `dark_assets::Project`) but nothing else: whoever put the sheet on the GPU passes the page, its
  size in texels and one rectangle per frame the file names. That is what let the whole runtime be
  tested without a GPU, and it is what `preview-fx` and the game will both hand in.
- **`Fade { from, to }` and `Drift { x, lift, y }` are named, not tuples.** The sketch wrote
  `colour: [(1,1,1,1), (1,1,1,0)]` and `drift: ((..), (..), (..))`. Named fields let a file say
  `drift: (lift: (-300.0, -280.0))` and nothing else, which is all falling rain needs, and they
  are what the editor's form in phase 3 will bind to.
- **`lands: true` was added.** Not in the plan: the first rain preview showed drops falling
  straight through the ground and on out of the picture. A particle that reaches the ground is
  gone — rain lands, smoke does not.
- **`deny_unknown_fields`**, for the reason `dark_assets::model::ModelMeasure` has it: nearly
  every field here has a default, so `color` for `colour` would be read as *no colour* and an
  artist would sit there tuning a number nothing reads.
- **`preview-fx` starts a fifth of the way in**, not at zero. A standing effect has nothing at
  all at zero, and a blank panel says nothing.

**Verified.**

- The guard: `dark_fx.workspace = true` added to `dark_time` — `check-sim-deps.sh` failed, naming
  `dark_fx` in five sim crates. Restored, passes. (The first try put it in `dark_sprite`, which
  cargo refuses as a dependency cycle before the guard ever runs.)
- Ten sabotages, each against the test that pins it, all caught: the carried fractional births,
  frame-rate-independent drag, `lift` raising the quad, `lands`, the cap, `spin`, per-emitter
  seeds, `deny_unknown_fields`, the size fade, and `add` checking a def built in memory.
- `preview-fx` on both files, looked at. Smoke reads as a plume — narrow at the fire, spreading
  and fading as it climbs; rain fills the panel and lands. The smoke was tuned twice *in the file*
  between renders, which is the whole point of the phase.
- `cargo fmt --all`, `cargo clippy` (clean for `dark_fx` and `dark-cli`), `cargo test --workspace
  --exclude dark-player` all green, `check-sim-deps.sh` passes. `dark-player`'s `pad.rs` test did
  not compile, which this entry first put down to another session's work in the same tree.
  **That was wrong**, and phase 2 found out why: `dark_model` had put `serde_json` in
  `dark-player`'s tree through `gltf`, so `Vec::new()` no longer inferred. My own change, blamed
  on somebody else because it appeared in a file they were editing. Fixed in
  *Put back what was not mine to commit*.

**Still owed, and unchanged by this phase:** phases 2–6 below. Added to the list: `dark-cli
package` and `Project::fingerprint` do not know about `*.fx.ron`, exactly as they do not know
about models. Neither is reachable from a scene yet; both come due together.

### Phase 2 — weather (2026-09-27)

Built to the design in *Phase 2 in detail* above, with the four gates green and `docs/PLAN.md
§16.7` written in the same change. It rains in the game.

**What landed.** `dark_sim::WeatherDef` / `SkyChance` / `CLEAR` in `world.ron`, with
`WeatherDef::at` and `spell`, validated when the world is built (7 tests);
`dark_fx::WeatherArt` / `Sky` reading `fx/weather.ron` (3 tests);
`apps/dark-player/src/weather.rs`, the whole feature — which sky, the standing emitter, the
cross-fade, the wash (4 tests); `--weather <sky>`. In the test project: a `weather` block in
`world.ron`, `fx/weather.ron`, `fx/snow.fx.ron`, and `AGENTS.md` says what the flag does.

`demo.rs` gained six lines: two fields, two constructor lines, a setter and one call. That was
the point of putting the feature in its own file, and it held.

**Where it differs from the design above.**

- **The first line of `chances` that fits wins, full stop.** The design said "a season and a
  region, then a season, then the rest", and writing it revealed there is no honest answer to
  whether a season outranks a region — only a rule nobody could predict. Plain first-come is what
  the prose had said all along; the ranking went.
- **The region is the map's own, not the spot stood on.** `SceneDef::region_at`, which would
  give a stamped town its own sky on a world map, is not on this branch yet — it is in another
  session's working copy. `SceneDef::region` is, so that is what this uses, and the difference is
  one argument when the other lands. The meadow declares its own region, so the screenshots below
  are byte-identical either way.
- Nothing else. The shapes are as sketched.

**Verified.**

- Eleven sabotages, each against the test that pins it. **Three passed on the first run and had
  to be strengthened**, which is the whole reason for doing it:
  - *the same moment always has the same sky* asserted exact answers that came from picking a
    line, not from the hash — it could not see the region being dropped out of it. It now asserts
    that two places on the same line differ somewhere in eighty days, and that one place's sky
    changes within a day.
  - *a world with no weather is clear* returned `clear` before ever reading `spell_hours`, so it
    could not see `Default` handing out zero hours. (That bug was real and was caught by every
    *other* test failing.) It now checks the default spell length and that such a world builds.
  - *the clock's first day is the calendar's first day* compared two calls that `saturating_sub`
    and a season-less calendar made identical — dropping the `+ 1` changed nothing. It now uses a
    season beginning on the second day, so the two sides of the off-by-one land in different
    seasons.
- `--autopilot --weather rain|snow|fog --screenshot`, all three looked at: rain falls across the
  screen under a cool wash, snow drifts and turns, fog is a pale wash with no particles at all,
  and the interface stays clean above all of it.
- And **without** the flag, at four hours of day one in Centreau: three spells clear and one
  raining, chosen by the calendar. The three clear ones are pixel-identical to each other, which
  is also how it is known that a clear sky costs nothing.
- `preview-fx` on the new snow.
- The four gates: `cargo fmt --all`, `cargo clippy --workspace --all-targets -D warnings` clean,
  `cargo test --workspace` 45 result blocks all green, `check-sim-deps.sh` passes with
  `WeatherDef` in `dark_sim`.

**What this phase turned up on the way, which matters more than the weather.** Checking the
staged tree in a worktree — rather than trusting that a commit built because the working tree did
— showed that **`HEAD` had not compiled since `6fdafcb`**. Three of my commits this session staged
shared files whole while another session was editing the same tree, and swept in halves of their
work: `dark_view` drawing a scene's `floors`, `dark-cli` dispatching `preview-world`, the assets
smoke test reading `dark_life::Slot`. A fourth failure, `pad.rs`'s `Vec::new()`, this entry had
twice blamed on them and was mine: `dark_model` put `serde_json` in `dark-player`'s tree through
`gltf`. All four are fixed in *Put back what was not mine to commit, and build again*.

Two lessons, in order of importance:

1. **A tree that builds is not a commit that builds.** Every commit from here is checked out of
   the index into a worktree and built there before it is made. Nothing else can see this, and in
   a shared tree it is not an edge case — it happened three times in one session.
2. Staging a shared file whole is the mistake, every time. Only hunks.

### Phase 3 — the Effects workspace (2026-09-27)

Built to the design above. The editor has a sixth tab, and an effect can be made, tuned and
saved without touching a file by hand — which is what this entry set out to be able to say.

**What landed.** `apps/dark-editor/src/effects.rs` (the list, the form, the preview, save);
`Viewport::render_meshes` and `Viewport::texture`, so the preview goes through the renderer the
map view already uses; `catalog::fx_files` and `fx_name`; the workspace wired into `editor.rs` in
eleven small insertions. In `dark_fx`: `Effects::retune`, `EffectDef::save`, `EffectDef::plain`,
and a hand-written `Serialize` for `Range`. `docs/PLAN.md` §16.8, and §18's list of workspaces.

**Where it differs from the design above.**

- **`Range` needed a `Serialize` of its own**, which the plan did not foresee. It reads as a
  number *or* a pair and the derived form writes a struct — so the first file the editor saved
  would have been a file it could not open again. Found by writing the round-trip test the plan
  did ask for, before any of the editor existed.
- **The backgrounds had to be converted.** The renderer's `clear` is a colour in light, not in
  sRGB, so the first run painted "Dusk" a pale lavender. Found by looking at it. `linear()`
  converts, and a test pins middle grey.
- **The preview's frame length is taken during layout**, not in `render`, which runs after the
  interface is built and has no context to ask. Clamped, so a window that was not drawn for a
  while does not advance an effect by a minute when it comes back.
- Otherwise the shapes are as sketched.

**Verified.**

- Six sabotages, each against the test that pins it, all caught: the `Range` round trip, the
  starting effect drifting from the file defaults, a retune that throws away what is in the air,
  a retune that half-applies a definition its art does not fit, a background handed over unconverted,
  and backgrounds too alike to tell apart.
- Scripted editor runs, looked at: the tab, `rain` and `smoke` playing against Dusk, and the whole
  loop of **making** an effect through the interface, **dragging** a number and **saving** it with
  Ctrl+S — the file came out with `rate: 105`, and `dark-cli preview-fx` loaded it, which is the
  proof that what this workspace writes is what the game reads. The test file was then removed.
- The four gates, and the commit built out of the index in a worktree.

**Still owed:** phases 4–6, and `fx/weather.ron` still has no screen — a designer can tune what an
effect looks like but must still write by hand which sky plays it.

### Phase 5 — night, and the lights that push back (2026-09-30)

Built to the design above. `docs/PLAN.md` §16.9.

**What landed.** `dark_render`: `SpriteKind::Light`, `fs_light` and `fs_darkness`, a darkness
pipeline (multiply) and a light pipeline (add), and a `night` pass between the world and the
interface. `dark_fx::WeatherArt::darkness` reads a `night` curve from `fx/weather.ron`.
`apps/dark-player/src/weather.rs` works out the hour's darkness and puts a flickering glow on every
campfire still burning. The test project got a night.

**Where it differs from the plan above.** Nowhere in shape — but twice in what the numbers mean:

- **`darkness` is a multiplier on light, and I first wrote the project's curve as though it were a
  colour.** `0.72` at seven in the evening looked like no night at all, because 0.72 of the light
  reads as 0.86 of the brightness. The second time this crate has caught me on that (the editor's
  backgrounds in phase 3 were the first), so it is now in the doc comment on both sides, with the
  number a designer actually wants written next to it.
- **The glow was far too strong at first** — 0.85 blew the middle out to white and took the fire's
  own picture with it. An additive pass wants a small number: 0.30.

**Verified.**

- Screenshots at hours 2, 12 and 19, **which used to be byte-identical and no longer are**: the
  average colour of the world goes (40, 52, 16) at first light, (130, 146, 115) at noon, (53, 52,
  13) at dusk.
- `--autopilot-camp --hour 20`, looked at: a warm pool of light around the fire, the tent and the
  fire both readable inside it, and the dusk intact around it.
- Four sabotages. **Two passed at first and had to be strengthened**: the night curve's test used
  a curve beginning at hour 0, so it could never ask what happens *before* the first entry — which
  is the whole of the night and the only place the wrap shows; and the light-occlusion test had
  the glow sorted *before* the character, so a plain sprite in its place would not have hidden
  them either. Both now fail when broken.
- The four gates, and the commit built out of the index in a worktree.

**Found on the way, not a bug.** `--hour 2` does not start the game at two: a lone player is
asleep then, the night passes at once, and the clock is at the waking hour. Worth knowing before
somebody hunts it.

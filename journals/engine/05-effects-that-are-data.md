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
  --exclude dark-player` all green, `check-sim-deps.sh` passes. `dark-player`'s `pad.rs` test does
  not compile — that belongs to another session's uncommitted work in this tree, not to this
  change, and nothing here touches `dark-player`.

**Still owed, and unchanged by this phase:** phases 2–6 below. Added to the list: `dark-cli
package` and `Project::fingerprint` do not know about `*.fx.ron`, exactly as they do not know
about models. Neither is reachable from a scene yet; both come due together.

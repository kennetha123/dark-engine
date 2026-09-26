# Journal engine/04 — The world becomes 3D

Status: Materialized.
Date: 2026-09-26. Depends on: `docs/PLAN.md` §3 (stack), `journals/engine/00`, `journals/engine/01`.

## Goal

Change the art direction from 2D sprites to 3D characters and terrain, in the manner of Tree of
Savior: real meshes, flat banded shading, hard outlines, hand-painted textures. Do it without
touching the simulation.

## Current state (evidence, verified 2026-09-26)

**What the renderer is today.** `dark_render` is a sprite renderer: `Sprite`, `SpriteKind`
(`Plain`, `Character`, `Blob`, `Outline`), `Mesh`/`MeshVertex`, and two shaders — `sprite.wgsl`
and `blit.wgsl`. There is no depth buffer and no mesh pipeline. Drawing is painter's algorithm,
ordered by `layer` and by Y.

**Where animation lives.** `dark_sprite` is a **simulation** crate — it is in the list in
`tools/check-sim-deps.sh`. `AnimationPlayer` (`play`, `tick`, `step`, `frame`, `finished`) is
deterministic networked state that runs in `FixedUpdate`.

`dark_spine` is **not** a sim crate, and it already proves the split this change needs: the
**bake** gives the simulation clip names, lengths and looping; the **rig** — the geometry — stays
in presentation. The simulation says *"clip 3, step 12"* and presentation resolves the pose.

**The land is already a heightmap.** `dark_land` generates continuous value noise and then
quantizes it to water plus three steps (`WATER`, `STEPS`, `HIGHEST`). The real height field
exists and is currently discarded.

**The grid is the gameplay.** `dark_physics` is tile-based — `Cell::Wall | Floor | Level(n)`,
colliders with `height`, jump apex, *low (jumped over, stood on)*. The simulation is already
2.5D.

**Tooling.** Blender 4.5 is installed at
`C:/Program Files/Blender Foundation/Blender 4.5/blender.exe`.

**The test model.** `D:/Catwalk Idle To Twist R.fbx`, measured through Blender:

| | |
|---|---|
| Armature | 69 bones, `mixamorig:` prefix — standard Mixamo humanoid |
| Mesh | 1 mesh, 2,654 verts, 5,036 triangles |
| Material | 1 — `Peasant_Girl`, 3 textures |
| Action | 1 — 66 frames at 30 fps (2.2 s), 520 fcurves |
| FBX version | 7.4 binary |

**What was proved.** FBX imports, converts to GLB (4.7 MB), and renders with cel banding
(`Diffuse → Shader to RGB → ColorRamp`, constant interpolation) through a 50° and 68° top-down
camera. The silhouette holds from front, side and back. Renders in
`<scratchpad>/tos3/` and the turnaround sheet beside them.

Two things that did **not** work and are tuning rather than risk:

- The inverted-hull outline did not draw. `use_backface_culling` on the shell material is not
  having the expected effect in EEVEE Next. In the real renderer an outline is a second draw of
  the same mesh with `cull_mode: Front` — explicit rasterizer state, not a material flag.
- The model imports Z-up but its **local** mesh dimensions read Y-up, because the import puts the
  rotation on the object. Measure world-space bounds; do not trust `object.dimensions`.

## Decisions

**Full 3D.** Meshes for characters and terrain. The 2D look comes from shading and texture art,
not from billboards.

**Tree of Savior is not billboard-headed.** It is full 3D with a non-photorealistic shader.
Recorded because the plan was nearly built on the opposite premise. The game that puts 2D sprites
on 3D terrain is Ragnarok Online, and that is a different design.

**The simulation does not change.** `dark_world`, `dark_physics`, `dark_net`, `dark_sim` and the
tile grid stay exactly as they are. Gameplay remains 2.5D on `Cell::Level(n)`. This is rendering
only, and rule 1 keeps it that way.

**Animation keeps its existing split.** The simulation owns clip and step. Presentation owns the
pose. **69 bone matrices never enter the simulation** — per actor, per tick, networked, they would
be absurd. This is what `dark_spine` already does.

**FBX is never loaded at run time.** Rust FBX support is poor and the format is proprietary.
Convert offline to glTF 2.0 through Blender, exactly as `bake-spine` already bakes Spine. The
`gltf` crate reads the result.

**Outlines are inverted hulls**, drawn as a second pass with front-face culling.

**2D portraits survive.** `FaceDef` and the faceset pipeline stay — Tree of Savior uses 2D
portraits in dialogue too.

## Execution phases

Ordered. Each is a separate change with its own gates.

1. **`dark-cli bake-model`** — FBX or glTF in, a baked model asset plus clip metadata out.
   Mirrors `bake-spine`, including the stale-bake test under `DARK_TEST_PROJECT`.
2. **A mesh pipeline in `dark_render`** — depth buffer, skinned-mesh shader, bone palette.
   Depends on 1.
3. **Toon shading and outlines** — banded ramp, inverted hull with `cull_mode: Front`.
   Depends on 2.
4. **A 3D camera in `dark_view`** — replaces `MapView`'s flat layout. Depends on 2.
5. **3D terrain from `dark_land`** — stop discarding the continuous height field. Depends on 4.
6. **The editor viewport** — `Map::preview` must draw what the game draws, or WYSIWYG is lost.
   Depends on 4.
7. **Retire Spine for characters.** Last, and only once 1–6 are proven.

## Verification

- The four gates in `AGENTS.md` at every phase.
- `tools/check-sim-deps.sh` must keep passing — no phase may put a render dependency into a sim
  crate. This is the single most important check in the whole program.
- Phase 1: a baked model round-trips and the stale-bake test fails when the source changes.
- Phase 2: the Peasant_Girl model draws, skinned, animating, at 30 fps of clip time.
- Phase 3: a screenshot comparison against `<scratchpad>/tos3/game.png`.
- Phases 4–6: the existing autopilot screenshots are the regression surface, and they will all
  change. New baselines are part of each phase, not an afterthought.

## Files this entry will touch

| Phase | Files |
|---|---|
| 1 | `apps/dark-cli/src/` (new `bake_model.rs`), `crates/dark_assets/src/lib.rs` |
| 2 | `crates/dark_render/src/` (new `mesh.rs`, `mesh.wgsl`), `lib.rs` |
| 3 | `crates/dark_render/src/mesh.wgsl`, `mesh.rs` |
| 4 | `crates/dark_view/src/` |
| 5 | `crates/dark_land/src/`, `crates/dark_world/src/land.rs` |
| 6 | `apps/dark-editor/src/viewport.rs` |
| all | `docs/PLAN.md` — changed in the same commit as the code it describes |

## Risk & rollback

- **Art replacement is the real cost**, and it is not an engineering number. Every character sheet
  in `../adventurer` becomes unusable for characters. Portraits and UI survive.
- **Spine becomes sunk cost** — the bake pipeline, the staleness test, `goblin.spine.ron`.
- **Sorting** is the known hard seam: painter's algorithm and Y-order must give way to a depth
  buffer, and anything still drawn as a sprite has to be reconciled with it.
- **The editor's WYSIWYG promise** is load-bearing and phase 6 is not optional.
- Rollback is per phase. Phases 1–3 are additive and can sit unused. From phase 4 the 2D path is
  being replaced, and that is the point of no return.

## Non-goals

- 3D physics. The tile grid stays, and gameplay stays 2.5D.
- Any change to the simulation crates.
- A 3D world editor. The editor keeps painting tiles; it draws them in 3D.
- Runtime FBX loading.

## Open questions

- **Does the camera rotate freely, or is the angle fixed?** Full 3D allows either. A fixed angle
  would have allowed pre-rendering to sprites instead, at a fraction of the cost; choosing 3D has
  settled the engineering either way, but the answer decides phase 4's shape.
- **Bone budget.** 69 includes fingers and both eyes. A top-down game wants nearer 25, and the
  bake is where that reduction belongs.
- **Where the tile art goes.** Terrain as textured geometry, or as a heightfield with the existing
  tile textures projected onto it.

## Execution log

### Phase 1 — `dark-cli bake-model`, done 2026-09-26

`crates/dark_assets/src/model.rs` (new): `ModelDef`, `ModelBake`, `BakedModelClip`,
`LoadedModel`, `source_hash`, `Project::load_model_def`, `Project::load_model`.
`tools/bake_model.py` (new), `apps/dark-cli/src/bake_model.rs` (new), wired into
`apps/dark-cli/src/main.rs`. Docs: `docs/PLAN.md` §16.1, `AGENTS.md`.

Proved against the real export. `D:/Catwalk Idle To Twist R.fbx` → 1 clip, 130 ticks, looping,
69 bones, 41 px tall, 4.7 MB GLB. 66 frames at 30 fps is 2.167 s, which is 130 ticks at 60 —
the arithmetic holds.

**Decided during the work**

- **One clip per action, no directions.** Recorded in `docs/PLAN.md` §16.1. Eight times fewer
  clips than a Spine sheet needs.
- **The bake script writes RON, not JSON.** No JSON dependency for a build tool. `dark-cli`
  reads it straight back into `ModelBake`, so a shape mismatch fails at the end of the bake
  rather than silently later, and a test in `dark_assets` pins the script's output verbatim.
- **Blender is found, not configured.** `DARK_BLENDER`, then the newest Windows install, then
  the path. Found 4.5 unaided.
- **`ModelBake::read` lives in `dark_assets`**, so `dark-cli` needs no `ron` dependency and the
  asset crate keeps ownership of its own format.

- **Blender reports seconds; the engine turns them into ticks.** `ModelMeasure` is what the
  script writes, `ModelBake` is what the engine writes, and `ticks_for` is the one rule.
- **The script never writes the bake itself.** It writes a measurement beside it, which is
  deleted once read, so a failed run cannot leave a stale bake looking fresh.

**Corrected during the work**

- **The tick rule was wrong, and it would have desynchronised combat.** The first version had
  Python compute ticks as `round(seconds × 60)` for every clip. §16 says a Spine clip that plays
  once is `floor(duration × 60) + 1` — it shows its last key — and only a looping one rounds. So
  a one-shot model clip would have been a tick shorter than the same animation on a skeleton, and
  an attack would have recovered on a different tick depending only on how the character was
  drawn. Found by reading `dark_spine::bake` against the script rather than by a test. The rule
  now lives once, in `dark_assets::model::ticks_for`, with a test that fails if either moves.
- RON strings from Blender were written unescaped. An animation named with a quote or a backslash
  — and an animation is called whatever an artist typed — would have written a file that does not
  parse. Escaped now, and non-finite numbers fall back to zero rather than emitting `NaN`, which
  RON will not read.

- `source_hash` was written with a length prefix and a comment claiming that without it two
  exports could collide by ending in zeroes. **That was false** — FNV-1a multiplies every byte
  through, so a trailing zero already moves the result. The sabotage check passed, which proved
  the assertions were vacuous. The prefix is gone and the comment says what is true. The test now
  fails when the fold is broken.
- The Blender script measures **world-space** bounds. A Mixamo FBX stands Z-up in the scene while
  its local `object.dimensions` read Y-up, which produced three wrong renders before it was
  caught. Written into the script as a comment so it is not rediscovered.

**Found, deliberately not fixed here** — two shipping paths do not know about models, and both
belong to phase 2 because nothing can reach a model yet (a look's `sheet` cannot name one):

- **`dark-cli package` does not collect them.** It follows `.spine.ron` to its skeleton, atlas,
  pages and bake; `.model.ron` needs the same for its source, mesh and bake, or a packaged build
  ships without the geometry. This is the same shape as the bug where `package` did not follow
  stamped places.
- **`Project::fingerprint` does not cover a model's bake.** It hashes reachable scenes, the land
  shape, `combat.ron` and `life.ron`. A bake carries **ticks per clip**, which drives attack
  recovery, so two machines with different bakes would fight differently and the handshake would
  not notice.

  Worth recording plainly: **Spine bakes have this gap today.** `fingerprint` does not follow a
  sheet to its baked file either, so a stale or hand-edited `*.baked.ron` is already a latent
  desync. It is low-likelihood — baking is deterministic and the smoke test checks the skeleton
  hash — but it is real, and it is not something this change introduced. Fixing it changes the
  fingerprint value, which turns away every client on the old one, so it is a deliberate
  versioned change and not something to slip into a model-baking commit.

**Gates** — `cargo fmt`, `clippy -D warnings`, `cargo test --workspace`, `check-sim-deps.sh`: all
green. `dark_assets` is still a simulation crate with no presentation dependency; Blender is
spawned from `dark-cli`, which is an app.

**Verified by hand** — the staleness test fires on a changed export (*"the model changed since it
was baked"*), a wrong animation name reports what the file actually holds, a corrupt source exits
1 and leaves the previous good bake untouched, and the command works from `cargo run` and from
the built binary with an unrelated working directory.

**From the review**

Four of its findings were already fixed before it reported, and it arrived independently at the
same remedy for the tick rule — move the arithmetic out of Python. What it found that had been
missed:

- **Blender exits 0 on anything but a `SystemExit`.** A locked output file, a corrupt export or a
  changed API would have been read as success. Now run with `--python-exit-code 1`, and verified:
  a corrupt source exits 1 and the previous bake survives.
- **`height` was the largest of the three axes, not the vertical one.** It would have reported a
  T-posed bind by its arm span, a quadruped by its length and a prone body by its reach — and
  nameplates and bubbles sit at this height. Now `extent.z`, which is where Blender's importers
  put up whatever the source convention was. It did not bite the test model, whose Z already led.
- **Event ticks lacked `dark_spine`'s epsilon and its dedup.** `((t × 60) − 1e-3).ceil()` now,
  and two markers of one name on one tick are one event, matching `dark_spine` exactly.
- **`os.makedirs` covered the mesh but not the measurement.**
- **The script path walk had no stop.** It ran to the drive root, so a stray
  `C:/tools/bake_model.py` could have been picked up and run. It now stops at the checkout.
- **`ModelMeasure` accepted unknown fields**, so a field the script renamed would have read back
  as a default and baked a model quietly wrong. It now refuses them.
- **PLAN overstated the contract test.** It said the test pinned the shape "with the script's own
  output"; it is a transcription and cannot see the script drift. The refusal above is what
  catches that, and PLAN now says so.

**Left alone, with reasons** — the `BakedModelClip::events` comment promised a `strike` mapping
that `ModelDef` has no field for, so the comment now says the marker must carry that name itself
rather than inventing a `strike_event`. The `project_smoke` model test scans the top of `models/`
only and is a no-op until a project has models; it is a guard for phase 2, not a claim about
today.

### Phase 2a — loading and posing, done 2026-09-26

Phase 2 splits. This is the half that reads a model and poses it; the mesh pipeline in
`dark_render` is 2b and is not started.

`crates/dark_model/` (new crate, presentation): `Model`, `Part`, `Vertex`, `Bone`, `Skeleton`,
`Animation`, `Track`, `Pose`. `apps/dark-cli/src/preview_model.rs` (new). Workspace gains `gltf`
1.4.1, pinned in the root as rule 5 requires. Docs: `docs/PLAN.md` §16.2.

**Proved by looking.** `preview-model` fills the posed triangles on the CPU with a depth buffer,
from a 50° top-down camera. The Peasant_Girl draws at 5,036 triangles over 69 bones, textured and
correctly self-occluding, and four ticks of her clip give four different images.

**Decided during the work**

- **`dark_model` is a presentation crate**, out of `SIM_CRATES`, exactly as `dark_spine` is. That
  is where `gltf` lives and where bone matrices stay. The check still passes.
- **A CPU preview, not the GPU, proves this phase.** It mirrors `preview-spine`, needs no window,
  and separates "can we read and pose a model" from "can we draw one fast" — which is 2b.
- **The orthographic projection uses the DirectX convention**, depth in 0..1, because that is
  what wgpu wants when this becomes a real pipeline.

**Corrected during the work**

- **The model rendered upside down.** glTF is Y-up by specification; Blender is Z-up and converts
  on export. `Model::height` was reading Z and the preview camera was climbing Z. Both now go
  through `Model::UP`, and PLAN §16.2 says it once so nothing guesses again. This is the third
  time an axis convention has cost time in this line of work.
- **The character came apart into spikes.** glTF names a bone two different ways: a **vertex**
  by its place in the skin's joint list, an **animation** by the glTF node it targets. One map
  was being used for both, so vertices skinned to whichever bone happened to sit at a node's
  number. `Rigging` now keeps both maps, and the parent-order sort moves both with it.
- `glam` 0.33 deprecated `Mat4::look_at_rh` and `Mat4::orthographic_rh`. Moved to
  `glam::camera::rh`, and the render is byte-identical across the change.

**From the review**

It found that three things this phase assumed were wrong **on the very model being used to prove
it**, which the reframing camera in `preview-model` was hiding. Each was confirmed by reading the
GLB before it was fixed:

- **The rule-1 guard never covered this crate.** `dark_model` was not in `FORBIDDEN` in
  `tools/check-sim-deps.sh`, and neither was `gltf`. Absent from `SIM_CRATES` is not the same as
  guarded — the script would have said *passed* while `gltf` sat in the simulation tree. Both
  this entry and PLAN §16.2 claimed a coverage that did not exist. Now listed, and proved:
  adding `dark_model` to `dark_world` makes the check fail.
- **Everything above a joint was being discarded.** The Peasant_Girl's topmost joint hangs under
  an `Armature` node carrying a quarter turn and a scale of 0.01 — the whole Z-up to Y-up
  conversion. A joint whose parent is not a joint now folds that chain into its rest transform.
  The loaded height and the baked height agree to 1.5% (40.65 px against 41.24 px), and
  `preview-model` prints both so a dropped transform shows up instead of being reframed away.
- **The clip was off by a frame and never reached its end.** The GLB's key times run 0.0333 s to
  2.2 s, because Blender writes an action at its own frame numbers, while playback counts ticks
  from zero. Loading now shifts a clip to start at zero, which also makes its length the same
  span `bake-model` measured.
- **STEP tracks were being blended.** This export carries both LINEAR and STEP. A held track now
  keeps its key, and a cubic track is refused rather than read as tangents.
- Attribute accessors shorter than the positions are refused instead of padded; a second skin is
  warned about; and the rasteriser drops non-finite triangles, which would otherwise poison a
  depth pixel permanently.

**And it found the tests could not fail.** The sort fixture was a straight reversal, where the
map and its inverse are the same array, so returning either would pass — on exactly the mapping
the joint-vs-node bug depends on. The child-bone test used pure translations, which commute, so
reversing the composition passed. `palette()` — the crate's actual output — had no test at all,
and every fixture left `inverse_bind` at identity.

All three are now sharpened, plus a new test for STEP, and a script breaks each pinned behaviour
in turn to prove they fail. **The palette test missed its first sabotage** — written with
translations only, it fell into the same commuting trap; it now turns the bone. Four sabotages,
four caught.

**Gates** — all four green. `check-sim-deps.sh` now genuinely covers `dark_model` and `gltf`.

**Still carried** — `dark-cli package` and `Project::fingerprint` do not know about models. Due
in 2b, when a model first becomes reachable. PLAN §16.2 now restates it so §16.1 is not the only
place that remembers.

### Phase 2b — the mesh pass, done 2026-09-26

`crates/dark_render/src/model.rs` and `model.wgsl` (new): `ModelVertex`, `ModelDraw`, `Light`,
`MAX_BONES`, and `Renderer::render_models`. `apps/dark-cli/src/gpu_model.rs` (new) adds
`preview-model … --gpu`. Docs: `docs/PLAN.md` §16.3.

**How it is checked.** The CPU preview and the GPU one draw the same model, clip and tick with
the **same camera** — `preview_model::game_camera` is shared, so a difference between the two
pictures is the drawing and not the framing. They agree to **99.9% of the silhouette**.

**Decided during the work**

- **Models get a pass of their own.** Sprites keep painter's algorithm; meshes get a depth
  buffer. Sorting the two against each other needs the game's camera and is phase 4.
- **The palette carries the placement.** `placement × world × inverse bind`, built on the CPU, so
  the shader multiplies one matrix per vertex rather than three.
- **One uniform buffer for every palette**, at a 256-aligned stride with a dynamic offset per
  draw. 128 bones is 8 KiB against the 64 KiB every backend promises. Short palettes are padded
  with identity rather than with the last draw's bones.
- **Cut out, not blended.** A depth-tested mesh cannot sort its own transparency.
- `dark-cli` gains `wgpu` and `pollster` to open a GPU with no window. It is a developer tool,
  and the preview commands it already carries belong together.

**Corrected during the work**

- **The pipeline ignored `doubleSided` and culled back faces from everything.** This model's
  material is marked double-sided — its hair and skirt are single-layer cards — so the GPU was
  throwing half of them away. The silhouettes agreed to only 78.5%. There are now two pipelines
  and the material picks; agreement went to 99.9%. **Nothing but the CPU-against-GPU comparison
  would have found this**: the GPU picture looked perfectly plausible on its own.
- **The CPU preview ignored alpha**, taking a texel's colour and not its transparency, so a
  cut-out texture drew as a solid card. It did not change this model, whose material is opaque,
  but it was wrong and is fixed to cut out exactly as `model.wgsl` does.

**From the review**

- **The cull-back pipeline had never drawn a triangle.** The only model to hand is marked
  `doubleSided`, so every draw so far took the other pipeline — and a silhouette comparison
  cannot find this, because a closed body looks the same whichever face is culled. There is now
  a GPU test for it, and the winding it pins was **measured rather than reasoned about**: a probe
  drew both windings under both pipelines and the test was written to what came back.
- **Nothing exercised more than one draw, either.** One part means `base_vertex` and the bone
  offset are always zero. Proved by hand first — splitting the model's single part in two gave a
  byte-identical picture — and then pinned by a GPU test that draws two quads whose palettes put
  them in different places.
- **`BONE_STRIDE` is now asserted 256-aligned at compile time.** The open question in this entry
  proposes cutting the bone count to about 25, which would make the stride 1600 — an illegal
  dynamic offset for every draw after the first, and only after the first, so a one-model preview
  would never have shown it.
- **Both palette tests were vacuous**, for the third time in this line of work: they padded with
  the same matrix twice, so a reversal or an off-by-one passed either. `.take(MAX_BONES)` was
  dead code as well, since the zip already stopped at the slots. Bones are told apart now.
- **The two previews did not share their light** — the CPU one lit from below, the GPU from
  above — so only the silhouette was ever comparable. They share `Light::default()` now.
- `render_models` promised more than it can do: a model cannot reach a window at all yet, because
  the sprite pass clears the target and `render` presents inside its own submit. The doc comment
  and §16.3 now say so.

**Corrected in my own method** — a test failed after a sabotage run and I nearly recorded the
wrong cause. The restore in the sabotage script used `shutil.move`, which keeps the backup's
timestamp, so cargo did not rebuild and the next run tested a stale artifact. I had already
"fixed" the test by rewriting it before checking; forcing a rebuild showed the original was fine
all along. The script now touches what it restores, and says why.

**Gates** — all four green. Seven sabotages against the GPU tests, all caught.

**Still carried** — `dark-cli package` and `Project::fingerprint` do not know about models. §16.2
named 2b as the deadline because a model would be reachable by then; it is not, so both move to
phase 4 and §16.3 says so rather than letting PLAN forget. Skinned normals still use the bone
matrix rather than its inverse transpose, which is right while bones only turn and move.

### Phase 4a — models in the frame, done 2026-09-26

Phase 3 (toon shading) is **skipped for now**, at the user's direction. Nothing depends on it:
the entry has 3 and 4 both depending on 2, not on each other.

Phase 4 splits. This is the half that puts a model into the frame the game draws, in one sorted
order with the sprites. The game camera and wiring `dark-player` are 4b.

`crates/dark_render/src/sprite.rs`: `BatchKind`, and `build_batches` interleaves models the way
it already interleaves Spine meshes. `crates/dark_render/src/lib.rs`: `Scene`, `render_scene`,
the depth attachment on the main pass, `draw_mixed` learning the model batch.
`apps/dark-cli/src/gpu_model.rs` uses the new path. Docs: `docs/PLAN.md` §16.4, and §16.3
corrected where it said offscreen-only.

**Decided during the work**

- **One order, not two passes.** A model takes its place among the sprites by layer and feet,
  like everything else. Drawing models in a pass of their own would have put every character in
  front of every prop.
- **The depth buffer lives on the main pass and only models touch it.** Every sprite pipeline
  drawn there declares it as never-write, never-fail, so the flat world is untouched while a
  model's own surfaces still sort. Verified by the autopilot screenshot, which is unchanged.
- **`render_scene` is the one implementation.** `render_with` and `render_models` are thin calls
  on it, so the GPU tests exercise the path the game will use rather than a preview-only one.
- **A `Scene` struct rather than seven arguments**, which also leaves room for 4b's camera.

**Proved**

- A new GPU test puts a model between two sprites: a sprite lower down the screen covers it, the
  same sprite higher up does not.
- The autopilot screenshot of the meadow is unchanged — the depth attachment disturbs nothing.
- `preview-model --gpu` is byte-identical to before the move, now going through the main pass.
- Nine sabotages against the GPU tests, all caught.

**Known and recorded, not fixed** — a model is not part of the silhouette system, so a character
drawn as a model behind a tree is simply hidden rather than showing through; and a model sorted
into the interface pass would be dropped silently. Both are in §16.4.

Phase 4b (the game camera, and `dark-player` submitting a model), and 3 and 5–7, not started.

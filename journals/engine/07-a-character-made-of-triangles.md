# Journal engine/07 — A character made of triangles

Status: Materialized.
Date: 2026-09-30. Depends on: `journals/engine/04` (baking, posing, the mesh pass, the camera),
`docs/PLAN.md` §16.1–§16.5, §16 (Spine, whose path this follows).

## Goal

Draw a character as a 3D model **instead of** their sprite, chosen by the project rather than by
an environment variable. This is the one gap §16.5 names as the reason everything else about
models is dev-only:

> a look cannot name a model, so nothing in a project draws one yet; the stand-in plays one clip
> and does not follow what the character is doing

Asked for directly: replace the player with the model in `../adventurer/models/`.

## Current state (evidence, verified 2026-09-30)

| Piece | State |
|---|---|
| Baking an FBX to glTF + measurements | ✅ `dark-cli bake-model` |
| Loading, skinning, posing | ✅ `dark_model::{Model, Pose}` |
| Drawing, sorted among the sprites, with depth | ✅ §16.3, §16.4 |
| The camera that agrees with the sprite world | ✅ §16.5 `model_camera`, `stand_at` |
| A **look** naming one | ❌ — this entry |
| Following what the character is doing | ❌ — this entry |
| Turning to face | ❌ — `stand_at` has no rotation |
| `package` collecting one | ❌ — due the moment a look can name one (§16.1) |

**The asset.** `models/peasant_girl.model.ron` names **one** animation:

```ron
actions: { "idle": "Armature|mixamo.com|Layer0" }   // 130 ticks, looping
```

So she can be drawn, turned and sorted, and she will play that one clip whatever she is doing,
because that is what is in the file. Walking and attacking need those animations exported and
added to `actions`; nothing in the engine will be in the way when they are.

## Decisions

**A look names a model the way it already names a skeleton: through its sheet.** `combat.ron` and
a scene name a `sheet`, and what that file *is* decides how the character is drawn —
`*.sheet.ron` is sprites, `*.spine.ron` is a Spine skeleton, and now `*.model.ron` is a model. No
new field, no new plumbing through `CharacterSheets`, `LookDef` or the net snapshots: every path
that resolves a look already carries a string, and the extension already means something.

**The bake becomes a sheet the simulation can read**, exactly as `SpineBake::sheet()` does: one
frame per tick, so clip lengths still drive attack recovery and nothing in the simulation learns
that models exist.

**Each action becomes its eight facings.** A model has one clip per action, because the mesh
turns (§16.1). The simulation asks for clips by action *and* facing — `idle_down`,
`attack_up_left` — so the sheet built from a model's bake names all eight for each action, every
one pointing at the same animation. The clips are a fiction the simulation consumes; the turning
is the view's.

**A model missing an action falls back to `idle`.** A Mixamo download is one animation at a time,
so a part-finished model is the normal case, not a broken one. A character who vanishes when she
walks is useless for judging the art; one who slides along in her idle is exactly what is wanted
while the rest is being exported. Said out loud in the log, once per model, so it is not a
mystery.

**Instead of, not beside.** A character whose look is a model draws no sprite. That is the ask,
and it is also the only way to see whether the model actually stands where the character is.
`DARK_MODEL` stays as it is: it is for looking at a model in a world that has none.

**`package` collects models now.** §16.1 recorded this as due "the moment a look can name one",
and this is that moment: a build that left the mesh behind would start and then draw nobody.

**`fingerprint` still does not cover the bake**, and the reason is `journals/engine/06`'s, not
neglect: hashing the *used* bakes needs the reachability walk that lives above `dark_assets`.
A model's clip ticks drive attack recovery, so this matters more now than it did — recorded again
in §16.1 rather than fixed here, because moving that walk is its own change.

## Architecture

```
crates/dark_assets/src/model.rs   ModelBake::sheet(), load_model_sheet()  — a model as a sheet
crates/dark_assets/src/lib.rs     load_sheet dispatches on `.model.ron`; LoadedSheet::model
apps/dark-player/src/demo.rs      ModelView, LookView::model, draw_model()
crates/dark_view/src/lib.rs       stand_at gains the facing it turns to
apps/dark-cli/src/package.rs      a look's model, its mesh and its bake
```

`draw_model` mirrors `draw_skeleton`: pose at the character's animation step, build the palette
with `stand_at`, push `ModelDraw`s. The stand-in already does all of it; this moves it from
"whatever `DARK_MODEL` said" to "what this character's look names".

**Turning.** `stand_at(feet, elevation, scale, facing)` adds a rotation about the up axis. The
eight facings are eight angles, and the mesh is turned to the one the character has. A model
authored facing the camera is at `Facing::Down`.

## Execution phases

One. A look names a model, the character is drawn as one, and a build carries it.

## Verification

- The four gates, and the commit built out of the index in a worktree.
- Unit tests: a model's bake becomes a sheet with eight clips per action, all of the same length,
  and the clip a facing asks for resolves; an action a model does not have falls back to idle; a
  `*.model.ron` named as a sheet loads as one.
- **The player, drawn as the model, in a screenshot** — the thing that was asked for — standing
  where the sprite stood and turning as she walks. `--autopilot` walks her about, so the turn can
  be seen between two frames.
- `package` on the real project, and the model in what it writes.

## Risk & rollback

- The change is opt-in per look: a project that names no model is untouched, and every existing
  test project is such a project.
- The asset has one clip, so the screenshots will show a catwalk idle while she walks. That is the
  file, not the engine, and the fallback is what makes it legible rather than broken.
- `apps/dark-player/src/demo.rs` is being edited by another session. The view side goes in as a
  new method and two small insertions, as `journals/engine/05` phase 2 did.

## Non-goals

- Retargeting, blending between clips, or a state machine over animations. One clip plays at a
  time, as it does for Spine.
- Models for NPCs and enemies in the project's data. The engine will not care, but choosing which
  characters are 3D is a design decision and not this entry's.
- The silhouette system for models (§16.4), which is still sprite-only.
- Shadows. A model gets the same blob shadow a sprite does.

## Execution log

### 2026-09-30

Built as planned. The player of `../adventurer` is the model that was sent, and a packaged build
draws her too.

**What landed.** `ModelBake::sheet()` and `clip_name`, so a model is a sheet the simulation reads;
`load_sheet` dispatching on `.model.ron`; `LoadedSheet::model`. `dark_view::stand_at` takes a
facing and `turn_to` works out the angle. In the player: `ModelView`, `LookView::model`, the
geometry loaded beside the rigs, and a two-pass draw — pose everyone into one palette buffer, then
build the draws from slices of it, because a draw borrows the buffer and nothing may push to it
while one is held. `dark-cli package` collects a look's model, mesh and bake.

**Where it differs from the plan above.** Nowhere in shape. One thing the plan did not foresee:
`StandInPart` and the stand-in's upload are now shared with looks, so the upload became
`upload_model` and the stand-in calls it.

**Verified.**

- **The player drawn as the model**, in daylight and in the rain: standing where the sprite stood,
  sorted among the world, taller than the villager beside her, with her blob shadow under her.
- **Turning**: front-on at one moment of the walk and in profile at another.
- **The packaged build**, run from its own folder, draws her — 91 files including the `.glb` and
  the bake, and not the 20 MB `.fbx`.
- Unit tests: every action offered under eight facings, all of one length; a missing action falls
  back to idle and a model with no idle says so; the feet land where they are put however the
  model is turned; each facing turns the front the right way.
- The four gates, and the commit built out of the index in a worktree.

**The bug the screenshots could not see.** `turn_to` had the two arguments of its `atan2` the
wrong way round, which mirrors the turn: a model walking right faced left. Two screenshots of her
walking looked entirely reasonable, because a figure in profile is a figure in profile. The test
that asks where each facing puts her *front* caught it at once. Worth remembering the next time a
thing looks right on the screen.

**Still owed.** A model takes no tint, so the hit flash and a corpse's fade are lost on one;
models are outside the silhouette system; `fingerprint` still does not cover a bake
(`journals/engine/06` says why); nothing blends between clips.

**For the art.** `models/peasant_girl.model.ron` names one animation, so she plays her catwalk
idle whatever she is doing. To give her more, export the animation from Mixamo onto the same
skeleton, drop the file beside the others, add a line to `actions` — `"walk": "<its name in the
file>"` — and run `dark-cli bake-model`. The engine needs nothing else. The actions it looks for
are `idle`, `walk`, `run` and `attack`.

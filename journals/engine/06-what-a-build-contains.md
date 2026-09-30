# Journal engine/06 — What a build contains, and what two machines must agree on

Status: Materialized.
Date: 2026-09-30. Depends on: `journals/engine/05` (effects and weather), `docs/PLAN.md` §19
(packaging), §5 (the handshake), §16.1 (bakes).

## Goal

M9 is *polish and ship*. Two of its shipping guarantees are not true today, and one of them broke
three days ago in `journals/engine/05` phase 2:

1. **A packaged build has no weather.** `dark-cli package` collects what the scenes, the database
   and the sheets refer to. Nothing follows `world.ron` to the skies it names, so `fx/weather.ron`,
   the `*.fx.ron` files and their sheet are left behind. Demonstrated: packaging Adventurer today
   writes 87 files and **no `fx/` directory at all**. The game logs and carries on, so a player
   would simply never see rain and never be told why.
2. **`Project::fingerprint` does not cover `world.ron`.** §16.7 says two players see the same sky
   "because they computed the same answer from a project the handshake already made them agree
   on". The handshake does no such thing: the fingerprint covers the settings' tile size, the
   reachable scenes, `combat.ron` and `life.ron`, and nothing else. That sentence is false as
   written, and I wrote it.
3. **Nor does it cover the bakes**, whose clip ticks drive attack recovery. Recorded as a known
   gap in §16.1 and deliberately not fixed then ("should not fix in passing"). This is the
   milestone where shipping correctness is the work, so it was to stop being in passing — until
   executing it showed why it cannot be done from `dark_assets` at all. See Decisions.

## Current state (evidence, verified 2026-09-30)

| Piece | Where | What it does today |
|---|---|---|
| `collect` | `apps/dark-cli/src/package.rs` | Walks scenes → sheets → images; reads `world.ron` as a *file* but never its contents |
| `fingerprint` | `crates/dark_assets/src/lib.rs` | tile size, start scene, reachable scenes (+ land shapes), `combat.ron`, `life.ron` |
| `SpineDef.baked` / `ModelDef.baked` | `dark_assets` | A path each, written by `bake-spine` / `bake-model`; hashed by nothing |

Proof of (1):

```
$ dark-cli package ../adventurer <out>
packaged 87 files (30.9 MB)
$ ls <out>/game/fx
No such file or directory
```

## Decisions

**A build follows the world to its weather.** `package` already reads `combat.ron` and `life.ron`
to find icons and structure art; `world.ron` gets the same treatment. Its `weather.chances` name
skies; `fx/weather.ron` says which `*.fx.ron` each sky draws; each effect names a sheet, and the
sheet loop already there pulls in the image. Four links, all of them data the game itself follows.

**A missing effect is an error at packaging time.** Everything else `collect` follows is: "anything
referred to but missing is an error: the package would be broken in a way only playing it would
show." Weather is exactly that kind of breakage — silent at run time — so it must be loud here.

**`world.ron` joins the fingerprint.** It *is* the world: its regions, its factions, its calendar
and now its weather. Two people holding different ones are in different worlds, which is the
whole question the check exists to answer. This also makes §16.7's sentence true rather than
aspirational.

**The bakes cannot join it yet, and this entry says why instead of pretending otherwise.**

*This decision was written the other way round and disproved while executing it.* The plan was:
hash every `sheets/*.spine.ron` and `models/*.model.ron` bake, found by looking, since walking to
the *used* ones would mean parsing `combat.ron` and `dark_assets` cannot depend on `dark_combat`.

What killed it is a sentence in `fingerprint`'s own doc comment: *"a scene nothing can walk into
is left out too, so a packaged build — which carries only what the game can reach — is the same
world as the project it came from."* A build carries no `models/` at all. Hashing every bake would
give a package and its project two different numbers, and a player of the build could not join a
host running from the project. The guarantee that the fingerprint already makes is the one this
would have broken.

So the bakes wait. What they need is for "what the game can reach" to live somewhere both
`package` and the handshake can call — today it lives inside `package`, above `dark_assets`, and
`dark_assets` cannot reach up to it. That is a refactor, not a line, and it is not this entry.

**Models are not due yet.** `package` still collects no `*.model.ron`, and that is right: nothing
in a project can name one (§16.5 — a look cannot name a model), so there is nothing to follow.
`DARK_MODEL` is a developer's env var, not project content. The debt comes due with the look, not
before. Their *bakes* are fingerprinted all the same, because that costs nothing and the day a
look names one the check is already honest.

## Architecture

No new types and no new crates. Two functions grow:

```rust
// apps/dark-cli/src/package.rs
//   after world.ron is added to `needed`:
//   read it, read fx/weather.ron, add each sky's effect file, push its sheet into `sheets`.

// crates/dark_assets/src/lib.rs — Project::fingerprint
//   `world.ron` beside combat.ron and life.ron;
//   then every bake named by sheets/*.spine.ron and models/*.model.ron, sorted.
```

`dark_assets` already parses `SpineDef` and `ModelDef`, so reading a project's defs to find its
bakes needs nothing new.

## Execution phases

One. This is a single change: a build that contains the weather, and a handshake that means what
§16.7 already claims. The bakes were to be the third piece and are not; see Decisions.

## Verification

- The four gates, and the commit built out of the index in a worktree.
- **Package Adventurer and find the weather in it** — the `fx/`, the sheet and its picture — which
  is the check that failed today.
- A packaged build refuses to package when an effect names a sheet that is not there.
- Unit tests: the fingerprint changes when `world.ron` changes and when it goes missing, and
  still does not change for art, words, or a scene nothing can walk into.
- `dark_assets`' project smoke test still passes against the real project.

## Files this entry will touch

`apps/dark-cli/src/package.rs`, `crates/dark_assets/src/lib.rs`, `crates/dark_assets/Cargo.toml`
(if reading `fx/weather.ron` needs `dark_fx` — it must not; see Risk), `docs/PLAN.md` §19 and
§16.7, `journals/engine/05` (its phase 2 gap list).

## Risk & rollback

- **`dark_assets` must not depend on `dark_fx`.** It is a simulation crate and `dark_fx` is
  presentation; `check-sim-deps.sh` would refuse it, rightly. The fingerprint therefore hashes
  `world.ron` as bytes, as it already does for `combat.ron`, and never parses an effect.
- `package` is an app and may depend on `dark_fx` freely.
- A stricter fingerprint refuses joins that used to be allowed. That is the point, but it means a
  project whose bakes are stale on one machine will now say so at the door rather than desyncing
  later — which is the better of the two, and the message already explains itself.

## Non-goals

- Collecting models. See Decisions.
- Sound for the weather. Named in §16.7's gaps and still owed; it is FMOD authoring, not packaging.
- Anything else in M9: Steam, Luau, lighting.

## Execution log

### 2026-09-30

Both true now. A packaged Adventurer rains.

**What landed.** `package::weather` follows `world.ron` → `fx/weather.ron` → each sky's
`*.fx.ron` → its sheet, which the existing sheet loop then pulls the picture from; an effect a
sky names and the project does not have stops the package. `world.ron` joined
`Project::fingerprint` beside `combat.ron` and `life.ron`. `docs/PLAN.md` §5, §19, §16.6 and
§16.7 say so, and §16.1 now records *why* the bakes are still outside it.

**Where it differs from the plan above.** The bakes. The plan had them joining the fingerprint by
enumeration; `fingerprint`'s own doc comment says a package and its project must agree, and a
package carries no `models/`. Written up in Decisions rather than quietly dropped, because the
next person to reach for it deserves the reason.

**Verified.**

- The regression, before and after, on the real project: `package ../adventurer` wrote **87 files
  and no `fx/` directory**; it now writes 92, the five being `fx/weather.ron`, `fx/rain.fx.ron`,
  `fx/snow.fx.ron`, `sheets/fx.sheet.ron` and `placeholder/fx.png`. `fx/smoke.fx.ron` is still
  left behind, rightly: no sky draws it.
- **The packaged build itself**, run from its own folder with `--weather rain`: it rains. That is
  the check that matters, and the one nothing did before.
- Two sabotages, both caught: dropping `world.ron` from the fingerprint's list, and collecting
  the weather without adding it to what is needed.
- The four gates, and the commit built out of the index in a worktree.

**Still owed:** the bakes, waiting on the reachability walk moving somewhere both callers can
reach; models, waiting on a look that can name one; and the weather still has no sound.

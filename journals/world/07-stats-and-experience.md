# Journal world/07 — Stats and experience

Status: Decided.
Date: 2026-09-25. Depends on: `journals/world/01`, `journals/world/06`.

## Goal

Record that the game needs character progression, what the world design already requires of it,
and what is not yet decided. This entry is the **design record**. The engine work gets its own
entry in an engine area when it is time to build.

## Current state (evidence, verified 2026-09-25)

The engine has no character progression. What exists today:

| Piece | Where | What it holds |
|---|---|---|
| Movesets | `combat.ron` | health, poise, each attack of the combo, the dodge |
| Enemies | `combat.ron` | look, attack sheet, moveset, how it thinks |
| Standing and loyalty | `world.ron`, Balance | how people feel about you |
| Party | `dark_world::party` | recruit, follow, dismiss |

Health and poise belong to a **moveset**, not to a character. There is no experience, no level, no
per-character numbers that grow. This is the largest new system the world design has asked for.
Everything else it needs — corpses, parties, factions, standing, NPC schedules, regions, climates
— already exists or is close.

## Decisions

**Characters have stats, and stats grow.**

**Stats gate roles, classes do not.** A place in the hero party is opened by a number, not by what
you chose at creation. The worked example on record: the mage's place wants a minimum **magic
attack of 30**.

**Background is a head start on those numbers, never a lock.** A character created as a mage
already clears the mage threshold and does not have to train into something they were not born
to. A character created as a farmer can still reach it. It is harder. It is never closed.

**Experience is shared inside a party.** Recorded in `journals/world/06`.

**The thresholds make the opening month a race.** Rein recruits on days 7, 15 and 30. A player who
was not born to a role has that long to reach its number — which is what turns the first month
into a sprint and gives the stat system a job beyond growth for its own sake.

## Open questions

Everything below is unsettled and should not be guessed at during implementation.

- **What the stats are.** Magic attack is the only one named so far. Physical attack, defence,
  speed and whatever else are open.
- **How experience is earned.** Fighting, practice, work, study, or several routes.
- **Whether levels exist**, or whether stats rise individually through use.
- **The numeric range.** 30 is a threshold with no scale behind it yet.
- **How background maps to starting numbers.**
- **Whether undead progression is the same system.** Low undead rot and high undead do not
  (`journals/world/02`), so ranking up the undead ladder is a progression of some kind. Whether it
  is this one or a parallel track is open.
- **Gandahl's one-hit test.** Landing a single blow on him is a pure skill gate, not a stat gate.
  Whether any stat helps at all is open, and the answer changes what the test means.

## Non-goals

- Implementation. The build is a separate entry in an engine area, and `docs/PLAN.md` changes with
  that work, not with this entry.
- Equipment, crafting and item stats. Related, but not this entry.

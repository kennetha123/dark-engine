# The Cast

Every person in the world gets written. Shopkeepers included.

Setting truth is `docs/WORLD.md`. Why any of it was decided is `journals/world/`.

## Two tiers

**Principals** get a file each, in `docs/cast/principals/`. The people the story turns on.

**Residents** are grouped by where they live: `docs/cast/<settlement>.md`, one short block per
person. That is how a town is designed and how the game stores it — a scene holds its NPCs.

`docs/cast/principals/INDEX.md` lists everyone known so far and what is still missing.

## Why the fields look like this

The fields mirror what the editor already asks for, so filling in the game is **transcription,
not invention**. A character has two halves in the data:

- **An actor** in `world.ron` — a person of the world, who lives the year and can join you.
- **An NPC** in a scene — a portrait, a sheet, a day, and lines.

Somebody who only stands in one shop needs the second. Somebody who lives a year needs both.

## Principal format

```markdown
# <Name> — <what they are, in one line>

Faction: <human | undead | neutral | elf> · Lives: <node> · Joins you: <yes | no | conditional>
Portrait: <faceset, index> · Sheet: <sheet> · Fights: <moveset, or none>
Actor: <id in world.ron, or "scene only">

## What they want
## What they did
## What they know
## Their day
## How they change
## Lines
## Connections
```

**What they want** — in the present tense. The thing that moves them today.

**What they did** — history. Only what bears on the game.

**What they know** — *the important one.* Rumours they will pass on if asked. This is how the
world tells itself, and it is why a shopkeeper is not just a shop. The player learns that Garskin
does not want the war because somebody in a neutral town says so.

**Their day** — hour, place, and whether they sleep there. Goes straight into the editor's day
panel.

**How they change** — by allegiance, by season, by what has happened in the world. Most people
should read differently to an undead player than to a human one.

**Lines** — the writing, with its string keys. Game content: written to its own voice, never to
the documentation style.

**Connections** — who they know, and what they would do about it.

## Resident format

Same fields, compressed to a block:

```markdown
## <Name> — <one line>

Sheet · Portrait · Day · Faction
**Wants:** …
**Knows:** …
**Lines:** …
```

## Rules

**Everyone knows something.** An NPC with nothing in *What they know* is furniture. Give them a
rumour, even a wrong one — especially a wrong one.

**Most people should react to what the player is.** Undead cannot enter human towns, so a human
shopkeeper never meets one. But the neutral towns meet everybody, and the people there have
opinions.

**Names are game content.** They are written for the ear, not for the documentation style guide.
Watch for two things that have already bitten: names that collide (**Rein** and **Reiner VII** are
half-brothers, which is deliberate — do not add a third), and names that mean something unintended
in English.

# Journal world/06 — The hero party is a clock

Status: Decided.
Date: 2026-09-25. Depends on: `journals/world/01`, `journals/world/04`.

## Goal

Settle the hero party: who they are, when they form, how a player takes one of their places, and
what it costs on either side of that decision.

## Decisions

**The hero party is four people, and so is a co-op party.** They are not the NPCs who win if you
do not. They are **another party in the same world doing the same job** — meetable, beatable to a
destination, recruitable away from each other.

**They form on a fixed schedule, not on the player's progress:**

| Day | Joins Rein |
|---|---|
| 7 | Porrega |
| 15 | Shanya |
| 30 | Rowen |

**Shanya becomes archmage on day 250**, when she discovers her power to protect others. The party
runs on the world simulation's clock. The player can accelerate it, delay it or break it, and the
chronicle reports what happened either way.

**A player can join Rein and take one of those places.** Filling the role stops the recruitment:
be a magician in the party before day 15 and **Shanya never joins**.

**The gate is a stat threshold, not a class** — for example a minimum magic attack of 30 for the
mage's place. A character born to it already clears it. A character not born to it can train up.
See `journals/world/07`.

**So days 1–30 are a training race.** If you were not born a mage and you want that place, you
have fifteen days to reach the number. The first month is a sprint in a game that is otherwise
about living slowly, and it is the only part of the year shaped that way.

**The party caps at four.** A fourth applicant is refused — Rein says it is full.

**Experience is shared.**

**Party members are bound and cannot separate.** A magical bond keeps them in one area. That is
the real price of joining: no settling in a town, no farming a season, no walking to Narimbia
because you felt like it. **Joining the hero party means giving up the game that the rest of this
design is about.** It is enforced mechanically rather than by a rule, and it keeps a party
co-located, which is kinder to the netcode than free roaming.

**Leaving is permanent.** Walk out and Rein will not take you back for the rest of the game.

**A place taken is an arc lost.** Shanya does not die if you take her seat. She stays at the Mage
Tower, still a high-rank mage, never archmage, never needed, never married. Visit her on day 300
and she is fine. Polite. Busy. The player finds out by absence, with no cutscene and no music.
The same holds for Porrega and Rowen.

**If the player takes the relics, the party goes anyway and dies.** Porrega comes and asks for
them back first (`journals/world/04`). Their corpses lie in the Dark Lord's mansion, and every
rule in `journals/world/03` applies to them — including that Ambrose can raise them, and the
player can meet Rein again with his own face somewhere in the thing's memory.

### The cast

Full entries in `docs/cast/`. In brief:

- **Rein** — the protagonist. A gifted swordsman who likes helping people. A son of Reiner VI by a
  concubine, so half-brother to the reigning king, whom he meets later. Takes a liking to Shanya.
- **Shanya** — a high-rank mage. Met Rein when he came to the Mage Tower asking where the sword
  was. Rowdy, tomboyish, very skilled.
- **Porrega** — a veteran paladin. The grandfather of the group: cooks, looks after everyone.
  Taught Rein swordmastery, and is with him from the start.
- **Rowen** — a priest. Likes handsome, heavy-set men. Dislikes Rein, whom she reads as a soft
  boy. Wants Porrega, who still loves his dead wife and will not move. She understands that and
  does not push. Drinks.

Rowen and Porrega is the party's emotional engine and it is already finished. It needs no
resolution, and that is what makes them people instead of roles.

## Open questions

- **Can more than one player join?** Three could fill hammer, book and runes, making the hero
  party Rein plus three humans, with Porrega, Shanya and Rowen all sitting out the story.
- **Co-op splits badly.** Two players join Rein and two do not; the bound pair physically cannot
  reach their friends. Leaving is possible but permanent, so the escape hatch costs the run.
  Unresolved, and a session-level risk.
- **What the ending is if a player holds Shanya's place and the party wins.** The marriage cannot
  happen.
- **Whether there is a later route in.** The whole decision currently lives in days 7–30 of 365,
  and a first-time player will miss it without knowing it existed. The darker option: you can join
  after day 30, but you **replace** someone, which means talking Rein into dropping a friend.

## Non-goals

- An eight-person merged party. Considered and rejected — the cap is four.

# Journal world/00 — The country and its three colours

Status: Decided.
Date: 2026-09-25. Depends on: nothing. First entry in the area.

## Goal

Settle what the world is made of: the places in it, who holds them, and how far apart they are.
Everything else in `journals/world/` sits on top of this.

## Decisions

**A day's walk between cities.** That is the unit. It keeps each journey a single compact
session, and it sets the scale of everything: the year is 365 of those days, and crossing the
country is a real expense of them.

**Territory has three colours, and it moves.**

| Colour | Places |
|---|---|
| **Human** | Ravingard, Sovern, Mariacth, Bosreim, Eidis, Livingard, Pincaoth, Surreanoth, Green Town, Gulard, Mirkish, Folkstagaard, Centreau, Tarniard, Velkstein, School of Mage Tower, the forge, the veterans' village |
| **Neutral** | Birkstein, Erdlis, Narimbia, High Fall, Magnoria |
| **Undead** | Porchin, Rinkart, Castle of Remorse, Castle of Horror, Dark Lord Castle |

**The colours are not fixed.** Undead can take territory. War can be instigated, and the player
can instigate it — see `journals/world/02` for the politics that decide whether it happens.

**Humanity gets breadth; the undead get the power to change the map.** Eighteen places against
five is not a balance error. A human player has somewhere to live; an undead player has a reason
to take somewhere. The two allegiances play different games on the same board.

**Neutral ground is the only place the sides meet.** Undead cannot enter human towns. So Birkstein,
Erdlis, Narimbia, High Fall and Magnoria carry every mixed transaction in the world: trade,
rumour, and a co-op party whose members do not share an allegiance. Birkstein is the closest
neutral node to the human heartland and is therefore the de facto crossroads — fitting, for a city
of poverty and thieves that does not care what anybody is.

**Every neutral zone holds a power that stands outside the war.** Gandahl at High Fall, Maria and
the elves at Erdlis. They judge nobody and belong to nobody. See `docs/cast/`.

**Two places are added to the map drawn so far:**

- **The forge** — a town built around a legendary blacksmith. He is the hinge of all three paths:
  humanity earns the sword from him, the dark path stops it being made, and a neutral player never
  learns what he could have done.
- **The veterans' village** — pensioned-off and maimed soldiers. The world's memory, and the best
  source of stories about places the player has not reached.

**Velkstein and Rinkart face each other across the water.** *Defense of humanity* and *defense of
undead*, named as mirrors, dug in. A front line that never moves is scenery, so this one moves.

**The land between towns is not blank.** The regions carry the survival pressure, and each one has
a job in the economy. Villages and the land beside every town grow the grain and keep the herds;
the food chain is local and does not need a dedicated breadbasket region.

**The military island is a career, not four castles.** Ravingard commands, Sovern trains and
ranks, Mariacth deploys the best, Bosreim projects force by sea. It cannot feed itself and imports
through Bosreim and Eidis, which makes the sea route strategically real and the island permanently
vulnerable.

## Open questions

- **Year one scope.** The full map is 25+ nodes. A first build may want the eastern wedge only —
  Centreau, Mirkish, Velkstein, Rinkart, High Fall, Magnoria — with the west named in conversation
  but not walkable. Not settled.
- **How territory changes hands** in simulation terms. The fact is decided; the mechanism is not.
- **Where the forge and the veterans' village sit** on the map. Both are agreed to exist. Neither
  has a position.
- **Whether Narimbia, Magnoria and High Fall are settlements or regions.** They are currently
  listed as nodes but describe terrain.

## Non-goals

- Biome art, tilesets and climate numbers. The engine reads a region's climate from the database;
  what each region *looks* like is a later entry.
- Roads and rivers as engine features. `docs/PLAN.md` §24.5 records them as a known gap.

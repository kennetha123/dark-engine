# Journals

The decision trail. Each entry is a plan that was written into the record.

## Plan versus entry

A **plan** is the working artifact while a piece of work is being talked through. It lives in
conversation. It is not authoritative.

A **journal entry** is a plan committed under `journals/`. It is authoritative. It is the record.

Writing a plan into `journals/` is called **materializing** it.

## Where things go

| Path | Holds | Committed |
|---|---|---|
| `journals/` | Materialized plans — the decision trail | yes |
| `docs/PLAN.md` | Engine truth: scope, stack, architecture, milestones | yes |
| `docs/WORLD.md` | Setting truth: what the world *is*, as currently settled | yes |
| `docs/cast/` | The people in the world, one file per principal | yes |

`docs/PLAN.md` and `docs/WORLD.md` say what is true **now**. Journals say **why and when it was
decided**. Read the first two to know the game; read journals to know how it got that way.

An entry cites those documents. It does not restate them.

## Layout

One directory per area. One file per entry. Numbered within each directory:

    journals/<area>/<NN>-<slug>.md
    journals/<area>/<line>/<NN>-<slug>.md

Areas exist when their first entry needs one. Do not seed empty areas. Nesting is one level deep
at most. The `NN-` prefix restarts at `00` in every directory.

Cite an entry by path and number, with an optional section: `journals/world/02 §Decisions`. Never
cite a bare `journals/02` — the number means nothing without its directory.

## Status

Each entry opens with a status line. Three states:

- **Decided** — a decision artifact. No execution follows. Most world and story entries are this.
- **Materialized** — on disk, not yet executed. A build task waiting.
- **Superseded by `<area>/<NN>`** — replaced by a later entry. Keep the entry. It is still the
  record of what was once true.

A world entry that later needs engine work does not change status. The build gets its **own
entry** in an engine area, depending on the world entry. Design and construction stay separate
records.

## Workflow

1. **Plan.** Talk the work through. The plan lives in conversation.
2. **Materialize.** Write it to `journals/<area>/<NN>-<slug>.md`. This is its own turn.
3. **Execute.** Carry the entry out against the code, only after being told to. Append the result
   to *Execution log* in the same file.

An entry on disk is **not permission to execute it**. Do not begin execution in the session that
wrote the entry unless told to. Default to stopping after materializing.

Trivial changes (typo, comment, import fix, formatting) skip all of this. Say what you are doing
and do it.

## Entry format

    # Journal <area>/<NN> — <one-line description>

    Status: <Decided | Materialized | Superseded by <area>/<NN>>.
    Date: <YYYY-MM-DD>. Depends on: <prior entries or docs, if any>.

    ## Goal
    ## Current state (evidence, verified <date>)
    ## Decisions
    ## Open questions
    ## Execution phases
    ## Verification
    ## Files this entry will touch
    ## Non-goals
    ## Execution log

Drop any section with nothing in it. A **Decided** entry usually carries Goal, Decisions, Open
questions and Non-goals, and nothing else.

Notes:

- **Current state** — what is true today. Date the evidence.
- **Decisions** — flat statements. One decision per bullet. Say what was chosen, not what was
  considered.
- **Open questions** — what is deliberately unsettled. Better here than quietly assumed.
- **Execution log** — appended to the same file when the work is done. Never a separate file.

Name real files, paths and contracts in every section.

## Style

Active voice. Short sentences. One name for one thing. Write the entry so a person who was not in
the conversation can act on it.

This applies to entries and to root-level documents. It never applies to **game content** —
names, dialogue, item text and flavour are written to their own voice, and nothing here constrains
them.

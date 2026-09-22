//! Events as English lines, for `dark-cli simulate` and the host's log.

use crate::social::Parting;
use crate::world::{ActorId, Happening, Outcome, RegionId, World, WorldEvent};

pub(crate) fn describe(
    world: &World,
    goal: ActorId,
    event: &WorldEvent,
    text: &dyn Fn(&str) -> String,
) -> String {
    let actor = |id: ActorId| {
        let a = world.get(id);
        if a.player.is_some() {
            "a player".to_owned()
        } else {
            text(&a.name)
        }
    };
    let region = |id: RegionId| text(&world.region_at(id).name);
    let many = |ids: &[ActorId]| {
        ids.iter()
            .map(|&id| actor(id))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let fallen = |deaths: &[ActorId]| {
        if deaths.is_empty() {
            String::new()
        } else {
            format!("; {} fell", many(deaths))
        }
    };
    let when = format!("day {:>3} {:02}:00", event.hour / 24 + 1, event.hour % 24);
    let what = match &event.what {
        Happening::Plan { intent } => format!("the hero party decides: {intent}"),
        Happening::Departed { from, to, hours } => format!(
            "the hero party sets out from {} for {} ({hours} h)",
            region(*from),
            region(*to)
        ),
        Happening::Arrived { region: r } => format!("the hero party reaches {}", region(*r)),
        Happening::Encounter {
            region: r,
            enemy,
            won,
            deaths,
        } => format!(
            "monsters ({enemy}) attack on the way to {}: {}{}",
            region(*r),
            if *won {
                "driven off"
            } else {
                "the party flees"
            },
            fallen(deaths)
        ),
        Happening::Battle { boss, won, deaths } => format!(
            "the hero party fights {}: {}{}",
            actor(*boss),
            if *won { "victory" } else { "defeat" },
            fallen(deaths)
        ),
        Happening::Trained { strength } => format!("the hero party trains (strength {strength})"),
        Happening::Rested { region: r } => format!("the hero party rests at {}", region(*r)),
        Happening::Joined { actor: a } => format!("{} joins the hero party", actor(*a)),
        Happening::Invited { actor: a, role } => {
            format!("{} could join the hero party as its {role}", actor(*a))
        }
        Happening::JoinedParty { actor: a, leader } => {
            format!("{} follows {}", actor(*a), actor(*leader))
        }
        Happening::LeftParty {
            actor: a,
            leader,
            how,
        } => format!(
            "{} {} {}",
            actor(*a),
            match how {
                Parting::Left => "leaves",
                Parting::Dismissed => "is sent away by",
                Parting::Deserted => "deserts",
                Parting::Betrayed => "turns away from",
                Parting::Disbanded => "parts ways with",
            },
            actor(*leader)
        ),
        Happening::Betrayed { actor: a, leader } => {
            format!("{} betrays {}", actor(*a), actor(*leader))
        }
        Happening::Defected { actor: a, faction } => format!(
            "{} goes over to {}",
            actor(*a),
            text(&world.factions[usize::from(faction.0)].name)
        ),
        Happening::Died { actor: a, killer } => match killer {
            Some(k) => format!("{} is killed by {}", actor(*a), actor(*k)),
            None => format!("{} dies", actor(*a)),
        },
        Happening::TitlePassed { title, to } => format!(
            "{} is now {}",
            actor(*to),
            text(&world.titles[usize::from(title.0)].name)
        ),
        Happening::TitleVacant { title } => format!(
            "nobody holds the title {} now",
            text(&world.titles[usize::from(title.0)].name)
        ),
        Happening::PartyFell => "the hero party is no more".to_owned(),
        Happening::YearEnded { outcome } => match outcome {
            Outcome::GoalDefeated { day } => {
                format!("the year ends; {} fell on day {}", actor(goal), day + 1)
            }
            Outcome::GoalSurvived => format!("the year ends with {} alive", actor(goal)),
        },
    };
    format!("{when}  {what}")
}

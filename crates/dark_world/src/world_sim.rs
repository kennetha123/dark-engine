//! The world simulation on the host (docs/PLAN.md §12), and the night passing when everyone
//! online sleeps (§4.1).
//!
//! [`dark_sim::WorldSim`] steps in in-game hours; each tick this catches it up with the clock,
//! after telling it where players are (their map's region) and so which regions the full
//! simulation is running. What it reports goes to the log, and the whole world ([`WorldSave`])
//! is saved at each new day when a save file is set, and by [`save_now`] when the host quits.

use std::path::{Path, PathBuf};

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, SystemSet};
use dark_assets::{Localization, Project};
use dark_core::{App, FixedUpdate, Plugin};
use dark_net::SessionState;
use dark_sim::{RegionId, WorldDef, WorldEvent, WorldSim, YEAR_HOURS};
use dark_time::{ClockEvent, Presence, everyone_asleep};

use crate::characters::{Asleep, CharacterState, PlayerAvatar};
use crate::life::{NightPass, NightPassed, minute_of};
use crate::save::{self, WorldSave};
use crate::{MapId, Maps, NetHost, WorldClock};

/// After physics (players have moved) and before snapshots (they show the new time).
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct WorldStep;

/// The world simulation and what the host needs to run it.
#[derive(Resource)]
pub struct WorldState {
    pub sim: WorldSim,
    /// Region of each map, by `MapId`.
    regions: Vec<Option<RegionId>>,
    /// Where to save at each new day.
    save: Option<PathBuf>,
    saved_day: u32,
    /// For the log: names from the project's first language.
    names: Localization,
    /// What happened this tick.
    recent: Vec<WorldEvent>,
    /// Who hosted the save this world was carried on from (kept for a host with no player).
    pub(crate) saved_host: Option<dark_net::PlayerId>,
}

impl WorldState {
    pub fn region_of(&self, map: MapId) -> Option<RegionId> {
        self.regions.get(usize::from(map.0)).copied().flatten()
    }

    /// What the world simulation reported this tick.
    pub fn recent(&self) -> &[WorldEvent] {
        &self.recent
    }
}

/// The world, from the project's `world.ron`, or carried on from `save` if that file exists.
/// `None` for a project without a world.
pub fn load_world(
    project: &Project,
    save: Option<&Path>,
    seed: u64,
) -> Result<Option<WorldSave>, String> {
    if let Some(save) = save
        && save.exists()
    {
        let text = std::fs::read_to_string(save).map_err(|e| format!("{}: {e}", save.display()))?;
        let world = WorldSave::parse(&text).map_err(|e| format!("{}: {e}", save.display()))?;
        tracing::info!(
            "world carried on from {} (day {}, {} characters)",
            save.display(),
            world.sim.day() + 1,
            world.characters.len()
        );
        return Ok(Some(world));
    }
    let path = project.path("world.ron");
    if !path.exists() {
        return Ok(None);
    }
    let def = WorldDef::load(&path).map_err(|e| e.to_string())?;
    WorldSim::new(&def, seed)
        .map(|sim| Some(WorldSave::new(sim)))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Runs the world simulation alongside the maps, and puts a saved world's characters,
/// structures and people back. Needs [`crate::MapsPlugin`], [`crate::CharactersPlugin`] and (for
/// a save that has them) [`crate::TalkPlugin`] and [`crate::LifePlugin`] first.
pub struct WorldSimPlugin {
    pub world: WorldSave,
    /// Autosave file, written at each new day.
    pub save: Option<PathBuf>,
    pub names: Localization,
}

impl Plugin for WorldSimPlugin {
    fn build(self, app: &mut App) {
        self.world.restore(&mut app.world);
        let WorldSave { sim, host, .. } = self.world;
        let maps = app.world.resource::<Maps>();
        let regions = maps
            .maps
            .iter()
            .map(|map| {
                let region = map.def.region.as_deref()?;
                let id = sim.world().region(region);
                if id.is_none() {
                    tracing::warn!("{}: region {region} is not in the world", map.name);
                }
                id
            })
            .collect();
        // A world carried on from a save brings its time with it.
        let mut clock = app.world.resource_mut::<WorldClock>();
        if sim.hour() > clock.0.day() * 24 + clock.0.hour() as u32 {
            clock.0.set_time(sim.day(), sim.hour() % 24);
        }
        let saved_day = sim.day();
        app.insert_resource(WorldState {
            sim,
            regions,
            save: self.save,
            saved_day,
            names: self.names,
            recent: Vec::new(),
            saved_host: host,
        })
        .add_systems(
            FixedUpdate,
            // NightPass is inside WorldStep.
            (
                sleep_consensus.in_set(NightPass),
                advance_world.in_set(WorldStep),
                autosave.in_set(WorldStep),
            )
                .chain(),
        );
    }
}

/// When every player online is asleep (offline characters count as asleep, players in the
/// reconnect grace window as awake; out cold counts as asleep), the night passes: morning comes
/// and everyone wakes.
fn sleep_consensus(
    host: Res<NetHost>,
    mut clock: ResMut<WorldClock>,
    night: Option<ResMut<NightPassed>>,
    mut players: Query<(&PlayerAvatar, &mut CharacterState, Has<Asleep>)>,
) {
    let presence = |avatar: &PlayerAvatar, state: &CharacterState, offline: bool| match host
        .0
        .sessions()
        .state(avatar.0)
    {
        Some(SessionState::InGame { .. }) if !offline => {
            if state.sleeping || state.impaired.out {
                Presence::Asleep
            } else {
                Presence::Awake
            }
        }
        Some(SessionState::Grace { .. }) => Presence::InGrace,
        _ => Presence::Offline,
    };
    if !everyone_asleep(players.iter().map(|(a, s, off)| presence(a, s, off))) {
        return;
    }
    let before = clock.0.day();
    if let Some(mut night) = night {
        night.0 = Some(minute_of(&clock.0));
    }
    match clock.0.skip_to_morning() {
        Some(ClockEvent::YearEnded) => tracing::info!("everyone slept; the year is over"),
        _ => tracing::info!(
            "everyone slept; morning of day {} (was day {})",
            clock.0.day() + 1,
            before + 1
        ),
    }
    for (_, mut state, _) in &mut players {
        state.sleeping = false;
    }
}

pub(crate) fn advance_world(
    clock: Res<WorldClock>,
    mut world: ResMut<WorldState>,
    players: Query<(&PlayerAvatar, &MapId, Has<Asleep>)>,
) {
    let target = if clock.0.is_year_over() {
        YEAR_HOURS
    } else {
        clock.0.day() * 24 + clock.0.hour() as u32
    };
    if target > world.sim.hour() {
        // Where players are before the hours run. Those online decide where the full
        // simulation runs; an offline player's character sleeps where it is and runs nothing.
        let (mut detailed, mut online) = (Vec::new(), Vec::new());
        for (avatar, map, offline) in &players {
            let Some(region) = world.region_of(*map) else {
                continue;
            };
            let actor = world.sim.player_actor(avatar.0.0.as_u128(), region);
            world.sim.move_player(actor, region);
            if !offline {
                detailed.push(region);
                online.push(actor);
            }
        }
        world.sim.set_online(&online);
        world.sim.set_detailed(&detailed);
        world.sim.advance_to(target);
    }

    // Everything reported this tick, the hours run and what players did (asked someone to
    // follow, say).
    let world = &mut *world;
    let events = world.sim.take_events();
    let text = |key: &str| world.names.text(key).to_owned();
    for event in &events {
        tracing::info!("world: {}", world.sim.describe(event, &text));
    }
    world.recent = events;
}

/// At each new day, the whole world goes to the save file.
fn autosave(world: &mut World) {
    let due = world
        .get_resource::<WorldState>()
        .is_some_and(|w| w.save.is_some() && w.sim.day() != w.saved_day);
    if due {
        let _ = save_now(world);
    }
}

/// Saves the whole world to the host's save file now (it quits, say). `None` without a save
/// file.
pub fn save_now(world: &mut World) -> Option<Result<PathBuf, String>> {
    let (path, sim) = {
        let mut state = world.get_resource_mut::<WorldState>()?;
        let path = state.save.clone()?;
        state.saved_day = state.sim.day();
        (path, state.sim.clone())
    };
    let written = WorldSave::gather(world, sim)
        .to_ron()
        .map_err(|e| e.to_string())
        .and_then(|text: String| save::write(&path, &text).map_err(|e| e.to_string()));
    match &written {
        Ok(()) => tracing::info!("world saved to {}", path.display()),
        Err(err) => tracing::error!("cannot save the world to {}: {err}", path.display()),
    }
    Some(written.map(|()| path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_world_is_carried_on_and_a_new_one_starts_from_world_ron() {
        let dir = std::env::temp_dir().join(format!("dark_world_save_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("project.ron"),
            r#"(name: "t", tile_size: 16, resolution: (320, 180))"#,
        )
        .unwrap();
        let project = Project::open(&dir).unwrap();
        assert!(
            load_world(&project, None, 1).unwrap().is_none(),
            "no world.ron, no world"
        );

        std::fs::write(dir.join("world.ron"), crate::net_tests::TEST_WORLD).unwrap();
        let save = dir.join("world.save.ron");
        let _ = std::fs::remove_file(&save);
        let mut fresh = load_world(&project, Some(&save), 1).unwrap().unwrap();
        assert_eq!(fresh.sim.day(), 0, "no save yet: a new world");
        fresh.sim.advance_hours(24 * 3);
        save::write(&save, &fresh.to_ron().unwrap()).unwrap();
        let carried = load_world(&project, Some(&save), 99).unwrap().unwrap();
        assert_eq!(carried, fresh, "the save wins over the seed");
        // A save from before M7 (the world simulation on its own) still loads.
        save::write(&save, &fresh.sim.save().unwrap()).unwrap();
        let old = load_world(&project, Some(&save), 99).unwrap().unwrap();
        assert_eq!(old.sim, fresh.sim);

        // The host's clock resumes from the saved world's time.
        let mut app = App::new(dark_core::DEFAULT_TICK_RATE);
        let host = dark_net::Host::new(dark_net::HostConfig::default()).unwrap();
        app.add_plugin(crate::HostPlugin {
            host,
            clock: dark_time::ClockConfig::default(),
        });
        app.insert_resource(Maps {
            maps: Vec::new(),
            params: dark_physics::MoveParams::default(),
        });
        app.add_plugin(WorldSimPlugin {
            world: carried,
            save: None,
            names: Localization::default(),
        });
        let clock = &app.world.resource::<WorldClock>().0;
        assert_eq!((clock.day(), clock.hour() as u32), (3, 6));
    }
}

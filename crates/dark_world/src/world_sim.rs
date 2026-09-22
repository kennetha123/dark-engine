//! The world simulation on the host (docs/PLAN.md §12), and the night passing when everyone
//! online sleeps (§4.1).
//!
//! [`dark_sim::WorldSim`] steps in in-game hours; each tick this catches it up with the clock,
//! after telling it where players are (their map's region) and so which regions the full
//! simulation is running. What it reports goes to the log, and the whole world is saved at
//! each new day when a save file is set.

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

/// The world simulation, from the project's `world.ron`, or carried on from `save` if that file
/// exists. `None` for a project without a world.
pub fn load_world(
    project: &Project,
    save: Option<&Path>,
    seed: u64,
) -> Result<Option<WorldSim>, String> {
    if let Some(save) = save
        && save.exists()
    {
        let text = std::fs::read_to_string(save).map_err(|e| format!("{}: {e}", save.display()))?;
        let sim = WorldSim::load(&text).map_err(|e| format!("{}: {e}", save.display()))?;
        tracing::info!(
            "world carried on from {} (day {})",
            save.display(),
            sim.day() + 1
        );
        return Ok(Some(sim));
    }
    let path = project.path("world.ron");
    if !path.exists() {
        return Ok(None);
    }
    let def = WorldDef::load(&path).map_err(|e| e.to_string())?;
    WorldSim::new(&def, seed)
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Runs the world simulation alongside the maps. Needs [`crate::MapsPlugin`] first.
pub struct WorldSimPlugin {
    pub sim: WorldSim,
    /// Autosave file, written at each new day.
    pub save: Option<PathBuf>,
    pub names: Localization,
}

impl Plugin for WorldSimPlugin {
    fn build(self, app: &mut App) {
        let maps = app.world.resource::<Maps>();
        let regions = maps
            .maps
            .iter()
            .map(|map| {
                let region = map.def.region.as_deref()?;
                let id = self.sim.world().region(region);
                if id.is_none() {
                    tracing::warn!("{}: region {region} is not in the world", map.name);
                }
                id
            })
            .collect();
        // A world carried on from a save brings its time with it.
        let mut clock = app.world.resource_mut::<WorldClock>();
        if self.sim.hour() > clock.0.day() * 24 + clock.0.hour() as u32 {
            clock.0.set_time(self.sim.day(), self.sim.hour() % 24);
        }
        let saved_day = self.sim.day();
        app.insert_resource(WorldState {
            sim: self.sim,
            regions,
            save: self.save,
            saved_day,
            names: self.names,
            recent: Vec::new(),
        })
        .add_systems(
            FixedUpdate,
            // NightPass is inside WorldStep.
            (
                sleep_consensus.in_set(NightPass),
                advance_world.in_set(WorldStep),
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
    if let Some(path) = world.save.clone()
        && world.sim.day() != world.saved_day
    {
        world.saved_day = world.sim.day();
        let written = world
            .sim
            .save()
            .map_err(std::io::Error::other)
            .and_then(|text| write_save(&path, &text));
        match written {
            Ok(()) => tracing::info!("world saved to {}", path.display()),
            Err(err) => tracing::error!("cannot save the world to {}: {err}", path.display()),
        }
    }
}

/// Writes beside the save first, so a crash mid-write never leaves half a world.
fn write_save(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let partial = path.with_extension("ron.partial");
    let mut file = std::fs::File::create(&partial)?;
    file.write_all(text.as_bytes())?;
    // On disk before the rename, or a power cut could leave the new name on empty data.
    file.sync_all()?;
    drop(file);
    std::fs::rename(&partial, path)
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
        assert_eq!(fresh.day(), 0, "no save yet: a new world");
        fresh.advance_hours(24 * 3);
        write_save(&save, &fresh.save().unwrap()).unwrap();
        let carried = load_world(&project, Some(&save), 99).unwrap().unwrap();
        assert_eq!(carried, fresh, "the save wins over the seed");

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
            sim: carried,
            save: None,
            names: Localization::default(),
        });
        let clock = &app.world.resource::<WorldClock>().0;
        assert_eq!((clock.day(), clock.hour() as u32), (3, 6));
    }
}

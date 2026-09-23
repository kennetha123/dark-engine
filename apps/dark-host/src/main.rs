//! Headless host: world simulation and UDP, no window, no GPU, no audio.
//!
//! Usage: `dark-host [--port 7777] [--day-secs 1440] [--project <dir> [--scene <file>]
//!        [--save <file>] [--world-seed <n>]]`
//! `--scene` overrides where the game starts; without it the project's `start_scene` is used, as
//! the players' game does, so both sides load the same maps.
//! With a project it hosts that world (maps, characters, NPCs, enemies, replication) for remote players;
//! without one it runs sessions and the clock only. With a `world.ron` the world simulation runs
//! too; `--save` carries a world on from that file and saves it at each new day.

use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use dark_assets::{Localization, Project};
use dark_combat::CombatDef;
use dark_core::{App, DEFAULT_TICK_RATE};
use dark_net::{Host, HostConfig};
use dark_time::ClockConfig;
use dark_world::{
    CharacterSheets, CharactersPlugin, CombatPlugin, HostPlugin, LifePlugin, Maps, MapsPlugin,
    PartyPlugin, ReplicationPlugin, TalkPlugin, WorldSimPlugin, load_world,
};

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let mut port = 7777u16;
    let mut project = None;
    // Without `--scene` the project says where the game starts, as it does for the players:
    // both sides must load the same scenes, in the same order, for map ids to mean the same.
    let mut scene: Option<String> = None;
    let mut save: Option<std::path::PathBuf> = None;
    let mut world_seed = None;
    let mut clock = ClockConfig {
        tick_rate: DEFAULT_TICK_RATE,
        ..ClockConfig::default()
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next();
        let parsed = match arg.as_str() {
            "--port" => value.and_then(|v| v.parse().ok()).map(|v| port = v),
            "--project" => value.map(|v| project = Some(v)),
            "--scene" => value.map(|v| scene = Some(v)),
            "--save" => value.map(|v| save = Some(v.into())),
            "--world-seed" => value
                .and_then(|v| v.parse().ok())
                .map(|v| world_seed = Some(v)),
            "--day-secs" => value
                .and_then(|v| v.parse().ok())
                .filter(|&v: &u32| v > 0)
                .map(|v| clock.day_length_secs = v),
            _ => None,
        };
        if parsed.is_none() {
            eprintln!(
                "usage: dark-host [--port <port>] [--day-secs <seconds>] [--project <dir> [--scene <file>] [--save <file>] [--world-seed <n>]]"
            );
            return ExitCode::FAILURE;
        }
    }

    let host = match Host::new(HostConfig {
        bind: Some(SocketAddr::from(([0, 0, 0, 0], port))),
    }) {
        Ok(host) => host,
        Err(err) => {
            tracing::error!("cannot start host: {err}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(
        "hosting on {}",
        host.udp_addr().expect("udp host has an address")
    );

    let mut app = App::new(DEFAULT_TICK_RATE);
    app.add_plugin(HostPlugin { host, clock });
    if let Some(dir) = project {
        let world = Project::open(dir).and_then(|p| {
            let scene = scene.clone().unwrap_or_else(|| {
                p.settings
                    .start_scene
                    .clone()
                    .unwrap_or_else(|| dark_assets::DEFAULT_SCENE.to_owned())
            });
            let maps = Maps::load(&p, &scene)?;
            let combat = CombatDef::load_or_default(&p.path("combat.ron")).map_err(|e| {
                dark_assets::AssetError::Invalid {
                    path: p.path("combat.ron"),
                    message: e.to_string(),
                }
            })?;
            let sheets = CharacterSheets::load(&p, &maps, &combat)?;
            let names = Localization::load(&p)?;
            let life = dark_life::LifeDef::load_or_default(&p.path("life.ron")).map_err(|e| {
                dark_assets::AssetError::Invalid {
                    path: p.path("life.ron"),
                    message: e.to_string(),
                }
            })?;
            let story_path = p.path("story.ron");
            let story = dark_story::StoryDef::load_or_default(&story_path).map_err(|e| {
                dark_assets::AssetError::Invalid {
                    path: story_path,
                    message: e.to_string(),
                }
            })?;
            Ok((p, maps, sheets, names, combat, life, story, scene))
        });
        match world {
            Ok((project, maps, sheets, names, combat, life, story, scene)) => {
                tracing::info!("hosting {} maps from {scene}", maps.maps.len());
                app.add_plugin(MapsPlugin(maps))
                    .add_plugin(CharactersPlugin(sheets))
                    .add_plugin(ReplicationPlugin)
                    .add_plugin(TalkPlugin)
                    .add_plugin(CombatPlugin(combat))
                    .add_plugin(LifePlugin(life));
                let seed = world_seed.unwrap_or_else(time_seed);
                match load_world(&project, save.as_deref(), seed) {
                    Ok(Some(sim)) => {
                        tracing::info!("world simulation running (seed {seed})");
                        app.add_plugin(WorldSimPlugin {
                            world: sim,
                            save,
                            names,
                        })
                        .add_plugin(PartyPlugin)
                        .add_plugin(dark_world::StoryPlugin(story));
                    }
                    Ok(None) => tracing::info!("the project has no world.ron; no world simulation"),
                    Err(err) => {
                        tracing::error!("cannot load the world: {err}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            Err(err) => {
                tracing::error!("cannot load the world: {err}");
                return ExitCode::FAILURE;
            }
        }
    }

    // Ctrl+C: save the world and close every connection before going.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopping = stop.clone();
    if let Err(err) = ctrlc::set_handler(move || {
        stopping.store(true, std::sync::atomic::Ordering::SeqCst);
    }) {
        tracing::warn!("cannot catch Ctrl+C ({err}); the world saves only at each new day");
    }
    let mut last = Instant::now();
    while !stop.load(std::sync::atomic::Ordering::SeqCst) {
        let now = Instant::now();
        app.update(now - last);
        last = now;
        std::thread::sleep(Duration::from_millis(1));
    }
    tracing::info!("stopping");
    let _ = dark_world::save_now(&mut app.world);
    app.world.resource_mut::<dark_world::NetHost>().0.shutdown();
    ExitCode::SUCCESS
}

/// A fresh world each session unless `--world-seed` pins one.
fn time_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

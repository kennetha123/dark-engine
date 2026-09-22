//! Game client. Hosts (single-player or co-op) or joins a host.
//!
//! Usage:
//!   dark-player [--project <dir>]                          single-player
//!   dark-player [--project <dir>] --host <port>            co-op host, you play too
//!   dark-player --project <dir> --join <ip:port>           join a host (same project and scene)
//! Options:
//!   --scene <file>          scene inside the project (default scenes/meadow.ron)
//!   --player <uuid>         stable identity (default random)
//!   --day-secs <seconds>    day length
//!   --clients <n>           when hosting: also launch n local clients that join this host
//!   --net-sim <l,j,p>       when joining: simulate l ms latency, j ms jitter, p % loss (each way)
//!   --autopilot             scripted route (jumps, ledges, attack, map change) instead of the keyboard
//!   --autopilot-fight       seek out the nearest enemy and fight it (combos, dodging its wind-ups)
//!   --autopilot-camp        pitch the tent, light a fire, drink four ales and stagger off
//!   --autopilot-party       ask Borin along, then lead him against the meadow's enemies
//!   --overlay               start with the collision overlay (F1) on
//!   --lang <code>           text language, one of the project's (default: its first)
//!   --save <file>           when hosting: carry the world on from this file, saving at each new day
//!                           and on quitting (without `--player`, you play the save's host again)
//!   --world-seed <n>        when hosting: the world simulation's seed (default: a new world)
//!   --screenshot <png>      save the internal image after `--frames` frames (default 120) and exit;
//!                           when hosting, frames then advance a fixed 1/60 s so the result is
//!                           reproducible (a joined client keeps real time, like its host)
//! Controls: WASD / arrows to walk, Shift to run, Space to jump, J to attack (again to combo),
//! K to dodge, E to talk (number keys answer when a conversation offers choices), Z to sleep (when every player online sleeps, the night passes), F1 for
//! the collision overlay, F2 to switch language.
//! `--clients` passes `--project`, `--scene`, `--net-sim`, `--lang` and `--autopilot` on to the clients.

mod demo;
mod fx;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, ExitCode};
use std::sync::Arc;
use std::time::Duration;

use dark_assets::{Localization, Project};
use dark_audio::{Audio, AudioConfig};
use dark_core::{App, DEFAULT_TICK_RATE};
use dark_net::{ClientStatus, Host, HostConfig, NetConditions, PlayerId, RemoteClient};
use dark_platform::winit::window::Window;
use dark_platform::{Flow, Game, KeyCode, WindowConfig};
use dark_render::Renderer;
use dark_time::ClockConfig;
use dark_world::{
    ClientSession, DrawCharacter, HostPlugin, LocalInput, NetHost, TickInput, WorldClock,
    WorldSimPlugin, load_world,
};
use demo::{DemoScene, DemoView, Frame, host_characters, host_life};
use glam::Vec2;

/// Used when no project is given.
const DEFAULT_RESOLUTION: (u32, u32) = (640, 360);
const DEFAULT_SCENE: &str = "scenes/meadow.ron";

enum Mode {
    Host(Box<App>),
    Join(Box<ClientSession>),
}

struct Screenshot {
    path: PathBuf,
    after_frames: u64,
}

struct Player {
    mode: Mode,
    internal_size: (u32, u32),
    renderer: Option<Renderer>,
    /// Loaded before the window exists; turned into `view` once the GPU is up.
    scene: Option<DemoScene>,
    view: Option<DemoView>,
    keys: HashSet<KeyCode>,
    /// Presses since the last frame.
    presses: TickInput,
    autopilot: bool,
    /// Fight whatever enemy is nearest instead of taking the keyboard.
    fight: bool,
    /// Make camp and get drunk instead of taking the keyboard.
    camp: bool,
    /// Recruit Borin and go fighting with him.
    recruit: bool,
    /// The characters drawn last frame, for the fight autopilot.
    seen: Vec<DrawCharacter>,
    audio: Audio,
    overlay: bool,
    /// Language to start in, applied once the view exists.
    lang: Option<String>,
    screenshot: Option<Screenshot>,
    frames: u64,
    /// Clients launched with `--clients`, closed with the host.
    children: Vec<Child>,
    /// Game time driven so far, for the autopilot.
    elapsed: Duration,
}

impl Player {
    fn elapsed_ticks(&self) -> u64 {
        (self.elapsed.as_secs_f64() * f64::from(DEFAULT_TICK_RATE)) as u64
    }

    /// This frame's input: held movement and run, and the presses since the last frame.
    fn input(&mut self, dt: Duration) -> TickInput {
        if self.fight {
            let before = self.elapsed_ticks();
            self.elapsed += dt;
            return fight_pilot(&self.seen, before);
        }
        if self.recruit {
            let before = self.elapsed_ticks();
            self.elapsed += dt;
            return party_pilot(before, self.elapsed_ticks(), &self.seen);
        }
        if self.camp {
            let before = self.elapsed_ticks();
            self.elapsed += dt;
            return camp_pilot(before, self.elapsed_ticks());
        }
        if self.autopilot {
            // Game time, not frames: the route must play the same at any frame rate.
            let before = self.elapsed_ticks();
            self.elapsed += dt;
            return autopilot(before, self.elapsed_ticks());
        }
        let held = |keys: &[KeyCode]| keys.iter().any(|k| self.keys.contains(k));
        let axis = |neg: &[KeyCode], pos: &[KeyCode]| f32::from(held(pos)) - f32::from(held(neg));
        TickInput {
            movement: Vec2::new(
                axis(
                    &[KeyCode::KeyA, KeyCode::ArrowLeft],
                    &[KeyCode::KeyD, KeyCode::ArrowRight],
                ),
                axis(
                    &[KeyCode::KeyW, KeyCode::ArrowUp],
                    &[KeyCode::KeyS, KeyCode::ArrowDown],
                ),
            ),
            run: held(&[KeyCode::ShiftLeft, KeyCode::ShiftRight]),
            ..self.presses.take_presses()
        }
    }
}

/// Fights the nearest living enemy in sight: runs in, combos in reach, and dodges away from an
/// enemy winding up close by. Wanders west (where the meadow's enemies are) until it sees one.
fn fight_pilot(seen: &[DrawCharacter], tick: u64) -> TickInput {
    let Some(me) = seen.iter().find(|c| c.you) else {
        return TickInput::default();
    };
    let foe = seen
        .iter()
        .filter(|c| c.hostile && !c.state.fighter.is_dead())
        .min_by(|a, b| {
            let (da, db) = (a.ground.distance(me.ground), b.ground.distance(me.ground));
            da.total_cmp(&db)
        });
    let Some(foe) = foe else {
        return TickInput {
            movement: Vec2::new(-1.0, 0.2),
            ..TickInput::default()
        };
    };
    let d = foe.ground - me.ground;
    let dir = d.normalize_or_zero();
    let winding_up = matches!(
        foe.state.fighter.action,
        dark_combat::Action::Attack { tick, .. } if tick < 8
    );
    if winding_up && d.length() < 40.0 {
        return TickInput {
            movement: -dir,
            dodge: true,
            ..TickInput::default()
        };
    }
    if d.length() > 22.0 {
        return TickInput {
            movement: dir,
            run: true,
            ..TickInput::default()
        };
    }
    TickInput {
        movement: dir * 0.02,
        attack: tick.is_multiple_of(5),
        ..TickInput::default()
    }
}

/// Asks Borin (just east of the spawn) along, in game ticks like [`autopilot`], then fights the
/// meadow's enemies with him beside her ([`fight_pilot`]).
fn party_pilot(from: u64, to: u64, seen: &[DrawCharacter]) -> TickInput {
    const ASK: u64 = 30;
    match to {
        0..20 => TickInput {
            movement: Vec2::new(1.0, -0.3),
            ..TickInput::default()
        },
        20..60 => TickInput {
            recruit: (from..to).contains(&ASK),
            ..TickInput::default()
        },
        _ => fight_pilot(seen, to),
    }
}

/// Makes camp east of the spawn (away from the meadow's enemies) and gets drunk, in game ticks
/// like [`autopilot`]: sets down the tent (hotbar 5) facing down and a fire (6) facing east,
/// drinks four ales (3), then staggers north.
fn camp_pilot(from: u64, to: u64) -> TickInput {
    const USES: &[(u64, u8)] = &[(45, 5), (55, 6), (70, 3), (80, 3), (90, 3), (100, 3)];
    let item = USES
        .iter()
        .find(|(t, _)| (from..to).contains(t))
        .map_or(0, |(_, slot)| *slot);
    let movement = match to {
        0..40 => Vec2::X,
        40..45 => Vec2::Y * 0.01,
        45..50 => Vec2::ZERO,
        50..53 => Vec2::X * 0.01,
        53..130 => Vec2::ZERO,
        _ => Vec2::NEG_Y,
    };
    TickInput {
        movement,
        item,
        ..TickInput::default()
    }
}

/// Scripted route through the meadow test area, in game ticks (60 per second): talk to the
/// villager by the spawn, jump onto the plateau, up onto its level-2 block, attack, drop back
/// down both ledges, then run through the arch into the next map. `from..to` is the span of
/// ticks this frame covers, so a press is never skipped by a frame that jumps over its tick.
fn autopilot(from: u64, to: u64) -> TickInput {
    // (until tick, direction); jumps and attacks at the listed ticks.
    const ROUTE: &[(u64, Vec2)] = &[
        (100, Vec2::X),
        (115, Vec2::Y),
        (280, Vec2::X),
        (300, Vec2::ZERO),
        (460, Vec2::NEG_X),
        (u64::MAX, Vec2::new(-0.31, -0.95)),
    ];
    const JUMPS: &[u64] = &[52, 230];
    const ATTACKS: &[u64] = &[285];
    const TALKS: &[u64] = &[26, 40];
    const RUN_FROM: u64 = 460;
    let movement = ROUTE
        .iter()
        .find(|(until, _)| to < *until)
        .map_or(Vec2::ZERO, |(_, dir)| *dir);
    let pressed = |ticks: &[u64]| ticks.iter().any(|t| (from..to).contains(t));
    TickInput {
        movement,
        run: to >= RUN_FROM,
        attack: pressed(ATTACKS),
        jump: pressed(JUMPS),
        interact: pressed(TALKS),
        ..TickInput::default()
    }
}

impl Player {
    /// A number key: answers a conversation while choices show, else uses the hotbar.
    fn number(&mut self, n: u8) {
        match self.view.as_ref().and_then(|v| v.choice_for(n)) {
            // A key past the last choice does nothing while choosing.
            Some(choice) => self.presses.choice = choice.unwrap_or(0),
            None => self.presses.item = n,
        }
    }
}

impl Game for Player {
    fn init(&mut self, window: Arc<Window>) {
        let mut renderer = match Renderer::new(window, self.internal_size) {
            Ok(renderer) => renderer,
            Err(err) => {
                tracing::error!("renderer unavailable: {err}");
                return;
            }
        };
        if let Some(scene) = self.scene.take() {
            let maps = match &self.mode {
                Mode::Host(app) => app.world.resource::<dark_world::Maps>(),
                Mode::Join(session) => session.maps(),
            };
            match scene.build_view(&mut renderer, maps) {
                Ok(mut view) => {
                    if self.overlay {
                        view.toggle_debug();
                    }
                    if let Some(lang) = &self.lang
                        && !view.set_language(lang)
                    {
                        tracing::warn!("the project has no language {lang:?}");
                    }
                    self.view = Some(view);
                }
                Err(err) => tracing::error!("cannot upload scene: {err}"),
            }
        }
        self.renderer = Some(renderer);
    }

    fn resized(&mut self, width: u32, height: u32) {
        if let Some(renderer) = &mut self.renderer {
            renderer.resize(width, height);
        }
    }

    fn key(&mut self, key: KeyCode, pressed: bool) {
        if pressed {
            self.keys.insert(key);
            match key {
                KeyCode::Space => self.presses.jump = true,
                KeyCode::KeyJ => self.presses.attack = true,
                KeyCode::KeyK => self.presses.dodge = true,
                KeyCode::KeyE => self.presses.interact = true,
                KeyCode::KeyZ => self.presses.sleep = true,
                KeyCode::KeyR => self.presses.relieve = true,
                KeyCode::KeyQ => self.presses.recruit = true,
                KeyCode::Digit1 => self.number(1),
                KeyCode::Digit2 => self.number(2),
                KeyCode::Digit3 => self.number(3),
                KeyCode::Digit4 => self.number(4),
                KeyCode::Digit5 => self.number(5),
                KeyCode::Digit6 => self.number(6),
                KeyCode::Digit7 => self.number(7),
                KeyCode::Digit8 => self.number(8),
                KeyCode::F2 => {
                    if let Some(view) = &mut self.view {
                        view.cycle_language();
                    }
                }
                KeyCode::F1 => {
                    if let Some(view) = &mut self.view {
                        view.toggle_debug();
                    }
                }
                _ => {}
            }
        } else {
            self.keys.remove(&key);
        }
    }

    fn focus_lost(&mut self) {
        // Releases are not reported while unfocused; drop everything, including latched presses.
        self.keys.clear();
        self.presses = TickInput::default();
    }

    fn frame(&mut self, dt: Duration) -> Flow {
        // Screenshots advance a fixed step per frame so the same frame count gives the same image.
        // Not when joined: the host runs in real time, and a client that runs faster than real
        // time would overflow the host's input queue.
        let dt = if self.screenshot.is_some() && matches!(self.mode, Mode::Host(_)) {
            Duration::from_secs(1) / 60
        } else {
            dt
        };
        let input = self.input(dt);
        self.frames += 1;
        let log_now = self.autopilot && self.frames.is_multiple_of(30);

        // Simulation and networking advance whether or not anything can be drawn: the host owns
        // the world for every connected player.
        let clear = match &mut self.mode {
            Mode::Host(app) => {
                if let Some(mut local) = app.world.get_resource_mut::<LocalInput>() {
                    local.0.movement = input.movement;
                    local.0.run = input.run;
                    local.0.latch(input);
                }
                app.update(dt);
                if log_now && let Some((map, characters)) = host_characters(app) {
                    for c in characters.iter().filter(|c| c.you) {
                        tracing::info!(
                            "frame {}: map {} at ({:.1}, {:.1}) elevation {:.1}",
                            self.frames,
                            map.0,
                            c.body.position.x,
                            c.body.position.y,
                            c.body.elevation
                        );
                    }
                }
                sky_color(app.world.resource::<WorldClock>().0.hour())
            }
            Mode::Join(session) => {
                let before = session.status();
                session.update(dt, input);
                if session.status() != before {
                    tracing::info!("connection: {:?}", session.status());
                }
                if log_now && let Some(me) = session.characters().iter().find(|c| c.you) {
                    tracing::info!(
                        "frame {}: map {} at ({:.1}, {:.1}) elevation {:.1}, last correction {:.2} px, seeing {}",
                        self.frames,
                        session.map().map_or(0, |m| m.0),
                        me.body.position.x,
                        me.body.position.y,
                        me.body.elevation,
                        session.last_correction(),
                        session.characters().len()
                    );
                }
                match session.status() {
                    ClientStatus::InGame => [0.05, 0.12, 0.08],
                    ClientStatus::Connecting | ClientStatus::Leaving => [0.08, 0.08, 0.08],
                    ClientStatus::Rejected(_) | ClientStatus::Disconnected => [0.2, 0.02, 0.02],
                }
            }
        };

        let Some(renderer) = &mut self.renderer else {
            if self.screenshot.is_some() {
                tracing::error!("no renderer; cannot take a screenshot");
                return Flow::Exit;
            }
            return Flow::Continue;
        };
        let secs = dt.as_secs_f32();
        self.seen = match (&mut self.mode, &mut self.view) {
            (Mode::Host(app), Some(view)) => match host_characters(app) {
                Some((map, characters)) => {
                    let (life, structures, party, story) = host_life(app, map);
                    let clock = &app.world.resource::<WorldClock>().0;
                    let frame = Frame {
                        maps: app.world.resource::<dark_world::Maps>(),
                        map,
                        characters: &characters,
                        time: Some((clock.day(), clock.hour() as f32)),
                        life: life.as_ref(),
                        structures: &structures,
                        party: &party,
                        story: &story,
                    };
                    view.draw(renderer, &frame, secs);
                    characters
                }
                None => {
                    renderer.render(Vec2::ZERO, clear, &mut []);
                    Vec::new()
                }
            },
            (Mode::Join(session), Some(view)) => match session.map() {
                Some(map) => {
                    let characters = session.characters();
                    let story = session.story();
                    let frame = Frame {
                        maps: session.maps(),
                        map,
                        characters: &characters,
                        time: session.clock(),
                        life: session.life(),
                        structures: session.structures(),
                        party: session.party(),
                        story: &story,
                    };
                    view.draw(renderer, &frame, secs);
                    characters
                }
                None => {
                    renderer.render(Vec2::ZERO, clear, &mut []);
                    Vec::new()
                }
            },
            _ => {
                renderer.render(Vec2::ZERO, clear, &mut []);
                Vec::new()
            }
        };

        // What happened this frame, heard from where the camera looks.
        if let Some(view) = &self.view {
            for &(event, at) in view.sounds() {
                self.audio.play(event, at);
            }
            self.audio.set_listener(view.camera());
        }
        self.audio.update();

        if let Some(shot) = &self.screenshot
            && self.frames >= shot.after_frames
        {
            let capture = renderer.capture();
            match image::save_buffer(
                &shot.path,
                &capture.rgba,
                capture.width,
                capture.height,
                image::ColorType::Rgba8,
            ) {
                Ok(()) => tracing::info!("saved {}", shot.path.display()),
                Err(err) => tracing::error!("cannot save {}: {err}", shot.path.display()),
            }
            return Flow::Exit;
        }
        Flow::Continue
    }

    fn exiting(&mut self) {
        match &mut self.mode {
            // The host leaving ends the session; tell remote players now instead of letting them time out.
            Mode::Host(app) => {
                // A world with a save file keeps everything played until now.
                let _ = dark_world::save_now(&mut app.world);
                app.world.resource_mut::<NetHost>().0.shutdown();
            }
            Mode::Join(session) => {
                let net = session.net_mut();
                net.quit();
                let dt = Duration::from_millis(16);
                while net.status() != ClientStatus::Disconnected {
                    net.update(dt);
                    net.send_packets();
                    std::thread::sleep(dt);
                }
            }
        }
        for child in &mut self.children {
            let _ = child.kill();
        }
    }
}

/// Night-to-noon sky tint from the in-game hour.
fn sky_color(hour: f64) -> [f64; 3] {
    let daylight = (1.0 - (hour / 24.0 * std::f64::consts::TAU).cos()) / 2.0;
    let night = [0.01, 0.01, 0.04];
    let day = [0.35, 0.55, 0.9];
    std::array::from_fn(|i| night[i] + (day[i] - night[i]) * daylight)
}

enum Launch {
    SinglePlayer,
    Host(u16),
    Join(SocketAddr),
}

struct Args {
    launch: Launch,
    player: PlayerId,
    /// `--player` was given (else a save's host plays on as themselves).
    player_given: bool,
    clock: ClockConfig,
    project: Option<PathBuf>,
    scene: String,
    clients: u32,
    net_sim: Option<(String, NetConditions)>,
    autopilot: bool,
    fight: bool,
    camp: bool,
    recruit: bool,
    overlay: bool,
    lang: Option<String>,
    save: Option<PathBuf>,
    world_seed: Option<u64>,
    screenshot: Option<PathBuf>,
    frames: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        launch: Launch::SinglePlayer,
        player: PlayerId::random(),
        player_given: false,
        clock: ClockConfig {
            tick_rate: DEFAULT_TICK_RATE,
            ..ClockConfig::default()
        },
        project: None,
        scene: DEFAULT_SCENE.into(),
        clients: 0,
        net_sim: None,
        autopilot: false,
        fight: false,
        camp: false,
        recruit: false,
        overlay: false,
        lang: None,
        save: None,
        world_seed: None,
        screenshot: None,
        frames: 120,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--autopilot" => {
                args.autopilot = true;
                continue;
            }
            "--autopilot-fight" => {
                args.fight = true;
                continue;
            }
            "--autopilot-camp" => {
                args.camp = true;
                continue;
            }
            "--autopilot-party" => {
                args.recruit = true;
                continue;
            }
            "--overlay" => {
                args.overlay = true;
                continue;
            }
            _ => {}
        }
        let value = it.next().ok_or_else(|| format!("{arg} needs a value"))?;
        let bad = |what: &str| format!("{arg}: {value:?} is not {what}");
        match arg.as_str() {
            "--host" => args.launch = Launch::Host(value.parse().map_err(|_| bad("a port"))?),
            "--join" => args.launch = Launch::Join(value.parse().map_err(|_| bad("ip:port"))?),
            "--player" => {
                args.player = PlayerId(value.parse().map_err(|_| bad("a uuid"))?);
                args.player_given = true;
            }
            "--day-secs" => {
                args.clock.day_length_secs = value
                    .parse()
                    .ok()
                    .filter(|&v: &u32| v > 0)
                    .ok_or_else(|| bad("a positive number"))?
            }
            "--project" => args.project = Some(value.into()),
            "--scene" => args.scene = value,
            "--lang" => args.lang = Some(value),
            "--save" => args.save = Some(value.into()),
            "--world-seed" => args.world_seed = Some(value.parse().map_err(|_| bad("a number"))?),
            "--clients" => args.clients = value.parse().map_err(|_| bad("a number"))?,
            "--net-sim" => {
                let conditions = value.parse::<NetConditions>()?;
                args.net_sim = Some((value, conditions));
            }
            "--screenshot" => args.screenshot = Some(value.into()),
            "--frames" => {
                args.frames = value
                    .parse()
                    .ok()
                    .filter(|&v: &u64| v > 0)
                    .ok_or_else(|| bad("a positive number"))?
            }
            _ => return Err(format!("unknown option {arg}")),
        }
    }
    if matches!(args.launch, Launch::Join(_)) && args.project.is_none() {
        return Err("--join needs --project (the same project and scene as the host)".into());
    }
    if args.clients > 0 && !matches!(args.launch, Launch::Host(_)) {
        return Err("--clients needs --host".into());
    }
    Ok(args)
}

/// Starts `count` copies of this program that join `port` on this machine.
fn launch_clients(count: u32, port: u16, args: &Args) -> Vec<Child> {
    let Ok(exe) = std::env::current_exe() else {
        tracing::error!("cannot find this program to launch clients");
        return Vec::new();
    };
    (0..count)
        .filter_map(|i| {
            let mut cmd = Command::new(&exe);
            cmd.args([
                "--join",
                &format!("127.0.0.1:{port}"),
                "--scene",
                &args.scene,
            ]);
            if let Some(project) = &args.project {
                cmd.arg("--project").arg(project);
            }
            if let Some((raw, _)) = &args.net_sim {
                cmd.args(["--net-sim", raw]);
            }
            if args.autopilot {
                cmd.arg("--autopilot");
            }
            if let Some(lang) = &args.lang {
                cmd.args(["--lang", lang]);
            }
            match cmd.spawn() {
                Ok(child) => Some(child),
                Err(err) => {
                    tracing::error!("cannot launch client {i}: {err}");
                    None
                }
            }
        })
        .collect()
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("error: {err}");
            eprintln!(
                "usage: dark-player [--project <dir>] [--scene <file>] [--host <port> [--clients <n>] | --join <ip:port>] \
                 [--net-sim <ms,ms,%>] [--player <uuid>] [--day-secs <s>] [--autopilot] [--overlay] \
                 [--screenshot <png> [--frames <n>]]"
            );
            return ExitCode::FAILURE;
        }
    };
    tracing::info!("player id {}", args.player);

    let project = match &args.project {
        Some(dir) => match Project::open(dir.clone()) {
            Ok(project) => Some(project),
            Err(err) => {
                tracing::error!("cannot open project: {err}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };
    // Screenshot runs stay quiet; otherwise FMOD, when the project has it.
    let audio = match (&project, &args.screenshot) {
        (Some(project), None) => {
            audio_config(project).map_or_else(Audio::silent, |c| Audio::open(&c))
        }
        _ => Audio::silent(),
    };
    let internal_size = project
        .as_ref()
        .map_or(DEFAULT_RESOLUTION, |p| p.settings.resolution);
    let mut scene = match &project {
        Some(project) => match DemoScene::load(project, &args.scene) {
            Ok(scene) => Some(scene),
            Err(err) => {
                tracing::error!("cannot load scene: {err}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };

    let mut children = Vec::new();
    let mode = match args.launch {
        Launch::Join(addr) => {
            let Some(scene) = &mut scene else {
                unreachable!("--join requires --project, checked in parse_args");
            };
            match RemoteClient::connect(addr, args.player) {
                Ok(mut net) => {
                    if let Some((raw, conditions)) = &args.net_sim {
                        tracing::info!(
                            "simulating network conditions {raw} (latency ms, jitter ms, loss %)"
                        );
                        net.set_conditions(Some(*conditions));
                    }
                    let sheets = scene.character_sheets();
                    Mode::Join(Box::new(ClientSession::new(net, scene.take_maps(), sheets)))
                }
                Err(err) => {
                    tracing::error!("cannot connect to {addr}: {err}");
                    return ExitCode::FAILURE;
                }
            }
        }
        Launch::SinglePlayer | Launch::Host(_) => {
            let bind = match args.launch {
                Launch::Host(port) => Some(SocketAddr::from(([0, 0, 0, 0], port))),
                _ => None,
            };
            let mut host = match Host::new(HostConfig { bind }) {
                Ok(host) => host,
                Err(err) => {
                    tracing::error!("cannot start host: {err}");
                    return ExitCode::FAILURE;
                }
            };
            if let Some(addr) = host.udp_addr() {
                tracing::info!("hosting co-op on {addr}");
            }
            // The world (and whose save it is) comes first: without `--player`, the one who
            // hosted a save plays their own character again.
            let world = match (&project, &scene) {
                (Some(project), Some(_)) => {
                    // Screenshots and the autopilot need the same world every run.
                    let seed = args.world_seed.unwrap_or_else(|| {
                        if args.screenshot.is_some() {
                            1
                        } else {
                            time_seed()
                        }
                    });
                    match load_world(project, args.save.as_deref(), seed) {
                        Ok(world) => world,
                        Err(err) => {
                            tracing::error!("cannot load the world: {err}");
                            return ExitCode::FAILURE;
                        }
                    }
                }
                _ => None,
            };
            let player = match (&world, args.player_given) {
                (Some(saved), false) => saved.host.unwrap_or(args.player),
                _ => args.player,
            };
            if player != args.player {
                tracing::info!("player id {player} (who hosted this save)");
            }
            host.connect_local(player);
            let mut app = App::new(DEFAULT_TICK_RATE);
            app.add_plugin(HostPlugin {
                host,
                clock: args.clock,
            });
            if let (Some(scene), Some(project)) = (&mut scene, &project) {
                scene.install_host(&mut app);
                if let Some(world) = world {
                    let names = Localization::load(project).unwrap_or_default();
                    app.add_plugin(WorldSimPlugin {
                        world,
                        save: args.save.clone(),
                        names,
                    })
                    .add_plugin(dark_world::PartyPlugin)
                    .add_plugin(dark_world::StoryPlugin(scene.story_def()));
                }
            }
            if let Launch::Host(port) = args.launch {
                children = launch_clients(args.clients, port, &args);
            }
            Mode::Host(Box::new(app))
        }
    };

    let title = project.as_ref().map_or_else(
        || "Dark Engine".to_owned(),
        |p| {
            let role = if matches!(mode, Mode::Join(_)) {
                "client"
            } else {
                "host"
            };
            format!("{} — Dark Engine ({role})", p.settings.name)
        },
    );
    let game = Player {
        mode,
        internal_size,
        renderer: None,
        scene,
        view: None,
        keys: HashSet::new(),
        presses: TickInput::default(),
        lang: args.lang.clone(),
        autopilot: args.autopilot,
        fight: args.fight,
        camp: args.camp,
        recruit: args.recruit,
        seen: Vec::new(),
        audio,
        overlay: args.overlay,
        screenshot: args.screenshot.map(|path| Screenshot {
            path,
            after_frames: args.frames,
        }),
        frames: 0,
        children,
        elapsed: Duration::ZERO,
    };
    let window = WindowConfig {
        title,
        width: internal_size.0 * 2,
        height: internal_size.1 * 2,
    };
    match dark_platform::run(window, game) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("event loop failed: {err}");
            ExitCode::FAILURE
        }
    }
}

/// A fresh world each session unless `--world-seed` pins one.
fn time_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// FMOD for this platform from the project's `audio` settings (`DARK_FMOD_LIB` overrides the
/// library). `None`, with a warning, when the project has no audio or it is misconfigured.
fn audio_config(project: &Project) -> Option<AudioConfig> {
    let def = project.settings.audio.as_ref()?;
    let platform = if cfg!(windows) { "windows" } else { "linux" };
    let library = match std::env::var_os("DARK_FMOD_LIB") {
        Some(path) => PathBuf::from(path),
        None => match def.library.get(platform) {
            Some(path) => project.path(path),
            None => {
                tracing::warn!("no FMOD library for {platform} in project.ron; no sound");
                return None;
            }
        },
    };
    let version = match dark_audio::parse_version(&def.fmod_version) {
        Ok(version) => version,
        Err(err) => {
            tracing::warn!("{err}; no sound");
            return None;
        }
    };
    Some(AudioConfig {
        library,
        banks: def.banks.iter().map(|b| project.path(b)).collect(),
        version,
        pixels_per_metre: project.settings.tile_size as f32,
    })
}

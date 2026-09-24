//! Game client. Hosts (single-player or co-op) or joins a host.
//!
//! Usage:
//!   dark-player [--project <dir>]                          single-player
//!   dark-player [--project <dir>] --host <port>            co-op host, you play too
//!   dark-player --project <dir> --join <ip:port>           join a host (same project and scene)
//! Options:
//!   --scene <file>          scene inside the project (default: the project's `start_scene`,
//!                           else scenes/meadow.ron)
//!   --player <uuid>         stable identity (default random)
//!   --day-secs <seconds>    day length
//!   --clients <n>           when hosting: also launch n local clients that join this host
//!   --net-sim <l,j,p>       when joining: simulate l ms latency, j ms jitter, p % loss (each way)
//!   --autopilot             scripted route (jumps, ledges, attack, map change) instead of the keyboard
//!   --autopilot-fight       seek out the nearest enemy and fight it (combos, dodging its wind-ups)
//!   --autopilot-camp        pitch the tent, light a fire, drink four ales and stagger off
//!   --autopilot-party       ask Borin along, then lead him against the meadow's enemies
//!   --overlay               start with the collision overlay (F1) on
//!   --menu                  start with the Esc menu open
//!   --title                 open the title screen even for a run that would go straight in
//!   --together              open the title screen on the games other people are playing
//!   --lang <code>           text language, one of the project's (default: its first)
//!   --save <file>           when hosting: carry the world on from this file, saving at each new day
//!                           and on quitting (without `--player`, you play the save's host again)
//!   --world-seed <n>        when hosting: the world simulation's seed (default: a new world)
//!   --screenshot <png>      save the internal image after `--frames` frames (default 120) and exit;
//!                           when hosting, frames then advance a fixed 1/60 s so the result is
//!                           reproducible (a joined client keeps real time, like its host)
//! Controls: WASD / arrows to walk, Shift to run, Space to jump, J to attack (again to combo),
//! K to dodge, E to talk (number keys answer when a conversation offers choices), Z to sleep (when every player online sleeps, the night passes), F1 for
//! the collision overlay, F2 to switch language, Esc for the menu (the world waits while nobody
//! else is online). A gamepad works alongside the keyboard; `pad.rs` lists its buttons.
//! `--clients` passes `--project`, `--scene`, `--net-sim`, `--lang` and `--autopilot` on to the clients.

// A built game opens no console window on Windows; a development build keeps one for the log.
// Either way the log still reaches a pipe or a file when the output is redirected.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod demo;
mod fx;
mod pad;
mod saves;
mod settings;
mod title;

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
use demo::{DemoScene, DemoView, Frame, Menu, host_characters, host_life};
use glam::Vec2;

/// Used when no project is given.
const DEFAULT_RESOLUTION: (u32, u32) = (640, 360);

/// How often the network is asked who is playing, while that list is up (docs/PLAN.md §23).
const LOOK_EVERY: Duration = Duration::from_secs(2);

/// How long leaving someone else's game waits for the goodbye to reach them before the title
/// screen comes back. Longer than the client's own leaving timeout, so the host hears a quit
/// rather than reading a crash and holding the slot for a minute.
const LINGER: Duration = Duration::from_millis(1500);

/// Why a game would not start, in as much as the player can be told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trouble {
    /// Another game already has the port; only one can be open on a machine.
    PortTaken,
    /// Anything else: a save that will not read, a scene that will not load.
    Other,
}

enum Mode {
    /// Before a world: the title screen (docs/PLAN.md §22).
    Title(Box<title::Title>),
    Host(Box<App>),
    Join(Box<ClientSession>),
}

/// What it takes to start a world once the title screen has been answered: the same things the
/// command line would have given.
struct WorldStart {
    project: Project,
    scene: String,
    clock: ClockConfig,
    player: PlayerId,
    /// `--player` was given, so a save's own host does not take it over.
    player_given: bool,
    world_seed: Option<u64>,
    /// Worked out the first time it is asked for; see [`WorldStart::world_id`].
    world_id: std::cell::OnceCell<u64>,
}

impl WorldStart {
    /// Which world this player holds: the scenes the game can walk into from where it starts, and
    /// what the simulation is built from, in one number. A host accepts only players who hold the
    /// same one, and the list of games on the network shows which of them that is (§5, §23).
    ///
    /// Worked out once, when the title screen is answered: it reads every one of those files, and
    /// the host, the beacon and the list all ask for it. A scene changed after that — by the
    /// editor, while a game is open — is not noticed until the next time a game is started, which
    /// is the same moment the maps themselves were last read.
    fn world_id(&self) -> u64 {
        *self
            .world_id
            .get_or_init(|| self.project.fingerprint(&self.scene))
    }

    /// Builds the host for a game: `fresh` starts a new world, else `save` is carried on. Either
    /// way the world is saved to `save` from then on. With `open`, other people can join on that
    /// port, and a beacon tells the network the game is there (docs/PLAN.md §23).
    fn start(
        &self,
        save: PathBuf,
        fresh: bool,
        open: Option<u16>,
    ) -> Result<App, (Trouble, String)> {
        tracing::info!("playing {} (a new year: {fresh})", save.display());
        let carry_on = (!fresh).then(|| save.clone());
        let bind = open.map(|port| SocketAddr::from(([0, 0, 0, 0], port)));
        // Why it would not start is worth keeping: the player is told the port is taken only
        // when it is, and not when a save is at fault.
        let mut host = Host::new(HostConfig {
            bind,
            world: self.world_id(),
        })
        .map_err(|err| {
            let taken = matches!(&err, dark_net::NetError::Io(io)
                if io.kind() == std::io::ErrorKind::AddrInUse);
            let why = if taken {
                Trouble::PortTaken
            } else {
                Trouble::Other
            };
            (why, err.to_string())
        })?;
        if let Some(addr) = host.udp_addr() {
            tracing::info!("hosting co-op on {addr}");
        }
        let seed = self.world_seed.unwrap_or_else(time_seed);
        let world = load_world(&self.project, carry_on.as_deref(), seed)
            .map_err(|err| (Trouble::Other, err))?;
        let player = match (&world, self.player_given) {
            (Some(saved), false) => saved.host.unwrap_or(self.player),
            _ => self.player,
        };
        host.connect_local(player);
        let mut app = App::new(DEFAULT_TICK_RATE);
        app.add_plugin(HostPlugin {
            host,
            clock: self.clock,
        });
        let mut scene = DemoScene::load(&self.project, &self.scene)
            .map_err(|err| (Trouble::Other, err.to_string()))?;
        scene.install_host(&mut app);
        if world.is_none() {
            tracing::warn!(
                "{} has no world.ron: this game has no world to save",
                self.project.path("").display()
            );
        }
        if let Some(world) = world {
            let names = Localization::load(&self.project).unwrap_or_default();
            app.add_plugin(WorldSimPlugin {
                world,
                // A game is saved where the title screen said, at each new day and on leaving.
                save: Some(save),
                names,
            })
            .add_plugin(dark_world::PartyPlugin)
            .add_plugin(dark_world::StoryPlugin(scene.story_def()));
        }
        Ok(app)
    }

    /// Joins someone else's game. The maps and the character sheets are this project's own:
    /// both sides play the same project, and the host says what happens in it.
    fn join(&self, host: SocketAddr) -> Result<ClientSession, String> {
        tracing::info!("joining {host}");
        let mut scene = DemoScene::load(&self.project, &self.scene).map_err(|e| e.to_string())?;
        let net =
            RemoteClient::connect(host, self.player, self.world_id()).map_err(|e| e.to_string())?;
        let sheets = scene.character_sheets();
        Ok(ClientSession::new(net, scene.take_maps(), sheets))
    }
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
    /// Presses since the last frame, from the keyboard and the pad alike.
    presses: TickInput,
    pads: pad::Pads,
    /// The window has the keyboard. A gamepad belongs to no window, so it is read only here.
    focused: bool,
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
    /// The Esc menu is open: the character takes no input, and the world waits while nobody
    /// else is online.
    menu: bool,
    /// Leave the game at the next frame: save it and go back to the title screen.
    to_title: bool,
    /// What it takes to start a world from the title screen; none when the command line said
    /// what to play (a joined game, a scripted run).
    start: Option<WorldStart>,
    /// What the title screen was last asked for.
    chosen: title::Chosen,
    /// Telling the network this game is open, and how many are in it (docs/PLAN.md §23).
    beacon: Option<dark_net::Beacon>,
    /// Asking the network who else is playing, while the player is looking.
    search: Option<dark_net::Search>,
    /// When the network was last asked, so it is asked on the clock and not on frames.
    looked: Option<std::time::Instant>,
    /// What the title screen should say once the game being left has been put away.
    title_note: Option<&'static str>,
    /// Saying goodbye to a host: how long since it was said. The frames go on meanwhile, so the
    /// window keeps drawing while the word travels.
    going: Option<Duration>,
    /// The pad's stick was pushed up or down last frame, so a held stick moves the choosing
    /// once, not every frame.
    pad_tilted: bool,
    /// The same, sideways, for nudging a setting.
    pad_pushed: bool,
    /// What this player has set: language, loudness, the whole screen or a window.
    settings: settings::Settings,
    /// The project the settings belong to; none for a run with no project at all.
    project: Option<Project>,
    /// The window, for the fullscreen setting.
    window: Option<Arc<Window>>,
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
    /// Saves the picture a `--screenshot` run asked for, once its frames have passed, and says
    /// the game is done. Whatever is on the screen is what is saved, the title screen included.
    fn shot(&mut self) -> Flow {
        let (Some(shot), Some(renderer)) = (&self.screenshot, &mut self.renderer) else {
            return Flow::Continue;
        };
        if self.frames < shot.after_frames {
            return Flow::Continue;
        }
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
        Flow::Exit
    }

    /// Keeps the settings for next time, and shows them on the title screen as they are now.
    fn settings_changed(&mut self) {
        if let Some(project) = &self.project {
            settings::save(project, &self.settings);
        }
        if let Mode::Title(title) = &mut self.mode {
            title.shows(self.settings.clone());
        }
    }

    /// Puts the window over the whole screen, or back in its frame.
    fn show_as_set(&self) {
        use dark_platform::winit::window::Fullscreen;
        if let Some(window) = &self.window {
            let how = self
                .settings
                .fullscreen
                .then_some(Fullscreen::Borderless(None));
            window.set_fullscreen(how);
        }
    }

    /// Fills the screen, or gives it back. What the window is doing now decides which, so one
    /// put back by anything else (a window manager) does not leave the setting stale.
    fn toggle_fullscreen(&mut self) {
        let filling = self
            .window
            .as_ref()
            .is_some_and(|window| window.fullscreen().is_some());
        self.settings.fullscreen = !filling;
        self.show_as_set();
        self.settings_changed();
    }

    /// What the menu's leaving line reads, for a game the player came to from the title screen,
    /// and whether Q still does anything: their own year is saved on the way out, someone else's
    /// is only left, and a goodbye already said is only waited on — pressing Q again would cut
    /// it short and leave the host reading a crash. A run the command line chose has no title to
    /// go back to.
    fn leaving(&self) -> Option<(&'static str, bool)> {
        match (&self.mode, self.start.is_some()) {
            _ if self.going.is_some() => Some(("ui.going", false)),
            (Mode::Host(_), true) => Some(("ui.to_title", true)),
            (Mode::Join(_), true) => Some(("ui.leave_game", true)),
            _ => None,
        }
    }

    /// The pad on the title screen: the stick or the d-pad moves through the list, A takes what
    /// is picked and B goes back, as the keys do.
    fn title_pad(&mut self) {
        let pad = self.pads.poll(&mut self.presses, self.focused);
        let presses = self.presses.take_presses();
        // The stick moves the choosing once a tilt, not once a tick.
        let tilt = pad.movement.y;
        let tilted = tilt.abs() > 0.5;
        let stick = if tilted && !self.pad_tilted {
            tilt.signum() as i32
        } else {
            0
        };
        self.pad_tilted = tilted;
        // The d-pad's hotbar slots are up (1) and down (3) here — and 5 and 7 with the shoulder
        // held, which a player resting a finger on it should not lose the d-pad for.
        let dpad: i32 = pad
            .numbers
            .iter()
            .map(|slot| match slot {
                1 | 5 => -1,
                3 | 7 => 1,
                _ => 0,
            })
            .sum();
        // Sideways on the stick nudges a setting that has a range.
        let sideways = pad.movement.x;
        if sideways.abs() > 0.5 && !self.pad_pushed {
            self.chosen = match &mut self.mode {
                Mode::Title(title) => title.nudge(sideways.signum() as i8),
                _ => title::Chosen::Waiting,
            };
        }
        self.pad_pushed = sideways.abs() > 0.5;
        let Mode::Title(title) = &mut self.mode else {
            return;
        };
        if stick + dpad != 0 {
            title.move_by(stick + dpad);
        }
        if presses.jump {
            self.chosen = title.choose();
        }
        if presses.dodge || pad.menu {
            title.back();
            self.chosen = title::Chosen::Waiting;
        }
    }

    /// Opens the network beacon for a game other people can join, so it shows in their list.
    fn open_to_others(&mut self, app: &App) {
        let Some(addr) = app.world.resource::<NetHost>().0.udp_addr() else {
            return;
        };
        let start = self.start.as_ref();
        self.beacon = open_beacon(
            addr.port(),
            start.map(|start| &start.project),
            start.map_or(0, WorldStart::world_id),
        );
    }

    /// Puts a line under the title screen's list: what went wrong. The string is named rather
    /// than taken, so it is read in whatever language the screen is showing when it is drawn.
    fn title_says(&mut self, key: &'static str) {
        if let Mode::Title(title) = &mut self.mode {
            title.says(key);
        }
    }

    /// Asks the network who is playing, and takes in whoever has answered since the last frame.
    fn look_around(&mut self, again: bool) {
        if self.search.is_none() {
            match dark_net::Search::new() {
                Ok(search) => self.search = Some(search),
                Err(err) => {
                    tracing::warn!("cannot look for games: {err}");
                    self.title_says("ui.cannot_look");
                    return;
                }
            }
        }
        let Some(search) = &mut self.search else {
            return;
        };
        if again {
            search.ask_around(&[title::TOGETHER_PORT]);
        }
        search.listen();
        let found = search.found();
        if let Mode::Title(title) = &mut self.mode {
            title.sees(found);
        }
    }

    /// The title screen: what the player picked, then the screen drawn again.
    fn title_frame(&mut self) -> Flow {
        self.title_pad();
        match std::mem::replace(&mut self.chosen, title::Chosen::Waiting) {
            title::Chosen::Quit => return Flow::Exit,
            title::Chosen::Language => {
                if let Some(view) = &mut self.view {
                    self.settings.language = Some(view.cycle_language());
                }
                self.settings_changed();
            }
            title::Chosen::Volume(delta) => {
                self.settings.change_volume(delta);
                self.audio.set_volume(self.settings.loudness());
                self.settings_changed();
            }
            title::Chosen::Fullscreen => self.toggle_fullscreen(),
            title::Chosen::Play { save, fresh, open } => {
                match self
                    .start
                    .as_ref()
                    .map(|start| start.start(save, fresh, open))
                {
                    Some(Ok(app)) => {
                        if open.is_some() {
                            self.open_to_others(&app);
                        }
                        self.mode = Mode::Host(Box::new(app));
                        return Flow::Continue;
                    }
                    Some(Err((why, err))) => {
                        tracing::error!("cannot start the game: {err}");
                        self.title_says(match why {
                            Trouble::PortTaken => "ui.cannot_open",
                            Trouble::Other => "ui.cannot_play",
                        });
                    }
                    None => tracing::error!("no project to play"),
                }
            }
            title::Chosen::Join(host) => match self.start.as_ref().map(|start| start.join(host)) {
                Some(Ok(session)) => {
                    // Whoever is looked for is being played with now.
                    self.search = None;
                    self.looked = None;
                    self.mode = Mode::Join(Box::new(session));
                    return Flow::Continue;
                }
                Some(Err(err)) => {
                    tracing::error!("cannot join {host}: {err}");
                    self.title_says("ui.cannot_join");
                }
                None => tracing::error!("no project to play"),
            },
            title::Chosen::Look => self.look_around(true),
            title::Chosen::Waiting => {}
        }
        // While the list of games is up it is asked for again every so often, so a game that
        // opens or fills while the player reads shows as it is. Away from that list nothing is
        // asked and the socket is given back, and coming to it asks again at once.
        let looking = matches!(&self.mode, Mode::Title(title) if title.looking());
        if looking {
            // A new socket asks at once, however lately the last one asked.
            let due = self.search.is_none()
                || self.looked.is_none_or(|last| last.elapsed() >= LOOK_EVERY);
            self.look_around(due);
            if due {
                self.looked = Some(std::time::Instant::now());
            }
        } else if self.search.is_some() {
            self.search = None;
            self.looked = None;
        }
        self.audio.update();
        self.frames += 1;
        if let (Some(renderer), Some(view), Mode::Title(title)) =
            (&mut self.renderer, &mut self.view, &self.mode)
        {
            let (heading, items, picked, note) = {
                let strings = view.strings();
                (
                    title.heading(strings),
                    title.items(strings),
                    title.picked(),
                    title.note().map(|key| strings.text(key).to_owned()),
                )
            };
            view.draw_menu(renderer, &heading, &items, picked, note.as_deref());
        } else if self.screenshot.is_some() {
            // Nothing can be drawn, so nothing can be saved: say so instead of waiting for ever.
            tracing::error!("no screen to take a picture of");
            return Flow::Exit;
        }
        self.shot()
    }

    fn elapsed_ticks(&self) -> u64 {
        (self.elapsed.as_secs_f64() * f64::from(DEFAULT_TICK_RATE)) as u64
    }

    /// This frame's input: held movement and run from the keyboard or the pad (whichever is
    /// asking for something), and the presses since the last frame, which both fill.
    fn input(&mut self, dt: Duration, pad: pad::Held) -> TickInput {
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
        let keys = Vec2::new(
            axis(
                &[KeyCode::KeyA, KeyCode::ArrowLeft],
                &[KeyCode::KeyD, KeyCode::ArrowRight],
            ),
            axis(
                &[KeyCode::KeyW, KeyCode::ArrowUp],
                &[KeyCode::KeyS, KeyCode::ArrowDown],
            ),
        );
        TickInput {
            // The keyboard leads while a key is down; otherwise the stick does.
            movement: if keys == Vec2::ZERO {
                pad.movement
            } else {
                keys
            },
            run: held(&[KeyCode::ShiftLeft, KeyCode::ShiftRight]) || pad.run,
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
    /// Lays the bread down by the fire, so the picture shows a dropped item too.
    const DROPS: &[(u64, u8)] = &[(110, 1)];
    let pressed = |presses: &[(u64, u8)]| {
        presses
            .iter()
            .find(|(t, _)| (from..to).contains(t))
            .map_or(0, |(_, slot)| *slot)
    };
    let item = pressed(USES);
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
        drop: pressed(DROPS),
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
    /// A number key: answers a conversation while choices show, else uses the hotbar. Held with
    /// Ctrl it lays one of that slot down instead (never an answer: a conversation is talking,
    /// not rummaging).
    fn number(&mut self, n: u8) {
        let ctrl =
            self.keys.contains(&KeyCode::ControlLeft) || self.keys.contains(&KeyCode::ControlRight);
        match self.view.as_ref().and_then(|v| v.choice_for(n)) {
            // A key past the last choice does nothing while choosing.
            Some(choice) => self.presses.choice = choice.unwrap_or(0),
            None if ctrl => self.presses.drop = n,
            None => self.presses.item = n,
        }
    }
}

impl Game for Player {
    fn init(&mut self, window: Arc<Window>) {
        self.window = Some(window.clone());
        self.show_as_set();
        let mut renderer = match Renderer::new(window, self.internal_size) {
            Ok(renderer) => renderer,
            Err(err) => {
                tracing::error!("renderer unavailable: {err}");
                return;
            }
        };
        if let Some(scene) = self.scene.take() {
            // The title screen has no world yet: its view is laid out from the scene's own maps,
            // which the host loads again for itself when a game starts.
            let mut scene = scene;
            let alone = matches!(self.mode, Mode::Title(_)).then(|| scene.take_maps());
            let maps = match (&self.mode, &alone) {
                (Mode::Title(_), Some(maps)) => maps,
                (Mode::Host(app), _) => app.world.resource::<dark_world::Maps>(),
                (Mode::Join(session), _) => session.maps(),
                (Mode::Title(_), None) => unreachable!("the title takes the scene's own maps"),
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
        // On the title screen the keys choose from the list instead of playing. They are still
        // remembered, so a key held as the game starts is a key the character feels.
        if pressed {
            self.keys.insert(key);
        } else {
            self.keys.remove(&key);
        }
        if pressed && let Mode::Title(title) = &mut self.mode {
            match key {
                KeyCode::ArrowUp | KeyCode::KeyW => title.move_by(-1),
                KeyCode::ArrowDown | KeyCode::KeyS => title.move_by(1),
                KeyCode::ArrowLeft | KeyCode::KeyA => self.chosen = title.nudge(-1),
                KeyCode::ArrowRight | KeyCode::KeyD => self.chosen = title.nudge(1),
                KeyCode::Enter | KeyCode::Space | KeyCode::KeyE => {
                    self.chosen = title.choose();
                }
                KeyCode::Escape => {
                    // Taking it back: the page behind, and nothing chosen.
                    title.back();
                    self.chosen = title::Chosen::Waiting;
                }
                _ => {}
            }
            return;
        }
        if pressed {
            match key {
                KeyCode::Space => self.presses.jump = true,
                KeyCode::KeyJ => self.presses.attack = true,
                KeyCode::KeyK => self.presses.dodge = true,
                KeyCode::KeyE => self.presses.interact = true,
                KeyCode::KeyZ => self.presses.sleep = true,
                KeyCode::KeyR => self.presses.relieve = true,
                // In the menu, leaving a game come to from the title: a game of one's own is
                // saved on the way, and the title opens again.
                KeyCode::KeyQ if self.menu && matches!(self.leaving(), Some((_, true))) => {
                    self.to_title = true;
                }
                KeyCode::KeyQ if self.menu => {}
                KeyCode::KeyQ => self.presses.recruit = true,
                KeyCode::Escape => self.menu = !self.menu,
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
                        self.settings.language = Some(view.cycle_language());
                    }
                    self.settings_changed();
                }
                KeyCode::F11 => self.toggle_fullscreen(),
                KeyCode::F1 => {
                    if let Some(view) = &mut self.view {
                        view.toggle_debug();
                    }
                }
                _ => {}
            }
        }
    }

    fn focus(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            // Releases are not reported while unfocused; drop everything, latched presses too.
            self.keys.clear();
            self.presses = TickInput::default();
        }
    }

    fn frame(&mut self, dt: Duration) -> Flow {
        // Leaving a game: put the world away and go back to the title, where its save is now
        // one of the games to carry on. Someone else's game is only said goodbye to.
        if std::mem::take(&mut self.to_title)
            && let Some(start) = &self.start
        {
            let ready = match &mut self.mode {
                Mode::Host(app) => {
                    match dark_world::save_now(&mut app.world) {
                        Some(Ok(_)) => {}
                        Some(Err(err)) => tracing::error!("the year was not saved: {err}"),
                        None => tracing::warn!("nothing to save: this game has no world"),
                    }
                    app.world.resource_mut::<NetHost>().0.shutdown();
                    true
                }
                // The goodbye is said once and then carried by the frames that follow, so the
                // window keeps drawing while it reaches the host.
                Mode::Join(session) if self.going.is_none() => {
                    let net = session.net_mut();
                    let gone = matches!(
                        net.status(),
                        ClientStatus::Rejected(_) | ClientStatus::Disconnected
                    );
                    if !gone {
                        net.quit();
                        self.going = Some(Duration::ZERO);
                    }
                    gone
                }
                // Said already, and now heard or waited out.
                Mode::Join(_) => true,
                Mode::Title(_) => false,
            };
            if ready {
                self.mode = Mode::Title(Box::new(title::Title::new(
                    start.project.clone(),
                    self.settings.clone(),
                    start.world_id(),
                )));
                // The game is over, so it is no longer on anyone's list.
                self.beacon = None;
                self.menu = false;
                self.going = None;
                if let Some(key) = self.title_note.take() {
                    self.title_says(key);
                }
            }
        }
        if matches!(self.mode, Mode::Title(_)) {
            return self.title_frame();
        }
        // Screenshots advance a fixed step per frame so the same frame count gives the same image.
        // Not when joined: the host runs in real time, and a client that runs faster than real
        // time would overflow the host's input queue.
        let dt = if self.screenshot.is_some() && matches!(self.mode, Mode::Host(_)) {
            Duration::from_secs(1) / 60
        } else {
            dt
        };
        // The pad fills the same presses the keyboard does; Start opens the menu as Esc does.
        let pad = self.pads.poll(&mut self.presses, self.focused);
        if pad.menu {
            self.menu = !self.menu;
        }
        for &slot in &pad.numbers {
            self.number(slot);
        }
        // Behind the menu the character stands still, presses made there are dropped, and the
        // autopilots wait.
        let input = if self.menu {
            self.presses = TickInput::default();
            TickInput::default()
        } else {
            self.input(dt, pad)
        };
        self.frames += 1;
        let log_now = self.autopilot && self.frames.is_multiple_of(30);

        // A game open to others keeps telling the network it is there, and how full it is.
        if let (Mode::Host(app), Some(beacon)) = (&self.mode, &mut self.beacon) {
            let host = &app.world.resource::<NetHost>().0;
            beacon.playing(host.sessions().online_count());
            beacon.answer();
        }

        // Simulation and networking advance whether or not anything can be drawn: the host owns
        // the world for every connected player.
        let clear = match &mut self.mode {
            // Handled above: the title screen draws itself and returns.
            Mode::Title(_) => return Flow::Continue,
            Mode::Host(app) => {
                if let Some(mut local) = app.world.get_resource_mut::<LocalInput>() {
                    // A press latched before the menu opened is dropped, not kept for after.
                    if self.menu {
                        local.0 = TickInput::default();
                    }
                    local.0.movement = input.movement;
                    local.0.run = input.run;
                    local.0.latch(input);
                }
                app.insert_resource(dark_world::Pause(self.menu));
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
                // Leaving by this player's own word: back to the title once the host has heard
                // it, or once it has been given long enough.
                if let Some(waited) = &mut self.going {
                    *waited += dt;
                    if *waited >= LINGER || session.status() == ClientStatus::Disconnected {
                        self.to_title = true;
                    }
                }
                // Turned away at the door, or the host has gone: back to the title with a word
                // about why, rather than a dark screen nothing comes of. A game the command
                // line chose has no title to go back to, so it stays as it is.
                else if self.start.is_some() {
                    self.title_note = match session.status() {
                        // Which refusal it was, where the player can do something about it: a
                        // stale copy of the project is theirs to fix, and "that game would not
                        // let you in" would have them blaming the host.
                        ClientStatus::Rejected(dark_net::RejectReason::DifferentWorld) => {
                            Some("ui.other_world")
                        }
                        ClientStatus::Rejected(_) => Some("ui.turned_away"),
                        ClientStatus::Disconnected => Some("ui.host_gone"),
                        ClientStatus::Connecting | ClientStatus::InGame => None,
                        // Already on the way out, by this player's own word.
                        ClientStatus::Leaving => None,
                    };
                    if self.title_note.is_some() {
                        self.to_title = true;
                    }
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

        let leaving = self.leaving();
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
                    let (life, structures, drops, party, story) = host_life(app, map);
                    let clock = &app.world.resource::<WorldClock>().0;
                    let frame = Frame {
                        maps: app.world.resource::<dark_world::Maps>(),
                        map,
                        characters: &characters,
                        time: Some((clock.day(), clock.hour() as f32)),
                        life: life.as_ref(),
                        structures: &structures,
                        drops: &drops,
                        party: &party,
                        story: &story,
                        menu: self.menu.then(|| {
                            if dark_world::paused(&app.world) {
                                Menu::Paused
                            } else {
                                Menu::OthersPlaying
                            }
                        }),
                        leaving,
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
                        drops: session.drops(),
                        party: session.party(),
                        story: &story,
                        // The host's world goes on.
                        menu: self.menu.then_some(Menu::OthersPlaying),
                        leaving,
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

        self.shot()
    }

    fn exiting(&mut self) {
        match &mut self.mode {
            // Nothing is running yet, or the world was already put away on the way here.
            Mode::Title(_) => {}
            // The host leaving ends the session; tell remote players now instead of letting them time out.
            Mode::Host(app) => {
                // A world with a save file keeps everything played until now.
                let _ = dark_world::save_now(&mut app.world);
                app.world.resource_mut::<NetHost>().0.shutdown();
            }
            Mode::Join(session) => say_goodbye(session),
        }
        for child in &mut self.children {
            let _ = child.kill();
        }
    }
}

/// On the way out of the program: tells the host this player is going and waits for the
/// connection to close, so it can tell a quit from a crash. Leaving for the title screen says
/// the same thing, but carries it on ordinary frames instead of holding the window still.
fn say_goodbye(session: &mut ClientSession) {
    let net = session.net_mut();
    // Nobody to say it to: turned away at the door, or the host is already gone.
    if matches!(
        net.status(),
        ClientStatus::Rejected(_) | ClientStatus::Disconnected
    ) {
        return;
    }
    net.quit();
    let dt = Duration::from_millis(16);
    while net.status() != ClientStatus::Disconnected {
        net.update(dt);
        net.send_packets();
        std::thread::sleep(dt);
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
    /// `--scene`; without it the project says where the game starts.
    scene: Option<String>,
    clients: u32,
    net_sim: Option<(String, NetConditions)>,
    autopilot: bool,
    fight: bool,
    camp: bool,
    recruit: bool,
    overlay: bool,
    menu: bool,
    /// Show the title screen even for a run that would otherwise go straight in.
    title: bool,
    /// Open it on the games to play together in (docs/PLAN.md §23).
    together: bool,
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
        scene: None,
        clients: 0,
        net_sim: None,
        autopilot: false,
        fight: false,
        camp: false,
        recruit: false,
        overlay: false,
        menu: false,
        title: false,
        together: false,
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
            "--menu" => {
                args.menu = true;
                continue;
            }
            "--title" => {
                args.title = true;
                continue;
            }
            "--together" => {
                args.title = true;
                args.together = true;
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
            "--hour" => {
                // Which hour the first day starts at: for watching a villager's day, or the
                // light at dusk, without waiting for it (docs/PLAN.md §21.1).
                args.clock.start_hour = Some(
                    value
                        .parse()
                        .ok()
                        .filter(|h| *h < 24)
                        .ok_or_else(|| bad("an hour of the day, 0 to 23"))?,
                );
            }
            "--day-secs" => {
                args.clock.day_length_secs = value
                    .parse()
                    .ok()
                    .filter(|&v: &u32| v > 0)
                    .ok_or_else(|| bad("a positive number"))?
            }
            "--project" => args.project = Some(value.into()),
            "--scene" => args.scene = Some(value),
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
    args.project = args.project.or_else(packaged_project);
    if matches!(args.launch, Launch::Join(_)) && args.project.is_none() {
        return Err("--join needs --project (the same project and scene as the host)".into());
    }
    if args.clients > 0 && !matches!(args.launch, Launch::Host(_)) {
        return Err("--clients needs --host".into());
    }
    Ok(args)
}

/// The scene to start in: `--scene`, else the project's `start_scene`, else the default. The
/// project holds the rule, so every app follows the same one — a host and a player who disagreed
/// about where the game starts would hold two worlds and refuse each other.
fn start_scene(project: &Project, args: &Args) -> String {
    project.start_scene(args.scene.as_deref())
}

/// The project a packaged game carries: `game/` beside the exe, or the exe's own folder
/// (`dark-cli package`). None when the game is run from a build, where `--project` says which.
fn packaged_project() -> Option<PathBuf> {
    let beside = std::env::current_exe().ok()?.parent()?.to_path_buf();
    [beside.join("game"), beside]
        .into_iter()
        .find(|dir| dir.join(Project::FILE).exists())
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
            cmd.args(["--join", &format!("127.0.0.1:{port}")]);
            if let Some(scene) = &args.scene {
                cmd.args(["--scene", scene]);
            }
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
                 [--net-sim <ms,ms,%>] [--player <uuid>] [--day-secs <s>] [--autopilot] [--overlay] [--menu] [--title] \
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
    let mut audio = match (&project, &args.screenshot) {
        (Some(project), None) => {
            audio_config(project).map_or_else(Audio::silent, |c| Audio::open(&c))
        }
        _ => Audio::silent(),
    };
    let internal_size = project
        .as_ref()
        .map_or(DEFAULT_RESOLUTION, |p| p.settings.resolution);
    let mut scene = match &project {
        // Where the game starts: what was asked for, else the project's own scene.
        Some(project) => match DemoScene::load(project, &start_scene(project, &args)) {
            Ok(scene) => Some(scene),
            Err(err) => {
                tracing::error!("cannot load scene: {err}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };

    // A game started by hand — a scripted run, a screenshot, co-op, a save named on the command
    // line — goes straight in. Otherwise the player is asked what to play (docs/PLAN.md §22).
    // Driven by flags rather than played: a picture or a route, nobody at the keys.
    let by_hand =
        args.screenshot.is_some() || args.autopilot || args.fight || args.camp || args.recruit;
    let scripted = !args.title && (by_hand || args.save.is_some());
    // What this player set last time they played: their language, loudness and window. A
    // scripted run ignores them, so a picture is the same whoever took it and whatever they
    // last set.
    let settings = match (&project, by_hand) {
        (Some(project), false) => settings::load(project),
        _ => settings::Settings::default(),
    };
    audio.set_volume(settings.loudness());
    // The language they read, unless this run asks for another.
    let lang = args.lang.clone().or_else(|| settings.language.clone());
    if args.title && args.save.is_some() {
        tracing::warn!("--title opens the title screen, so --save is not used");
    }
    let start = match (&project, &args.launch, scripted) {
        (Some(project), Launch::SinglePlayer, false) => Some(WorldStart {
            project: project.clone(),
            scene: start_scene(project, &args),
            clock: args.clock,
            player: args.player,
            player_given: args.player_given,
            world_seed: args.world_seed,
            world_id: std::cell::OnceCell::new(),
        }),
        _ => None,
    };

    let mut children = Vec::new();
    // A game hosted from the command line says so on the network too, as one opened from the
    // title screen does.
    let mut beacon = None;
    let mode = match (&start, &args.launch) {
        (Some(start), _) => {
            let mut title =
                title::Title::new(start.project.clone(), settings.clone(), start.world_id());
            if args.together {
                title.look_together();
            }
            Mode::Title(Box::new(title))
        }
        (None, launch) => match launch {
            Launch::Join(addr) => {
                let Some(scene) = &mut scene else {
                    unreachable!("--join requires --project, checked in parse_args");
                };
                match RemoteClient::connect(*addr, args.player, world_id(&project, &args)) {
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
                let mut host = match Host::new(HostConfig {
                    bind,
                    world: world_id(&project, &args),
                }) {
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
                    beacon = open_beacon(port, project.as_ref(), world_id(&project, &args));
                    children = launch_clients(args.clients, port, &args);
                }
                Mode::Host(Box::new(app))
            }
        },
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
        pads: pad::Pads::new(),
        focused: true,
        lang,
        autopilot: args.autopilot,
        fight: args.fight,
        camp: args.camp,
        recruit: args.recruit,
        seen: Vec::new(),
        audio,
        overlay: args.overlay,
        menu: args.menu,
        to_title: false,
        start,
        chosen: title::Chosen::Waiting,
        beacon,
        search: None,
        looked: None,
        title_note: None,
        going: None,
        pad_tilted: false,
        pad_pushed: false,
        settings,
        // A scripted run keeps its settings to itself: it was given them, not asked.
        project: project.filter(|_| !by_hand),
        window: None,
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

/// Says on the network that a game is open on this port, under the project's name, so it shows
/// in other players' lists (docs/PLAN.md §23). A game whose beacon cannot open is still played;
/// it only has to be joined by address.
fn open_beacon(port: u16, project: Option<&Project>, world: u64) -> Option<dark_net::Beacon> {
    let name = project.map_or_else(
        || "Dark Engine".to_owned(),
        |project| project.settings.name.clone(),
    );
    match dark_net::Beacon::new(port, name, world) {
        Ok(beacon) => Some(beacon),
        Err(err) => {
            tracing::warn!("this game will not show on the network: {err}");
            None
        }
    }
}

/// Which world a command line holds, for the paths that have a project rather than a
/// [`WorldStart`]. Without a project there is no world to compare, and `0` says so.
///
/// The scene is resolved the same way the game resolves it, so a host that let the project say
/// where to start and a player who named that same scene are in one world, not two.
fn world_id(project: &Option<Project>, args: &Args) -> u64 {
    project.as_ref().map_or(0, |project| {
        project.fingerprint(&start_scene(project, args))
    })
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

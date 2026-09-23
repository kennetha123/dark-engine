//! End-to-end replication over real UDP on localhost: a host app and clients in one process.

use std::time::Duration;

use dark_assets::Project;
use dark_core::{App, DEFAULT_TICK_RATE};
use dark_net::{ClientStatus, Host, HostConfig, NetConditions, PlayerId, RemoteClient};
use dark_time::ClockConfig;
use glam::Vec2;

use bevy_ecs::world::Mut;
use dark_life::{Need, Shelter, Status, Structure};

use crate::characters::tests::sheets;
use crate::{
    BodyState, CharacterSheets, CharactersPlugin, ClientSession, HostPlugin, MapId, Maps,
    MapsPlugin, NetId, PlayerAvatar, ReplicationPlugin, TalkPlugin, TickInput,
};

const DT: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// Two maps: `a` (spawn at 100,100; exit on its east edge) and `b`.
/// A capital (map `a`) and the lord's keep five hours away.
pub(crate) const TEST_WORLD: &str = r#"(
    regions: [
        (id: "capital", name: "Capital", kind: City, danger: 0, inn: true),
        (id: "keep", name: "Keep", kind: Fortress, danger: 1),
    ],
    roads: [(between: ("capital", "keep"), hours: 5)],
    factions: [(id: "kingdom", name: "Kingdom"), (id: "demons", name: "Demons", hostile: true)],
    titles: [(id: "hero", name: "Hero"), (id: "lord", name: "Lord")],
    actors: [
        (id: "hero", name: "Hero", role: "warrior", faction: "kingdom", power: 40, home: "capital", titles: ["hero"]),
        (id: "squire", name: "Squire", role: "squire", faction: "kingdom", power: 10, home: "capital"),
        (id: "hermit", name: "Hermit", role: "hermit", faction: "kingdom", power: 10, home: "capital", trust: 900),
        (id: "lord", name: "Lord", role: "lord", faction: "demons", power: 500, home: "keep", titles: ["lord"], boss: true),
    ],
    hero_party: (leader_title: "hero", roles: ["warrior"], members: ["hero"], goal: "lord"),
)"#;

fn project() -> Project {
    project_with("")
}

/// The test project, with `enemies` (a scene field, or nothing) in map `a`.
fn project_with(enemies: &str) -> Project {
    project_with_npc(enemies, "")
}

/// The test project, with `enemies` and one more NPC (both scene text, or nothing) in map `a`.
fn project_with_npc(enemies: &str, npc: &str) -> Project {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("dark_world_net_{}_{n}", std::process::id()));
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    std::fs::write(
        dir.join("project.ron"),
        r#"(name: "t", tile_size: 16, resolution: (320, 180))"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("scenes/a.ron"),
        r#"(size: (480, 320), ground: (sheet: "g", frame: 0), region: "capital",
            player: (sheet: "w", attack: "a", moveset: "hero", spawn: (100, 100)),
            npcs: [
                (sheet: "n", position: (100, 180), lines: ["hello", (reply: "thanks"), "bye"],
                    actor: "squire", moveset: "hero"),
                (sheet: "n", position: (300, 250), lines: ["hm"], actor: "hermit"),
                NPC
            ],
            exits: [(area: (460, 0, 20, 320), to: "scenes/b.ron", spawn: (40, 40))],
            inns: [(area: (200, 200, 60, 40), bed: (230, 220))],
            ENEMIES)"#
            .replace("ENEMIES", enemies)
            .replace("NPC", npc),
    )
    .unwrap();
    std::fs::write(
        dir.join("scenes/b.ron"),
        r#"(size: (320, 320), ground: (sheet: "g", frame: 0))"#,
    )
    .unwrap();
    std::fs::write(dir.join("world.ron"), TEST_WORLD).unwrap();
    Project::open(dir).unwrap()
}

/// Ale (slot 1), a tent (2) and firewood (3).
/// The hermit (at 300,250) has a storylet: a gift, a wedding, or a pact with the demons.
const TEST_STORY: &str = r#"(
    storylets: [(id: "hermit", with: "hermit", start: "hello", nodes: {
        "hello": (line: "hm.hello", choices: [
            (says: "hm.gift", then: [Give(item: "ale", count: 2)], next: "thanks"),
            (says: "hm.wed", when: [Unmarried], then: [Marry, FadeToBlack]),
            (says: "hm.dark", then: [Defect("demons")]),
        ]),
        "thanks": (line: "hm.thanks"),
    })],
)"#;

const TEST_LIFE: &str = r#"(
    items: {
        "ale": (name: "item.ale", use: Consume((thirst: -100, bladder: 150, alcohol: 1000))),
        "tent": (name: "item.tent", use: Place(Tent)),
        "wood": (name: "item.wood", use: Place(Campfire(minutes: 90))),
    },
    start: [("ale", 5), ("tent", 1), ("wood", 1)],
)"#;

/// The first NPC map `a` lists (ids go to NPCs as maps load, before anyone joins): the squire,
/// who talks, and follows if asked.
const SQUIRE: NetId = NetId(1);

fn maps(project: &Project) -> Maps {
    Maps::load(project, "scenes/a.ron").unwrap()
}

/// The test looks for every sheet the maps name: `a` is an attack sheet, anything else a base.
/// The test moveset, as `hero`.
pub(crate) fn combat() -> dark_combat::CombatDef {
    // A grunt: 30 health, one slow 10-damage swing.
    let mut grunt = crate::characters::tests::moveset();
    grunt.health = 30;
    grunt.dodge = None;
    grunt.combo[0].damage = 10;
    grunt.combo[0].startup = 12;
    grunt.combo[0].recovery = 24;
    dark_combat::CombatDef {
        movesets: [
            ("hero".to_owned(), crate::characters::tests::moveset()),
            ("grunt".to_owned(), grunt),
        ]
        .into(),
        enemies: [(
            "grunt".to_owned(),
            dark_combat::EnemyDef {
                name: "Grunt".into(),
                sheet: "w".into(),
                attack: Some("a".into()),
                moveset: "grunt".into(),
                ai: dark_combat::AiDef {
                    sight: 200.0,
                    leash: 300.0,
                    attack_range: 18.0,
                    speed: 0.5,
                    back_off: 30,
                    respawn: 600,
                },
            },
        )]
        .into(),
    }
}

/// One grunt a little east of the player's spawn, facing it.
const ARENA: &str = r#"enemies: [(kind: "grunt", position: (140, 100), facing: Left)],"#;

fn looks(project: &Project) -> CharacterSheets {
    let test = sheets();
    CharacterSheets::build(&maps(project), &combat(), project, |path| {
        Ok(if path == "a" {
            test.looks[0].attack.clone()
        } else {
            test.looks[0].base.clone()
        })
    })
    .unwrap()
}

struct Net {
    host: App,
    clients: Vec<ClientSession>,
    addr: std::net::SocketAddr,
    project: Project,
}

impl Net {
    fn new(players: usize, conditions: Option<NetConditions>) -> Self {
        Self::with_project(players, conditions, project())
    }

    /// Like [`Net::new`], with a grunt to fight.
    fn arena(players: usize) -> Self {
        Self::with_project(players, None, project_with(ARENA))
    }

    fn with_project(players: usize, conditions: Option<NetConditions>, project: Project) -> Self {
        let ids = (0..players).map(|_| PlayerId::random()).collect();
        Self::with_world(ids, conditions, project, None)
    }

    /// A host on `project`'s world, or on `saved` when given, and a client for each of `players`.
    fn with_world(
        players: Vec<PlayerId>,
        conditions: Option<NetConditions>,
        project: Project,
        saved: Option<crate::WorldSave>,
    ) -> Self {
        let host = Host::new(HostConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        })
        .unwrap();
        let addr = host.udp_addr().unwrap();
        let mut app = App::new(DEFAULT_TICK_RATE);
        app.add_plugin(HostPlugin {
            host,
            clock: ClockConfig::default(),
        })
        .add_plugin(MapsPlugin(maps(&project)))
        .add_plugin(CharactersPlugin(looks(&project)))
        .add_plugin(ReplicationPlugin)
        .add_plugin(TalkPlugin)
        .add_plugin(crate::CombatPlugin(combat()))
        .add_plugin(crate::LifePlugin(
            dark_life::LifeDef::parse(TEST_LIFE).unwrap(),
        ));
        let world = saved.unwrap_or_else(|| {
            crate::load_world(&project, None, 1)
                .unwrap()
                .expect("the test project has a world")
        });
        app.add_plugin(crate::WorldSimPlugin {
            world,
            save: None,
            names: dark_assets::Localization::default(),
        })
        .add_plugin(crate::PartyPlugin)
        .add_plugin(crate::StoryPlugin(
            dark_story::StoryDef::parse(TEST_STORY).unwrap(),
        ));
        let clients = players
            .into_iter()
            .map(|player| {
                let mut net = RemoteClient::connect(addr, player).unwrap();
                net.set_conditions(conditions);
                ClientSession::new(net, maps(&project), looks(&project))
            })
            .collect();
        Self {
            host: app,
            clients,
            addr,
            project,
        }
    }

    /// Runs `frames` frames with each client holding its movement.
    fn run(&mut self, frames: usize, movement: &[Vec2]) {
        for _ in 0..frames {
            self.frame(movement, false, false);
        }
    }

    /// One frame; the presses go to every client.
    fn frame(&mut self, movement: &[Vec2], jump: bool, attack: bool) {
        for (client, m) in self.clients.iter_mut().zip(movement) {
            client.update(
                DT,
                TickInput {
                    movement: *m,
                    jump,
                    attack,
                    ..TickInput::default()
                },
            );
        }
        self.host.update(DT);
        std::thread::sleep(Duration::from_micros(300));
    }

    /// Client 0 presses `presses` once, then everyone stands a few frames.
    fn press(&mut self, presses: TickInput) {
        self.clients[0].update(DT, presses);
        self.host.update(DT);
        self.run(10, &vec![Vec2::ZERO; self.clients.len()]);
    }

    /// The host's life of character `id`, to read or change.
    fn host_life(&mut self, id: NetId) -> Mut<'_, crate::Life> {
        let mut q = self.host.world.query::<(&NetId, &mut crate::Life)>();
        q.iter_mut(&mut self.host.world)
            .find(|(n, _)| **n == id)
            .map(|(_, l)| l)
            .expect("players have a body")
    }

    /// Replaces client `i` with a fresh connection for `player`.
    fn reconnect(&mut self, i: usize, player: PlayerId) {
        let net = RemoteClient::connect(self.addr, player).unwrap();
        self.clients[i] = ClientSession::new(net, maps(&self.project), looks(&self.project));
    }

    fn connect(&mut self) {
        let still = vec![Vec2::ZERO; self.clients.len()];
        for _ in 0..600 {
            self.run(1, &still);
            if self.clients.iter().all(|c| c.map().is_some()) {
                return;
            }
        }
        panic!("clients never got their character");
    }

    /// The host's authoritative body of client `i`'s character.
    fn host_body(&mut self, id: NetId) -> dark_physics::Body {
        let mut q = self.host.world.query::<(&NetId, &BodyState)>();
        q.iter(&self.host.world)
            .find(|(n, _)| **n == id)
            .map(|(_, b)| b.0)
            .unwrap()
    }

    fn me(&self, i: usize) -> crate::DrawCharacter {
        self.clients[i]
            .characters()
            .into_iter()
            .find(|c| c.you)
            .unwrap()
    }
}

#[test]
fn a_client_gets_a_character_and_its_prediction_matches_the_host() {
    let mut net = Net::new(1, None);
    net.connect();
    assert_eq!(net.clients[0].status(), ClientStatus::InGame);
    let mut avatars = net.host.world.query::<&PlayerAvatar>();
    assert_eq!(avatars.iter(&net.host.world).count(), 1);

    // While walking, every snapshot must agree with what the client predicted for that tick.
    for _ in 0..60 {
        net.run(1, &[Vec2::X]);
        let off = net.clients[0].last_correction();
        assert!(off < 0.01, "prediction was {off} px off the host");
    }
    net.run(30, &[Vec2::ZERO]); // stop, and let the last acks arrive
    let me = net.me(0);
    let host = net.host_body(me.id);
    assert!(
        (host.position.x - 100.0) > 70.0,
        "walked right, host x={}",
        host.position.x
    );
    assert!(
        me.body.position.distance(host.position) < 0.01,
        "prediction {} vs host {}",
        me.body.position,
        host.position
    );
}

#[test]
fn clients_see_each_other_and_only_their_own_map() {
    let mut net = Net::new(2, None);
    net.connect();
    net.run(10, &[Vec2::ZERO, Vec2::ZERO]);
    let a = net.me(0).id;
    let seen_by_b = |net: &Net| net.clients[1].characters().into_iter().find(|c| c.id == a);
    let before = seen_by_b(&net).expect("b sees a").ground;

    net.run(40, &[Vec2::Y, Vec2::ZERO]);
    let after = seen_by_b(&net).expect("b still sees a").ground;
    assert!(
        after.y > before.y + 20.0,
        "a's movement reached b: {before} -> {after}"
    );

    // A walks east through the exit into map b; b must stop receiving a.
    for _ in 0..600 {
        net.run(1, &[Vec2::X, Vec2::ZERO]);
        if net.clients[0].map() == Some(MapId(1)) {
            break;
        }
    }
    assert_eq!(net.clients[0].map(), Some(MapId(1)));
    net.run(30, &[Vec2::ZERO, Vec2::ZERO]);
    assert!(
        seen_by_b(&net).is_none(),
        "b no longer sees a in another map"
    );
    assert_eq!(
        net.clients[0].characters().len(),
        1,
        "a sees only its own map"
    );
}

#[test]
fn prediction_converges_under_latency_jitter_and_loss() {
    let conditions = "40,20,10".parse().unwrap();
    let mut net = Net::new(1, Some(conditions));
    net.connect();
    let mut corrections = Vec::new();
    for step in 0..6 {
        let dir = if step % 2 == 0 { Vec2::X } else { Vec2::Y };
        for _ in 0..25 {
            net.run(1, &[dir]);
            corrections.push(net.clients[0].last_correction());
        }
    }
    // Every input is applied exactly once on the host, so even with latency, jitter and loss the
    // prediction never has to be corrected.
    assert!(
        corrections.iter().all(|&c| c < 0.01),
        "corrections while moving: {corrections:?}"
    );
    net.run(60, &[Vec2::ZERO]);
    let me = net.me(0);
    let host = net.host_body(me.id);
    assert!(
        host.position.distance(Vec2::new(100.0, 100.0)) > 60.0,
        "moved, host at {}",
        host.position
    );
    assert!(
        me.body.position.distance(host.position) < 0.01,
        "prediction {} vs host {}",
        me.body.position,
        host.position
    );
}

#[test]
fn jumps_and_attacks_are_predicted_exactly_under_jitter() {
    let mut net = Net::new(1, Some("40,20,10".parse().unwrap()));
    net.connect();
    let mut corrections = Vec::new();
    let mut airborne = false;
    let mut attacked = false;
    for frame in 0..300 {
        let dir = if frame < 150 { Vec2::X } else { Vec2::Y };
        net.frame(&[dir], frame % 50 == 5, frame % 50 == 40);
        corrections.push(net.clients[0].last_correction());
        let me = net.me(0);
        airborne |= me.body.elevation > 4.0;
        attacked |= matches!(me.state.fighter.action, dark_combat::Action::Attack { .. });
    }
    assert!(
        airborne && attacked,
        "jumped {airborne}, attacked {attacked}"
    );
    assert!(
        corrections.iter().all(|&c| c < 0.01),
        "corrections: {corrections:?}"
    );
    net.run(60, &[Vec2::ZERO]);
    let me = net.me(0);
    let host = net.host_body(me.id);
    assert!(me.body.position.distance(host.position) < 0.01);
    assert!((me.body.elevation - host.elevation).abs() < 0.01);
}

#[test]
fn a_player_who_reconnects_gets_the_same_character_and_moves_again() {
    let mut net = Net::new(1, None);
    net.connect();
    let player = net.clients[0].net_mut().player();
    net.run(30, &[Vec2::X]);
    let id = net.me(0).id;

    net.clients[0].net_mut().quit();
    for _ in 0..300 {
        net.run(1, &[Vec2::ZERO]);
        if net.clients[0].status() == ClientStatus::Disconnected {
            break;
        }
    }
    assert_eq!(net.clients[0].status(), ClientStatus::Disconnected);

    net.reconnect(0, player);
    net.connect();
    assert_eq!(net.me(0).id, id, "same character");
    let mut avatars = net.host.world.query::<&PlayerAvatar>();
    assert_eq!(avatars.iter(&net.host.world).count(), 1);

    let before = net.host_body(id).position;
    net.run(60, &[Vec2::Y]);
    net.run(20, &[Vec2::ZERO]);
    let after = net.host_body(id).position;
    assert!(
        after.y > before.y + 40.0,
        "moved after reconnecting: {before} -> {after}"
    );
    assert!(net.me(0).body.position.distance(after) < 0.01);
}

#[test]
fn hostile_input_messages_are_ignored() {
    use crate::TickInput;
    use crate::replication::ClientInputs;
    use dark_net::{Channel, encode};

    let mut net = Net::new(1, None);
    net.connect();
    let junk = TickInput {
        movement: Vec2::X,
        run: true,
        jump: true,
        attack: true,
        dodge: true,
        interact: true,
        sleep: true,
        relieve: true,
        recruit: true,
        item: 200,
        drop: 200,
        choice: 250,
    };
    let hostile = [
        ClientInputs {
            inputs: vec![(u32::MAX, junk)],
        },
        ClientInputs {
            inputs: (1..=200).map(|s| (s + 100_000, junk)).collect(),
        },
    ];
    for message in &hostile {
        net.clients[0]
            .net_mut()
            .send(Channel::State, encode(message));
    }
    // The honest inputs keep flowing and the host keeps agreeing with them.
    for _ in 0..60 {
        net.run(1, &[Vec2::Y]);
        let off = net.clients[0].last_correction();
        assert!(off < 0.01, "prediction was {off} px off the host");
    }
    net.run(20, &[Vec2::ZERO]);
    let me = net.me(0);
    let host = net.host_body(me.id);
    assert!(
        host.position.y > 130.0,
        "still walks, host at {}",
        host.position
    );
    assert!(me.body.position.distance(host.position) < 0.01);
}

#[test]
fn a_stalled_client_cannot_freeze_its_character_in_the_air() {
    let mut net = Net::new(1, None);
    net.connect();
    net.run(10, &[Vec2::ZERO]);
    let id = net.me(0).id;
    net.frame(&[Vec2::ZERO], true, false);
    for _ in 0..3 {
        net.frame(&[Vec2::ZERO], false, false);
    }
    // The client stops sending (a hang, a dragged window, a cheat) while the host keeps going.
    let mut peak: f32 = 0.0;
    for _ in 0..120 {
        net.host.update(DT);
        peak = peak.max(net.host_body(id).elevation);
    }
    assert!(peak > 0.0, "the jump reached the host");
    let body = net.host_body(id);
    assert!(
        body.grounded && body.elevation == 0.0,
        "landed after the hold ran out: {body:?}"
    );
}

#[test]
fn talking_to_an_npc_shows_its_lines_to_the_client_in_turn() {
    let mut net = Net::new(1, Some("30,10,0".parse().unwrap()));
    net.connect();
    let npc = |net: &Net| {
        net.clients[0]
            .characters()
            .into_iter()
            .find(|c| c.id == SQUIRE)
            .expect("the NPC is replicated")
    };
    assert_eq!(npc(&net).speech, None);

    // Out of reach: nothing happens.
    let press = |net: &mut Net| {
        net.clients[0].update(
            DT,
            TickInput {
                interact: true,
                ..TickInput::default()
            },
        );
        net.run(30, &[Vec2::ZERO]);
    };
    press(&mut net);
    assert_eq!(npc(&net).speech, None);

    // Walk down to it (spawn 100,100; NPC 100,180) and talk twice.
    net.run(40, &[Vec2::Y]);
    press(&mut net);
    let said = npc(&net);
    assert_eq!(said.speech.clone().map(|s| s.line), Some("hello".into()));
    assert_eq!(
        said.state.facing,
        dark_sprite::Facing::Up,
        "turned to the speaker"
    );
    // The reply comes from the one talking, and the NPC falls quiet meanwhile.
    press(&mut net);
    let me = net.me(0);
    let reply = me.speech.clone().expect("the talker replies");
    assert_eq!(reply.line, "thanks");
    assert_eq!(reply.to, Some(said.id));
    assert_eq!(npc(&net).speech, None);
    press(&mut net);
    let bye = npc(&net).speech.expect("the NPC speaks again");
    assert_eq!((bye.line.as_str(), bye.to), ("bye", Some(me.id)));
    assert_eq!(net.me(0).speech, None);

    // A line waits for the next press, however long that takes.
    net.run(crate::SPEECH_TICKS as usize + 30, &[Vec2::ZERO]);
    assert_eq!(npc(&net).speech.map(|s| s.line), Some("bye".into()));

    // The press after the last line closes the conversation, and the NPC faces its own way.
    press(&mut net);
    assert_eq!(npc(&net).speech, None, "closed");
    assert_eq!(net.me(0).speech, None);
    assert_eq!(npc(&net).state.facing, dark_sprite::Facing::Down);

    // The next press starts it again from the top.
    press(&mut net);
    assert_eq!(npc(&net).speech.map(|s| s.line), Some("hello".into()));

    // Walking away ends it too; coming back starts it again.
    press(&mut net);
    assert_eq!(net.me(0).speech.map(|s| s.line), Some("thanks".into()));
    net.run(100, &[Vec2::NEG_Y]);
    assert_eq!(
        net.me(0).speech,
        None,
        "the reply stops when its listener is left"
    );
    assert_eq!(npc(&net).state.facing, dark_sprite::Facing::Down);
    net.run(100, &[Vec2::Y]);
    press(&mut net);
    assert_eq!(npc(&net).speech.map(|s| s.line), Some("hello".into()));
}

#[test]
fn each_player_hears_an_npc_conversation_from_the_start() {
    let mut net = Net::new(2, None);
    net.connect();
    // Both walk down next to the NPC (spawn 100,100; NPC 100,180).
    net.run(40, &[Vec2::Y, Vec2::Y]);
    let talk = |net: &mut Net, who: usize| {
        let mut input = [TickInput::default(), TickInput::default()];
        input[who].interact = true;
        for (client, input) in net.clients.iter_mut().zip(input) {
            client.update(DT, input);
        }
        net.run(30, &[Vec2::ZERO, Vec2::ZERO]);
    };
    let npc_line = |net: &Net| {
        net.clients[0]
            .characters()
            .into_iter()
            .find(|c| c.id == SQUIRE)
            .and_then(|c| c.speech)
            .map(|s| (s.line, s.to))
    };
    talk(&mut net, 0);
    assert_eq!(npc_line(&net), Some(("hello".into(), Some(net.me(0).id))));
    // One conversation at a time: the second player waits.
    talk(&mut net, 1);
    assert_eq!(npc_line(&net), Some(("hello".into(), Some(net.me(0).id))));
    // The first goes on (their reply, the NPC's goodbye) and closes it.
    for _ in 0..3 {
        talk(&mut net, 0);
    }
    assert_eq!(npc_line(&net), None);
    // Then the second starts at the beginning too, not where the first left off.
    talk(&mut net, 1);
    assert_eq!(npc_line(&net), Some(("hello".into(), Some(net.me(1).id))));
}

#[test]
fn one_character_cannot_walk_through_another_and_the_one_standing_still_stays() {
    let mut net = Net::new(2, None);
    net.connect();
    // They spawn on the same spot and part as they walk; then 0 walks back into 1, who stands.
    net.run(30, &[Vec2::X, Vec2::ZERO]);
    let (walker, stander) = (net.me(0).id, net.me(1).id);
    let stood_at = net.host_body(stander).position;
    net.run(90, &[Vec2::NEG_X, Vec2::ZERO]);

    let (a, b) = (net.host_body(walker), net.host_body(stander));
    let apart = a.position.distance(b.position);
    assert!(
        apart >= a.radius + b.radius - 0.5,
        "walked into each other: {apart} px apart"
    );
    assert!(
        b.position.distance(stood_at) < 1.0,
        "the one standing still was shoved {} px",
        b.position.distance(stood_at)
    );
    // The walker's own screen shows it too: its prediction makes the same room.
    assert!(
        net.me(0).ground.distance(a.position) < 2.0,
        "prediction is {} px off",
        net.me(0).ground.distance(a.position)
    );
}

#[test]
fn a_villager_walks_their_day_and_lies_down_where_it_says_they_sleep() {
    // A baker whose day is the far corner of the map, and bed where they started.
    let project = project_with_npc(
        "",
        r#"(sheet: "n", position: (60, 60), lines: ["hm"],
            day: [(from: 0.0, at: (380.0, 60.0)), (from: 23.0, at: (60.0, 60.0), sleep: true)]),"#,
    );
    let mut net = Net::with_project(1, None, project);
    net.connect();
    let baker = |net: &Net| {
        net.clients[0]
            .characters()
            .into_iter()
            .find(|c| c.ground.distance(Vec2::new(100.0, 180.0)) > 1.0 && c.npc && !c.you)
            .filter(|c| c.ground.x > 40.0 && c.ground.y < 140.0)
    };
    let started = baker(&net).expect("the baker is in the map").ground;
    net.run(200, &[Vec2::ZERO]);
    let now = baker(&net).expect("still there").ground;
    assert!(
        now.x > started.x + 30.0,
        "the baker stayed at {started}, now {now}"
    );
    assert!((now.y - 60.0).abs() < 8.0, "wandered off the line: {now}");

    // Their bed is where they began; at bedtime they go back to it and lie down.
    net.host
        .world
        .resource_mut::<crate::WorldClock>()
        .0
        .set_time(0, 23);
    net.run(600, &[Vec2::ZERO]);
    let abed = baker(&net).expect("still there");
    assert!(
        abed.ground.distance(started) < 8.0,
        "went to bed at {}, not {started}",
        abed.ground
    );
    assert!(abed.state.sleeping, "did not lie down");
}

#[test]
fn an_item_dropped_lies_where_it_fell_until_someone_walks_over_it() {
    let mut net = Net::new(1, None);
    net.connect();
    let me = net.me(0).id;
    let carried = |net: &mut Net| net.host_life(me).inventory.count("ale");
    let had = carried(&mut net);
    assert!(had > 0, "a new character starts with ale");

    // Drop one: it lies in front of the character, and cannot be taken back at once.
    net.press(TickInput {
        drop: 1,
        ..TickInput::default()
    });
    assert_eq!(carried(&mut net), had - 1, "one ale left the pack");
    let lying = net.clients[0].drops().to_vec();
    assert_eq!(lying.len(), 1, "one ale on the ground");
    assert_eq!(lying[0].item, "ale");
    // Standing over your own drop does not hand it back, however long you stand there.
    net.run(120, &[Vec2::ZERO]);
    assert_eq!(carried(&mut net), had - 1, "taken back by standing still");
    assert_eq!(net.clients[0].drops().len(), 1, "still lying there");

    // Another of the same joins the pile rather than making a second drop.
    net.press(TickInput {
        drop: 1,
        ..TickInput::default()
    });
    let lying = net.clients[0].drops().to_vec();
    assert_eq!(lying.len(), 1, "one pile, not two drops");
    assert_eq!(lying[0].count, 2, "two ales in it");

    // Walk away and back: whoever walks over it takes the lot.
    net.run(40, &[Vec2::NEG_X]);
    assert_eq!(carried(&mut net), had - 2, "still on the ground");
    net.run(60, &[Vec2::X]);
    assert_eq!(carried(&mut net), had, "both picked up on the way back");
    assert!(
        net.clients[0].drops().is_empty(),
        "and gone from the ground"
    );
}

#[test]
fn the_night_passes_when_every_player_online_sleeps_and_the_world_catches_up() {
    let mut net = Net::new(2, None);
    net.connect();
    let day = |net: &Net| net.host.world.resource::<crate::WorldClock>().0.day();
    let sleep = |net: &mut Net, who: usize| {
        let mut input = [TickInput::default(), TickInput::default()];
        input[who].sleep = true;
        for (client, input) in net.clients.iter_mut().zip(input) {
            client.update(DT, input);
        }
        net.run(30, &[Vec2::ZERO, Vec2::ZERO]);
    };
    assert_eq!(day(&net), 0);

    // One asleep, one awake: the night does not pass.
    sleep(&mut net, 0);
    assert!(net.me(0).state.sleeping);
    assert!(!net.me(1).state.sleeping);
    assert_eq!(day(&net), 0);

    // Both asleep: morning of the next day, everyone up.
    sleep(&mut net, 1);
    assert_eq!(day(&net), 1);
    let clock = &net.host.world.resource::<crate::WorldClock>().0;
    assert!(
        (6.0..6.1).contains(&clock.hour()),
        "just after dawn: {}",
        clock.hour()
    );
    assert!(!net.me(0).state.sleeping && !net.me(1).state.sleeping);
    // The world lived through the night too: its hour matches the clock's.
    let world = net.host.world.resource::<crate::WorldState>();
    assert_eq!(world.sim.hour(), 24 + 6);
    // Both players are in the world simulation, in the capital, which is now run in detail.
    let capital = world.sim.world().region("capital").unwrap();
    let players: Vec<_> = world
        .sim
        .world()
        .actors
        .iter()
        .filter(|a| a.player.is_some())
        .collect();
    assert_eq!(players.len(), 2);
    assert!(players.iter().all(|a| a.region == capital));
    assert!(world.sim.world().region_at(capital).detailed);
}

#[test]
fn a_fight_hurts_both_sides_and_the_dead_enemy_leaves() {
    let mut net = Net::arena(1);
    net.connect();
    let grunt = |net: &Net| net.clients[0].characters().into_iter().find(|c| c.hostile);
    let start = grunt(&net).expect("the grunt is replicated").id;
    assert!(!net.me(0).hostile);

    // Standing still, the grunt comes over and hits.
    let full = net.me(0).state.fighter.health;
    let mut hurt = false;
    for _ in 0..600 {
        net.run(1, &[Vec2::ZERO]);
        if net.me(0).state.fighter.health < full {
            hurt = true;
            break;
        }
    }
    assert!(hurt, "the grunt never landed a hit");
    assert_eq!(net.me(0).state.fighter.hit_by, Some(start.0));

    // Strike back, turning to it each time, until it falls.
    let mut fell = false;
    for frame in 0..900 {
        let Some(g) = grunt(&net) else {
            break;
        };
        if g.state.fighter.is_dead() {
            fell = true;
            break;
        }
        let toward = (g.ground - net.me(0).ground).normalize_or_zero() * 0.02;
        net.clients[0].update(
            DT,
            TickInput {
                movement: toward,
                attack: frame % 6 == 0,
                ..TickInput::default()
            },
        );
        net.host.update(DT);
        std::thread::sleep(Duration::from_micros(300));
    }
    assert!(fell, "the grunt never fell");

    // It lies a while, then is gone from the world (and the snapshots).
    net.run(crate::CORPSE_TICKS as usize + 30, &[Vec2::ZERO]);
    assert!(grunt(&net).is_none(), "a dormant enemy is not replicated");
    // And after all that, prediction agrees with the host again.
    let me = net.me(0);
    let host = net.host_body(me.id);
    assert!(me.body.position.distance(host.position) < 0.01);
}

#[test]
fn a_dead_player_gets_up_at_the_start() {
    let mut net = Net::new(1, None);
    net.connect();
    net.run(40, &[Vec2::X]);
    let id = net.me(0).id;
    // Something kills the player's character (on the host, where hits land).
    {
        let mut q = net
            .host
            .world
            .query::<(&NetId, &mut crate::CharacterState)>();
        let sheets = net.host.world.resource::<CharacterSheets>().clone();
        for (nid, mut state) in q.iter_mut(&mut net.host.world) {
            if *nid == id {
                let m = &sheets.look(state.look).moveset;
                let mut deadly = m.combo[0].clone();
                deadly.damage = 1000;
                state.fighter.take_hit(m, &deadly, Vec2::X, 0);
            }
        }
    }
    net.run(5, &[Vec2::ZERO]);
    assert!(net.me(0).state.fighter.is_dead());
    assert!(net.host_body(id).position.x > 120.0, "fell where it stood");
    net.run(crate::PLAYER_RESPAWN_TICKS as usize + 10, &[Vec2::ZERO]);
    assert!(!net.me(0).state.fighter.is_dead());
    assert_eq!(
        net.host_body(id).position,
        Vec2::new(100.0, 100.0),
        "back at the start"
    );
}

#[test]
fn drink_from_the_hotbar_makes_a_player_stagger_and_their_hud_shows_it() {
    let mut net = Net::new(1, None);
    net.connect();
    let ale = TickInput {
        item: 1,
        ..TickInput::default()
    };
    for _ in 0..4 {
        net.press(ale);
    }
    let life = net.clients[0]
        .life()
        .expect("a player is sent their body")
        .clone();
    assert_eq!(life.slots[0], ("ale".to_owned(), 1));
    assert!(life.alcohol >= 3900, "{}", life.alcohol);
    assert!(
        life.statuses.contains(&Status::Wasted),
        "{:?}",
        life.statuses
    );
    let impaired = net.me(0).state.impaired;
    assert_eq!((impaired.wobble, impaired.speed), (3, 70));

    // Walking east now lurches north and south, slower, and prediction still matches the host.
    let mut ys = Vec::new();
    for _ in 0..120 {
        net.run(1, &[Vec2::X]);
        ys.push(net.me(0).body.position.y);
    }
    assert!(
        ys.iter().any(|&y| y > 100.5) || ys.iter().any(|&y| y < 99.5),
        "no stagger"
    );
    net.run(20, &[Vec2::ZERO]);
    let me = net.me(0);
    assert!(me.body.position.x < 100.0 + 2.1 * crate::WALK_SPEED * 0.7);
    assert!(me.body.position.distance(net.host_body(me.id).position) < 0.01);
}

#[test]
fn a_tent_and_a_fire_set_down_shelter_and_warm_and_the_tent_packs_up() {
    let mut net = Net::new(1, None);
    net.connect();
    let wood = TickInput {
        item: 3,
        ..TickInput::default()
    };
    net.press(TickInput {
        item: 2,
        ..TickInput::default()
    });
    // Set down ahead of her (she faces down).
    let at: Vec<Vec2> = net.clients[0].structures().iter().map(|s| s.at).collect();
    assert_eq!(at, [Vec2::new(100.0, 114.0)]);
    // Not on top of the tent: refused, and the firewood kept.
    net.press(wood);
    assert_eq!(net.clients[0].structures().len(), 1);
    assert_eq!(
        net.clients[0].life().unwrap().slots[2],
        ("wood".to_owned(), 1)
    );
    // Turned east, the fire goes down beside it.
    net.run(1, &[Vec2::X * 0.01]);
    net.press(wood);
    let kinds: Vec<Structure> = net.clients[0]
        .structures()
        .iter()
        .map(|s| s.structure)
        .collect();
    assert_eq!(kinds.len(), 2);
    assert!(kinds.contains(&Structure::Tent));
    // A game minute later she is in the tent, by the fire.
    net.run(70, &[Vec2::ZERO]);
    let life = net.clients[0].life().unwrap().clone();
    assert_eq!(life.shelter, Shelter::Tent);
    assert!(life.fire);
    assert_eq!(life.slots[1], ("tent".to_owned(), 0));

    // Interact packs the tent back up; the fire burns on.
    net.press(TickInput {
        interact: true,
        ..TickInput::default()
    });
    let left: Vec<Structure> = net.clients[0]
        .structures()
        .iter()
        .map(|s| s.structure)
        .collect();
    assert!(matches!(left[..], [Structure::Campfire { .. }]), "{left:?}");
    assert_eq!(
        net.clients[0].life().unwrap().slots[1],
        ("tent".to_owned(), 1)
    );
}

#[test]
fn bodies_sleep_through_the_night_that_passes() {
    let mut net = Net::new(1, None);
    net.connect();
    let id = net.me(0).id;
    net.host_life(id).body.set_need(Need::Fatigue, 800);
    net.press(TickInput {
        sleep: true,
        ..TickInput::default()
    });
    net.run(20, &[Vec2::ZERO]);
    assert_eq!(net.host.world.resource::<crate::WorldClock>().0.day(), 1);
    let life = net.host_life(id);
    assert_eq!(life.body.need(Need::Fatigue), 0, "a full night's rest");
    // Asleep, hunger grows at the sleeping rate: 20 ‰ an hour for the day that passed.
    let hunger = life.body.need(Need::Hunger);
    assert!((475..=485).contains(&hunger), "{hunger}");
}

#[test]
fn exhaustion_knocks_a_player_out_which_counts_as_asleep() {
    let mut net = Net::new(1, None);
    net.connect();
    let id = net.me(0).id;
    net.host_life(id).body.set_need(Need::Fatigue, 1000);
    // Within a game minute she drops; out cold counts as asleep, so the night passes at once.
    net.run(80, &[Vec2::X]);
    assert_eq!(net.host.world.resource::<crate::WorldClock>().0.day(), 1);
    assert!(
        !net.me(0).state.impaired.out,
        "rested, she has come to by morning"
    );
    assert!(net.host_life(id).body.need(Need::Fatigue) < 600);
}

#[test]
fn a_player_who_quits_is_put_to_bed_at_the_inn_where_enemies_leave_them_be() {
    let mut net = Net::arena(1);
    net.connect();
    let id = net.me(0).id;
    net.clients[0].net_mut().quit();
    // The goodbye goes out with the client's next updates.
    for _ in 0..30 {
        net.clients[0].update(DT, TickInput::default());
        net.host.update(DT);
        std::thread::sleep(Duration::from_micros(300));
    }
    assert_eq!(
        net.host_body(id).position,
        Vec2::new(230.0, 220.0),
        "in bed"
    );
    let health = |net: &mut Net| {
        let mut q = net.host.world.query::<(&NetId, &crate::CharacterState)>();
        q.iter(&net.host.world)
            .find(|(n, _)| **n == id)
            .map(|(_, s)| s.fighter.health)
            .unwrap()
    };
    let before = health(&mut net);
    let body = net.host_life(id).body;
    // The grunt (post at 140,100, sight 200) goes home and stays there.
    for _ in 0..400 {
        net.host.update(DT);
    }
    assert_eq!(health(&mut net), before);
    assert_eq!(
        net.host_life(id).body,
        body,
        "away, the body is kept as it was"
    );
    let mut q = net
        .host
        .world
        .query_filtered::<&BodyState, bevy_ecs::query::With<crate::Hostile>>();
    let grunt = q.single(&net.host.world).unwrap().0.position;
    assert!(grunt.distance(Vec2::new(140.0, 100.0)) < 4.0, "{grunt}");
}

/// Client 0 walks up to the squire (standing at 100,180) from the spawn.
fn walk_to_the_squire(net: &mut Net) {
    let still = vec![Vec2::ZERO; net.clients.len()];
    let mut down = still.clone();
    down[0] = Vec2::Y;
    net.run(42, &down);
    net.run(10, &still);
}

fn squire(net: &Net) -> crate::DrawCharacter {
    net.clients[0]
        .characters()
        .into_iter()
        .find(|c| c.id == SQUIRE)
        .expect("the squire is in view")
}

fn world(net: &mut Net) -> Mut<'_, crate::WorldState> {
    net.host.world.resource_mut::<crate::WorldState>()
}

#[test]
fn a_person_asked_along_follows_the_player_and_stays_when_sent_away() {
    let mut net = Net::new(1, None);
    net.connect();
    walk_to_the_squire(&mut net);
    let ask = TickInput {
        recruit: true,
        ..TickInput::default()
    };
    net.press(ask);
    let s = squire(&net);
    assert_eq!(
        s.speech.as_ref().map(|s| s.line.as_str()),
        Some("party.yes")
    );
    assert_eq!(net.clients[0].party(), [s.id]);
    // East a good way: the squire keeps up.
    net.run(120, &[Vec2::X]);
    net.run(90, &[Vec2::ZERO]);
    let (me, s) = (net.me(0), squire(&net));
    assert!(me.ground.x > 200.0);
    assert!(
        s.ground.distance(me.ground) < 45.0,
        "{} vs {}",
        s.ground,
        me.ground
    );
    // Sent away, it stays where it is.
    net.press(ask);
    assert_eq!(
        squire(&net).speech.as_ref().map(|s| s.line.as_str()),
        Some("party.farewell")
    );
    assert!(net.clients[0].party().is_empty());
    let left_at = squire(&net).ground;
    net.run(60, &[Vec2::NEG_X]);
    assert!(squire(&net).ground.distance(left_at) < 1.0);
}

#[test]
fn a_stranger_who_does_not_trust_the_player_says_no() {
    let mut net = Net::new(1, None);
    net.connect();
    // To the hermit at (300, 250), who wants a far better name than a newcomer has.
    let to = (Vec2::new(300.0, 236.0) - Vec2::new(100.0, 100.0)).normalize();
    net.run(185, &[to]);
    net.run(10, &[Vec2::ZERO]);
    net.press(TickInput {
        recruit: true,
        ..TickInput::default()
    });
    let hermit = net.clients[0]
        .characters()
        .into_iter()
        .find(|c| c.npc && c.speech.is_some())
        .expect("the hermit answers");
    assert_eq!(
        hermit.speech.as_ref().map(|s| s.line.as_str()),
        Some("party.no.distrust")
    );
    assert!(net.clients[0].party().is_empty());
}

#[test]
fn a_follower_shamed_by_its_leader_deserts_and_one_wronged_enough_turns_on_them() {
    let mut net = Net::new(1, None);
    net.connect();
    walk_to_the_squire(&mut net);
    let ask = TickInput {
        recruit: true,
        ..TickInput::default()
    };
    net.press(ask);
    let (me, squire_actor) = (
        net.me(0).id,
        world(&mut net).sim.world().actor("squire").unwrap(),
    );
    // Loyalty down to 50; then she wets herself in front of him.
    world(&mut net).sim.sway(squire_actor, -500);
    net.host_life(me).body.set_need(Need::Bladder, 1000);
    net.run(80, &[Vec2::ZERO]);
    let s = squire(&net);
    assert_eq!(
        s.speech.as_ref().map(|s| s.line.as_str()),
        Some("react.accident")
    );
    assert!(net.clients[0].party().is_empty(), "he has had enough");
    assert!(!s.hostile);

    // He left at about −100: at once he refuses; some twelve days on he has forgiven enough.
    net.press(ask);
    assert_eq!(
        squire(&net).speech.as_ref().map(|s| s.line.as_str()),
        Some("party.no.grudge")
    );
    world(&mut net).sim.advance_hours(24 * 12);
    net.run(30, &[Vec2::ZERO]);
    // Wronged beyond forgiving, he turns on her.
    net.press(ask);
    assert_eq!(net.clients[0].party().len(), 1);
    world(&mut net).sim.sway(squire_actor, -2000);
    net.run(10, &[Vec2::ZERO]);
    let s = squire(&net);
    assert!(s.hostile, "a traitor is an enemy");
    assert!(net.clients[0].party().is_empty());
    assert_eq!(
        s.speech.as_ref().map(|s| s.line.as_str()),
        Some("party.betrayed")
    );
}

#[test]
fn two_players_band_together_by_asking_each_other() {
    let mut net = Net::new(2, None);
    net.connect();
    let ask = TickInput {
        recruit: true,
        ..TickInput::default()
    };
    // Both stand at the spawn: the first asks, the second answers.
    net.press(ask);
    let says = |net: &Net, i: usize| net.me(i).speech.map(|s| s.line);
    assert_eq!(says(&net, 0).as_deref(), Some("party.invite"));
    net.clients[1].update(DT, ask);
    net.host.update(DT);
    net.run(10, &[Vec2::ZERO, Vec2::ZERO]);
    assert_eq!(says(&net, 1).as_deref(), Some("party.together"));
    let (a, b) = (net.me(0).id, net.me(1).id);
    assert_eq!(net.clients[0].party(), [b]);
    assert_eq!(net.clients[1].party(), [a]);
    // The second walks off and leaves.
    net.run(40, &[Vec2::ZERO, Vec2::X]);
    net.clients[1].update(DT, ask);
    net.host.update(DT);
    net.run(10, &[Vec2::ZERO, Vec2::ZERO]);
    assert_eq!(says(&net, 1).as_deref(), Some("party.leave"));
    assert!(net.clients[0].party().is_empty() && net.clients[1].party().is_empty());
}

#[test]
fn slaying_a_monster_earns_the_player_standing() {
    let mut net = Net::arena(1);
    net.connect();
    let me = net.me(0).id;
    let kingdom = world(&mut net).sim.world().faction("kingdom").unwrap();
    let grunt = net.clients[0]
        .characters()
        .into_iter()
        .find(|c| c.hostile)
        .unwrap()
        .id;
    // The player's blow kills it (on the host, where hits land).
    {
        let sheets = net.host.world.resource::<CharacterSheets>().clone();
        let mut q = net
            .host
            .world
            .query::<(&NetId, &mut crate::CharacterState)>();
        for (id, mut state) in q.iter_mut(&mut net.host.world) {
            if *id == grunt {
                let m = &sheets.look(state.look).moveset;
                let mut deadly = m.combo[0].clone();
                deadly.damage = 1000;
                state.fighter.take_hit(m, &deadly, Vec2::X, me.0);
            }
        }
    }
    net.run(5, &[Vec2::ZERO]);
    let w = world(&mut net);
    let actor = w
        .sim
        .world()
        .actors
        .iter()
        .position(|a| a.player.is_some())
        .map(|i| dark_sim::ActorId(i as u16))
        .unwrap();
    assert_eq!(w.sim.world().standing(actor, kingdom), 505);
    // And their screen is told, for the menu.
    net.run(10, &[Vec2::ZERO]);
    assert!(
        net.clients[0]
            .story()
            .standing
            .contains(&("Kingdom".into(), 505)),
        "{:?}",
        net.clients[0].story().standing
    );
}

#[test]
fn a_pause_holds_the_world_until_someone_else_is_online() {
    let mut net = Net::with_world(Vec::new(), None, project(), None);
    net.host.insert_resource(crate::Pause(true));
    let hour = |net: &Net| net.host.world.resource::<crate::WorldClock>().0.hour();
    let start = hour(&net);
    net.run(120, &[]);
    assert!(crate::paused(&net.host.world));
    assert_eq!(hour(&net), start, "the clock stands still");
    // Someone joining is let in, and the world goes on while they are there.
    net.clients.push(ClientSession::new(
        RemoteClient::connect(net.addr, PlayerId::random()).unwrap(),
        maps(&net.project),
        looks(&net.project),
    ));
    net.connect();
    assert!(!crate::paused(&net.host.world));
    net.run(60, &[Vec2::X]);
    assert!(hour(&net) > start);
    assert!(net.me(0).body.position.x > 101.0, "they walk");
}

#[test]
fn a_saved_world_brings_back_characters_bodies_packs_tents_and_people() {
    let project = project();
    let player = PlayerId::random();
    let mut net = Net::with_world(vec![player], None, project.clone(), None);
    net.connect();
    walk_to_the_squire(&mut net);
    // Hungry, a tent pitched, the squire asked along; then east a way, where he follows.
    let id = net.me(0).id;
    net.host_life(id).body.set_need(Need::Hunger, 640);
    net.press(TickInput {
        item: 2,
        ..TickInput::default()
    });
    net.press(TickInput {
        recruit: true,
        ..TickInput::default()
    });
    net.run(90, &[Vec2::X]);
    net.run(60, &[Vec2::ZERO]);
    let (at, health) = (net.me(0).body.position, net.me(0).state.fighter.health);
    let squire_at = squire(&net).ground;

    let sim = world(&mut net).sim.clone();
    let saved = crate::WorldSave::gather(&mut net.host.world, sim);
    let text = saved.to_ron().unwrap();
    let saved = crate::WorldSave::parse(&text).unwrap();
    assert_eq!(saved.characters.len(), 1);
    assert_eq!(saved.structures.len(), 1);
    drop(net);

    // A new host carries the world on; the same player comes back to the same character.
    let mut net = Net::with_world(vec![player], None, project, Some(saved));
    net.connect();
    net.run(10, &[Vec2::ZERO]);
    let me = net.me(0);
    assert!(
        me.body.position.distance(at) < 0.01,
        "{} vs {at}",
        me.body.position
    );
    assert_eq!(me.state.fighter.health, health);
    let life = net.clients[0].life().unwrap().clone();
    assert!(life.needs[0] >= 640, "still hungry: {:?}", life.needs);
    assert_eq!(life.slots[1], ("tent".to_owned(), 0));
    assert_eq!(
        net.clients[0].structures().len(),
        1,
        "the tent still stands"
    );
    // The squire is where he was, and follows her again.
    assert!(squire(&net).ground.distance(squire_at) < 20.0);
    assert_eq!(net.clients[0].party(), [SQUIRE]);
}

/// Client 0 walks from the spawn to the hermit at (300, 250).
fn walk_to_the_hermit(net: &mut Net) {
    let to = (Vec2::new(300.0, 236.0) - Vec2::new(100.0, 100.0)).normalize();
    net.run(185, &[to]);
    net.run(10, &[Vec2::ZERO]);
}

fn said(net: &Net, pred: impl Fn(&crate::DrawCharacter) -> bool) -> Option<String> {
    net.clients[0]
        .characters()
        .into_iter()
        .find(|c| pred(c))
        .and_then(|c| c.speech)
        .map(|s| s.line)
}

#[test]
fn a_storylet_offers_choices_and_what_is_chosen_happens() {
    let mut net = Net::new(1, None);
    net.connect();
    walk_to_the_hermit(&mut net);
    let press = |net: &mut Net, input: TickInput| net.press(input);
    let talk = TickInput {
        interact: true,
        ..TickInput::default()
    };
    let choose = |n: u8| TickInput {
        choice: n,
        ..TickInput::default()
    };
    let hermit = |c: &crate::DrawCharacter| c.npc && c.id != SQUIRE;
    press(&mut net, talk);
    assert_eq!(said(&net, hermit).as_deref(), Some("hm.hello"));
    let says = |net: &Net| -> Vec<String> {
        net.clients[0]
            .story()
            .choices
            .into_iter()
            .map(|(_, s)| s)
            .collect()
    };
    assert_eq!(says(&net), ["hm.gift", "hm.wed", "hm.dark"]);
    // A gift: she thanks him for it, he thanks her back; two more ales in the pack.
    press(&mut net, choose(1));
    assert_eq!(said(&net, |c| c.you).as_deref(), Some("hm.gift"));
    assert!(
        net.clients[0].story().choices.is_empty(),
        "no choosing while answering"
    );
    net.run(100, &[Vec2::ZERO]);
    assert_eq!(said(&net, hermit).as_deref(), Some("hm.thanks"));
    assert_eq!(
        net.clients[0].life().unwrap().slots[0],
        ("ale".to_owned(), 7)
    );
    // The last line: interact ends it (without plain talk), and again starts it over.
    press(&mut net, talk);
    assert_eq!(
        said(&net, hermit),
        None,
        "heard out; he does not start on his plain lines"
    );
    press(&mut net, talk);
    assert_eq!(said(&net, hermit).as_deref(), Some("hm.hello"));
    // A wedding: the screen fades, and the choice is gone once married.
    press(&mut net, choose(2));
    net.run(100, &[Vec2::ZERO]);
    assert_eq!(net.clients[0].story().faded, 1);
    press(&mut net, talk);
    assert_eq!(says(&net), ["hm.gift", "hm.dark"]);
    // Choices are sent by the node's own index: the pact is still the third.
    assert_eq!(net.clients[0].story().choices[1].0, 2);
    press(&mut net, choose(3));
    net.run(10, &[Vec2::ZERO]);
    assert!(net.me(0).hostile, "gone over to the enemy");
}

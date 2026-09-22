//! Whole-year behaviour on a small world.

use super::*;

/// A capital, a village, wilds, a general's lair and the lord's keep.
pub(crate) const TEST_WORLD: &str = r#"(
    regions: [
        (id: "capital", name: "Capital", kind: City, danger: 0, inn: true),
        (id: "village", name: "Village", kind: Village, danger: 1, inn: true),
        (id: "wilds", name: "Wilds", kind: Wilds, danger: 4),
        (id: "lair", name: "Lair", kind: Dungeon, danger: 5),
        (id: "keep", name: "Keep", kind: Fortress, danger: 6),
    ],
    roads: [
        (between: ("capital", "village"), hours: 8),
        (between: ("village", "wilds"), hours: 12),
        (between: ("wilds", "lair"), hours: 10),
        (between: ("wilds", "keep"), hours: 20),
    ],
    factions: [
        (id: "kingdom", name: "Kingdom"),
        (id: "church", name: "Church"),
        (id: "demons", name: "Demons", hostile: true),
    ],
    titles: [
        (id: "hero", name: "the Hero", succession: (to_killer: true, appointed: (faction: "kingdom", role: "warrior"))),
        (id: "high_priest", name: "the High Priest", succession: (appointed: (faction: "church", role: "priest"))),
        (id: "demon_lord", name: "the Demon Lord"),
    ],
    actors: [
        (id: "hero", name: "Aldric", role: "warrior", faction: "kingdom", power: 40, home: "capital", titles: ["hero"]),
        (id: "mage", name: "Selene", role: "mage", faction: "kingdom", power: 45, home: "capital"),
        (id: "priest", name: "Bram", role: "priest", faction: "church", power: 30, home: "capital", titles: ["high_priest"]),
        (id: "knight", name: "Garrick", role: "warrior", faction: "kingdom", power: 25, home: "capital"),
        (id: "hedge_mage", name: "Fenna", role: "mage", faction: "kingdom", power: 20, home: "village"),
        (id: "acolyte", name: "Oswin", role: "priest", faction: "church", power: 15, home: "village"),
        (id: "general", name: "Hrimgar", role: "general", faction: "demons", power: 150, home: "lair", boss: true),
        (id: "lord", name: "Vorgath", role: "demon_lord", faction: "demons", power: 500, home: "keep", titles: ["demon_lord"], boss: true),
    ],
    hero_party: (
        leader_title: "hero",
        roles: ["warrior", "mage", "priest"],
        members: ["hero", "mage", "priest"],
        goal: "lord",
        lieutenants: ["general"],
    ),
)"#;

pub(crate) fn sim(seed: u64) -> WorldSim {
    WorldSim::new(&WorldDef::parse(TEST_WORLD).unwrap(), seed).unwrap()
}

fn names(_: &str) -> String {
    String::new()
}

fn id(sim: &WorldSim, name: &str) -> ActorId {
    sim.world().actor(name).unwrap()
}

#[test]
fn a_seed_gives_the_same_year_every_time() {
    let run = |seed| {
        let mut s = sim(seed);
        s.run_year();
        (s.take_events(), s.save().unwrap())
    };
    assert_eq!(run(1), run(1));
    assert_ne!(run(1).0, run(2).0, "different seeds, different years");
}

#[test]
fn a_year_fast_forwards_quickly_and_ends_with_an_outcome() {
    let start = std::time::Instant::now();
    let mut s = sim(7);
    s.run_year();
    assert!(s.is_year_over());
    assert!(s.outcome().is_some());
    let events = s.take_events();
    assert!(matches!(
        events.last().map(|e| &e.what),
        Some(Happening::YearEnded { .. })
    ));
    // Every event can be told.
    for e in &events {
        assert!(!s.describe(e, &names).is_empty());
    }
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "a year took {:?}",
        start.elapsed()
    );
}

#[test]
fn the_party_plans_trains_hunts_and_eventually_faces_the_goal() {
    // Across seeds, the party must at least fight the lieutenant, and the goal must fall in
    // some years and survive in others: the year is a contest, not a script.
    let mut won = 0;
    let mut fought_general = 0;
    for seed in 0..40 {
        let mut s = sim(seed);
        s.run_year();
        let general = id(&s, "general");
        let events = s.take_events();
        if events
            .iter()
            .any(|e| matches!(e.what, Happening::Battle { boss, .. } if boss == general))
        {
            fought_general += 1;
        }
        if matches!(s.outcome(), Some(Outcome::GoalDefeated { .. })) {
            won += 1;
        }
    }
    assert!(
        fought_general > 30,
        "fought the general in {fought_general}/40"
    );
    assert!((4..40).contains(&won), "the goal fell in {won}/40 years");
}

#[test]
fn a_hero_killed_by_a_person_passes_the_title_to_the_killer() {
    let mut s = sim(1);
    let (hero, knight) = (id(&s, "hero"), id(&s, "knight"));
    let title = s.world().title("hero").unwrap();
    s.kill(hero, Some(knight));
    assert_eq!(s.world().holder(title), Some(knight));
    let events = s.take_events();
    assert!(
        events
            .iter()
            .any(|e| e.what == Happening::TitlePassed { title, to: knight })
    );
}

#[test]
fn a_title_without_a_worthy_killer_is_appointed_or_falls_vacant() {
    let mut s = sim(1);
    let hero_title = s.world().title("hero").unwrap();
    // Killed by monsters: the kingdom appoints its strongest free warrior.
    s.kill(id(&s, "hero"), None);
    assert_eq!(s.world().holder(hero_title), Some(id(&s, "knight")));
    // Killed again with no warrior left: vacant.
    s.kill(id(&s, "knight"), None);
    assert_eq!(s.world().holder(hero_title), None);
    // No succession at all: the demon lord's title simply ends with him.
    let lord_title = s.world().title("demon_lord").unwrap();
    s.kill(id(&s, "lord"), Some(id(&s, "mage")));
    assert_eq!(s.world().holder(lord_title), None);
    assert!(s.outcome().is_none(), "decided when the hour runs");
    s.advance_hours(1);
    assert!(matches!(
        s.outcome(),
        Some(Outcome::GoalDefeated { day: 0 })
    ));
}

#[test]
fn a_dead_member_is_replaced_by_someone_of_the_same_role() {
    let mut s = sim(3);
    let mage = id(&s, "mage");
    s.kill(mage, None);
    s.advance_hours(24 * 4);
    let hedge_mage = id(&s, "hedge_mage");
    assert!(
        s.party().members.contains(&hedge_mage),
        "the only other mage joined: {:?}, intent {:?}",
        s.party().members,
        s.party().intent
    );
}

#[test]
fn a_player_who_kills_the_hero_leads_the_party_and_it_follows_them() {
    let mut s = sim(5);
    let village = s.world().region("village").unwrap();
    let capital = s.world().region("capital").unwrap();
    let player = s.player_actor(42, capital);
    assert_eq!(s.player_actor(42, village), player, "one actor per player");
    s.kill(id(&s, "hero"), Some(player));
    let hero_title = s.world().title("hero").unwrap();
    assert_eq!(s.world().holder(hero_title), Some(player));
    assert!(s.join_hero_party(player));
    assert!(!s.join_hero_party(player), "once");
    s.move_player(player, village);
    s.advance_hours(30);
    assert_eq!(s.party().intent, "follow the player leading");
    assert_eq!(
        s.party().location,
        Location::At(village),
        "the party came to the player"
    );
}

#[test]
fn players_are_invited_to_fill_a_missing_role() {
    let mut s = sim(2);
    let capital = s.world().region("capital").unwrap();
    // Every other priest is gone, so the party is a priest short and nobody can fill it.
    s.kill(id(&s, "priest"), None);
    s.kill(id(&s, "acolyte"), None);
    let player = s.player_actor(7, capital);
    s.advance_hours(1);
    let invited: Vec<_> = s
        .take_events()
        .into_iter()
        .filter(|e| matches!(&e.what, Happening::Invited { actor, role } if *actor == player && role == "priest"))
        .collect();
    assert_eq!(invited.len(), 1);
    s.advance_hours(48);
    assert!(
        !s.take_events()
            .iter()
            .any(|e| matches!(e.what, Happening::Invited { .. })),
        "asked once"
    );
}

#[test]
fn no_random_encounters_where_players_are_watching() {
    let count = |detailed: bool| {
        let mut s = sim(11);
        if detailed {
            let all: Vec<RegionId> = (0..s.world().regions.len())
                .map(|i| RegionId(i as u16))
                .collect();
            s.set_detailed(&all);
        }
        s.advance_hours(24 * 60);
        s.take_events()
            .iter()
            .filter(|e| matches!(e.what, Happening::Encounter { .. }))
            .count()
    };
    assert!(count(false) > 0);
    assert_eq!(count(true), 0);
}

#[test]
fn a_saved_world_carries_on_exactly_as_if_never_saved() {
    let mut a = sim(9);
    // With a player in it: their id is a u128.
    let capital = a.world().region("capital").unwrap();
    a.player_actor(u128::MAX - 5, capital);
    a.advance_hours(24 * 50);
    let mut b = WorldSim::load(&a.save().unwrap()).unwrap();
    a.run_year();
    b.run_year();
    assert_eq!(a, b);
}

#[test]
fn broken_definitions_are_refused() {
    let broken = [
        (
            r#"home: "capital", titles: ["hero"])"#,
            r#"home: "nowhere", titles: ["hero"])"#,
        ),
        (r#"goal: "lord""#, r#"goal: "nobody""#),
        (
            r#"(between: ("wilds", "keep"), hours: 20)"#,
            r#"(between: ("wilds", "keep"), hours: 0)"#,
        ),
        (r#"titles: ["high_priest"]"#, r#"titles: ["hero"]"#),
    ];
    for (from, to) in broken {
        let text = TEST_WORLD.replacen(from, to, 1);
        assert_ne!(text, TEST_WORLD, "test edit {from:?} applies");
        let def = WorldDef::parse(&text).unwrap();
        assert!(WorldSim::new(&def, 0).is_err(), "accepted {to:?}");
    }
}

#[test]
fn a_boss_who_kills_the_hero_does_not_become_the_hero() {
    let mut s = sim(1);
    let hero_title = s.world().title("hero").unwrap();
    s.kill(id(&s, "hero"), Some(id(&s, "general")));
    assert_eq!(
        s.world().holder(hero_title),
        Some(id(&s, "knight")),
        "the kingdom appoints instead"
    );
}

#[test]
fn no_boss_ever_joins_the_hero_party() {
    for seed in 0..60 {
        let mut s = sim(seed);
        s.run_year();
        for e in s.take_events() {
            if let Happening::Joined { actor } | Happening::TitlePassed { to: actor, .. } = e.what {
                assert!(
                    !s.world().get(actor).boss,
                    "seed {seed}: {} joined or took a title",
                    s.world().get(actor).name
                );
            }
        }
    }
}

#[test]
fn the_party_follows_a_player_leader_only_while_they_are_online() {
    let mut s = sim(5);
    let capital = s.world().region("capital").unwrap();
    let village = s.world().region("village").unwrap();
    let player = s.player_actor(42, capital);
    s.kill(id(&s, "hero"), Some(player));
    s.join_hero_party(player);
    s.move_player(player, village);
    s.set_online(&[]);
    s.advance_hours(30);
    assert_ne!(s.party().intent, "follow the player leading");
    s.set_online(&[player]);
    s.advance_hours(30);
    assert_eq!(s.party().intent, "follow the player leading");
}

#[test]
fn players_never_die_in_statistical_fights_and_a_party_of_players_does_not_fight_alone() {
    let mut s = sim(4);
    let capital = s.world().region("capital").unwrap();
    let player = s.player_actor(9, capital);
    s.join_hero_party(player);
    // The NPCs fall; only the player is left, and nobody can be recruited.
    for name in ["hero", "mage", "priest", "knight", "hedge_mage", "acolyte"] {
        s.kill(id(&s, name), None);
    }
    s.take_events();
    s.run_year();
    assert!(
        s.world().get(player).alive,
        "the player's character is untouched"
    );
    let events = s.take_events();
    assert!(
        !events.iter().any(|e| matches!(
            e.what,
            Happening::Battle { .. } | Happening::Encounter { .. }
        )),
        "no fights without NPC fighters"
    );
    assert_eq!(s.party().intent, "wait for its players");
}

/// A player (actor) in the village, where Fenna (kingdom mage) and Oswin (church acolyte) live.
fn with_player(sim: &mut WorldSim, player: u128) -> ActorId {
    let village = sim.world().region("village").unwrap();
    sim.player_actor(player, village)
}

fn happened(sim: &mut WorldSim, what: impl Fn(&Happening) -> bool) -> bool {
    sim.take_events().iter().any(|e| what(&e.what))
}

#[test]
fn npcs_follow_a_player_they_trust_and_the_director_leaves_them_be() {
    let def = TEST_WORLD.replace(
        r#"(id: "acolyte", name: "Oswin", role: "priest", faction: "church", power: 15, home: "village")"#,
        r#"(id: "acolyte", name: "Oswin", role: "priest", faction: "church", power: 15, home: "village", trust: 100)"#,
    );
    let mut sim = WorldSim::new(&WorldDef::parse(&def).unwrap(), 1).unwrap();
    let me = with_player(&mut sim, 7);
    let (fenna, oswin) = (id(&sim, "hedge_mage"), id(&sim, "acolyte"));
    // Fenna is of the player's own faction: they are well regarded, she comes.
    assert_eq!(sim.ask_to_join(me, fenna), Ok(()));
    let party = sim.party_of(me).unwrap();
    assert_eq!((party.leader, party.members.clone()), (me, vec![me, fenna]));
    assert_eq!(party.loyalty(fenna), Some(300 + 500 / 2));
    assert!(sim.world().get(fenna).in_party);
    // The church does not know the player yet.
    assert_eq!(sim.ask_to_join(me, oswin), Err(Refusal::Distrust));
    // Slaying the demon army's creatures earns the church's regard.
    for _ in 0..20 {
        sim.slain_enemy(me);
    }
    assert_eq!(
        sim.world()
            .standing(me, sim.world().faction("church").unwrap()),
        100
    );
    assert_eq!(sim.ask_to_join(me, oswin), Ok(()));
    // Who cannot come: the hero party's own, the far away, bosses.
    let (mage, knight, general) = (id(&sim, "mage"), id(&sim, "knight"), id(&sim, "general"));
    assert_eq!(sim.ask_to_join(me, mage), Err(Refusal::Busy));
    assert_eq!(sim.ask_to_join(me, knight), Err(Refusal::Away));
    assert_eq!(sim.ask_to_join(me, general), Err(Refusal::Unable));
    // A year on, the hero party never took the player's followers.
    sim.run_year();
    assert!(!sim.party().members.contains(&fenna) && !sim.party().members.contains(&oswin));
    // And the parties survive a save.
    let saved = WorldSim::load(&sim.save().unwrap()).unwrap();
    assert_eq!(saved.parties(), sim.parties());
}

#[test]
fn killing_changes_standing_murder_most_of_all() {
    let mut sim = sim(1);
    let me = with_player(&mut sim, 7);
    let (kingdom, church) = (
        sim.world().faction("kingdom").unwrap(),
        sim.world().faction("church").unwrap(),
    );
    assert_eq!(
        sim.world().standing(me, kingdom),
        500,
        "the players' faction"
    );
    sim.kill(id(&sim, "hedge_mage"), Some(me));
    assert_eq!(sim.world().standing(me, kingdom), 100);
    assert_eq!(
        sim.world().standing(me, church),
        -150,
        "murder is frowned on everywhere"
    );
    // Slaying a demon general is not murder.
    let demons = sim.world().faction("demons").unwrap();
    let before = sim.world().standing(me, demons);
    sim.kill(id(&sim, "general"), Some(me));
    assert_eq!(sim.world().standing(me, demons), before - 400);
    assert_eq!(sim.world().standing(me, church), -150 + 5);
}

#[test]
fn followers_grow_loyal_but_leave_or_turn_on_a_leader_who_shames_them() {
    let mut sim = sim(1);
    let me = with_player(&mut sim, 7);
    let fenna = id(&sim, "hedge_mage");
    sim.ask_to_join(me, fenna).unwrap();
    let start = sim.party_of(me).unwrap().loyalty(fenna).unwrap();
    // Each midnight, a little more loyal.
    sim.advance_hours(24);
    assert_eq!(sim.party_of(me).unwrap().loyalty(fenna), Some(start + 10));
    // Shamed below zero she leaves; the party of one breaks up.
    sim.take_events();
    sim.sway(fenna, -(start + 10) - 1);
    assert!(happened(&mut sim, |w| matches!(
        w,
        Happening::LeftParty {
            how: Parting::Deserted,
            ..
        }
    )));
    assert!(sim.party_of(me).is_none() && !sim.world().get(fenna).in_party);
    // She holds it against the player for a while: asking again at once mends nothing.
    assert_eq!(sim.ask_to_join(me, fenna), Err(Refusal::Grudge));
    sim.advance_hours(24);
    assert_eq!(sim.ask_to_join(me, fenna), Ok(()));
    assert_eq!(
        sim.party_of(me).unwrap().loyalty(fenna),
        Some(9),
        "as loyal as she left, a day's forgiveness later"
    );
    // Another player joins; shamed far worse, she turns on them both. The kingdom holds it
    // against her once, not once per player.
    let other = with_player(&mut sim, 8);
    sim.join_players(me, other).unwrap();
    let kingdom = sim.world().faction("kingdom").unwrap();
    let before = sim.world().standing(fenna, kingdom);
    sim.sway(fenna, -2000);
    assert!(happened(&mut sim, |w| matches!(
        w,
        Happening::Betrayed { .. }
    )));
    assert_eq!(sim.world().standing(fenna, kingdom), before - 600);
    // An outlaw now: nobody takes her in, not even the hero party.
    assert!(sim.world().get(fenna).outlaw);
    assert_eq!(sim.ask_to_join(other, fenna), Err(Refusal::Enemy));
    sim.run_year();
    assert!(!sim.party().members.contains(&fenna));
}

#[test]
fn players_band_together_and_leadership_passes_to_a_player() {
    let mut sim = sim(1);
    let (a, b, c) = (
        with_player(&mut sim, 1),
        with_player(&mut sim, 2),
        with_player(&mut sim, 3),
    );
    let (fenna, oswin) = (id(&sim, "hedge_mage"), id(&sim, "acolyte"));
    assert_eq!(sim.join_players(a, b), Ok(()));
    sim.ask_to_join(a, fenna).unwrap();
    assert_eq!(sim.ask_to_join(b, oswin), Err(Refusal::NotLeader));
    sim.ask_to_join(c, oswin).unwrap();
    // Two parties merge into the first's.
    assert_eq!(sim.join_players(a, c), Ok(()));
    let party = sim.party_of(a).unwrap();
    assert_eq!(party.members, vec![a, b, fenna, c, oswin]);
    assert_eq!(sim.parties().len(), 1);
    // The leader leaves: the next player leads, the NPCs stay.
    sim.leave_party(a);
    assert_eq!(sim.party_of(b).unwrap().leader, b);
    assert!(sim.dismiss(b, oswin));
    assert!(!sim.dismiss(c, fenna), "only the leader sends people away");
    sim.leave_party(b);
    sim.take_events();
    // The last player leaves: Fenna has nobody to follow.
    sim.leave_party(c);
    assert!(happened(&mut sim, |w| matches!(
        w,
        Happening::LeftParty {
            how: Parting::Disbanded,
            ..
        }
    )));
    assert!(sim.parties().is_empty());
}

#[test]
fn going_over_to_the_demons_costs_titles_standing_and_friends() {
    let mut sim = sim(1);
    let aldric = id(&sim, "hero");
    let (kingdom, demons) = (
        sim.world().faction("kingdom").unwrap(),
        sim.world().faction("demons").unwrap(),
    );
    sim.defect(aldric, demons);
    let hero = sim.world().title("hero").unwrap();
    assert_eq!(
        sim.world().holder(hero),
        Some(id(&sim, "knight")),
        "the crown names another"
    );
    assert!(sim.world().standing(aldric, kingdom) <= -500);
    assert_eq!(sim.world().standing(aldric, demons), 500);
    assert!(!sim.party().members.contains(&aldric));
    // A player who goes over betrays the party they were in.
    let me = with_player(&mut sim, 7);
    let fenna = id(&sim, "hedge_mage");
    sim.ask_to_join(me, fenna).unwrap();
    sim.take_events();
    sim.defect(me, demons);
    assert!(happened(&mut sim, |w| matches!(
        w,
        Happening::Betrayed { .. }
    )));
    assert!(sim.party_of(fenna).is_none());
    // And the other side will not follow them now.
    assert_eq!(sim.ask_to_join(me, fenna), Err(Refusal::Enemy));
}

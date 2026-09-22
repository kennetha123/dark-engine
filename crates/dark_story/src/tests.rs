use super::*;
use dark_sim::WorldDef;

const WORLD: &str = r#"(
    regions: [
        (id: "village", name: "Village", kind: Village, danger: 0, inn: true),
        (id: "keep", name: "Keep", kind: Fortress, danger: 5),
    ],
    roads: [(between: ("village", "keep"), hours: 5)],
    factions: [
        (id: "kingdom", name: "Kingdom"),
        (id: "church", name: "Church"),
        (id: "demons", name: "Demons", hostile: true),
    ],
    titles: [(id: "hero", name: "Hero"), (id: "lord", name: "Lord")],
    actors: [
        (id: "hero", name: "Hero", role: "warrior", faction: "kingdom", power: 40, home: "village", titles: ["hero"]),
        (id: "borin", name: "Borin", role: "villager", faction: "kingdom", power: 10, home: "village"),
        (id: "oswin", name: "Oswin", role: "priest", faction: "church", power: 10, home: "village"),
        (id: "lord", name: "Lord", role: "lord", faction: "demons", power: 500, home: "keep", titles: ["lord"], boss: true),
    ],
    hero_party: (leader_title: "hero", roles: ["warrior", "priest"], members: ["hero"], goal: "lord"),
)"#;

const STORY: &str = r#"(
    storylets: [
        (id: "borin_chat", with: "borin", repeat: Daily, start: "hello", nodes: {
            "hello": (line: "borin.hello", then: [Affinity(50)], choices: [
                (says: "me.travel", when: [NotFollowing, WillFollow], then: [Follow], next: "glad"),
                (says: "me.bye"),
            ]),
            "glad": (line: "borin.glad"),
        }),
        (id: "borin_propose", with: "borin", priority: 10, repeat: Once,
            when: [Affinity(300), Unmarried], start: "ask", nodes: {
            "ask": (line: "borin.shy", choices: [
                (says: "me.marry", when: [PersonUnmarried], then: [Marry, FadeToBlack, SetFlag("wed")]),
                (says: "me.not_yet"),
            ]),
        }),
        (id: "oswin_hero", with: "oswin", when: [Standing(faction: "church", at_least: 50), NotInHeroParty],
            start: "call", nodes: {
            "call": (line: "oswin.call", choices: [
                (says: "me.yes", then: [JoinHeroParty, Standing(faction: "church", by: 100)]),
                (says: "me.dark", then: [Defect("demons")]),
            ]),
        }),
    ],
    endings: [
        (id: "dark", text: "end.dark", when: [Faction("demons")]),
        (id: "wed", text: "end.wed", when: [Married, GoalSurvived]),
        (id: "none", text: "end.none"),
    ],
)"#;

fn setup() -> (StoryDef, WorldSim, Story, ActorId) {
    let def = StoryDef::parse(STORY).unwrap();
    def.validate().unwrap();
    let mut sim = WorldSim::new(&WorldDef::parse(WORLD).unwrap(), 1).unwrap();
    let village = sim.world().region("village").unwrap();
    let me = sim.player_actor(7, village);
    (def, sim, Story::default(), me)
}

fn id(sim: &WorldSim, name: &str) -> ActorId {
    sim.world().actor(name).unwrap()
}

#[test]
fn a_person_tells_their_storylet_and_choices_change_the_world() {
    let (def, mut sim, mut story, me) = setup();
    let borin = id(&sim, "borin");
    let talk = story
        .open(&def, &sim, me, borin)
        .expect("Borin has something to say");
    assert!(story.begin(&def, &mut sim, &talk).is_empty());
    // Walking away before answering loses nothing: it can be told again today.
    assert!(story.open(&def, &sim, me, borin).is_some());
    assert_eq!(story.affinity(borin, me), 50, "the greeting warms him");
    let view = story.view(&def, &sim, &talk);
    assert_eq!(view.line, "borin.hello");
    assert_eq!(
        view.choices,
        vec![(0, "me.travel".into()), (1, "me.bye".into())]
    );
    let (next, _) = story.choose(&def, &mut sim, &talk, 0);
    let next = next.expect("he answers");
    assert_eq!(story.view(&def, &sim, &next).line, "borin.glad");
    assert!(sim.party_of(borin).is_some(), "he follows her now");
    // Told daily: not again today, again tomorrow (and the travel choice is gone: he follows).
    assert!(story.open(&def, &sim, me, borin).is_none());
    sim.advance_hours(24);
    let again = story.open(&def, &sim, me, borin).unwrap();
    assert_eq!(
        story.view(&def, &sim, &again).choices,
        vec![(1, "me.bye".into())]
    );
}

#[test]
fn warm_enough_he_proposes_once_and_marriage_shapes_the_ending() {
    let (def, mut sim, mut story, me) = setup();
    let borin = id(&sim, "borin");
    for _ in 0..6 {
        let talk = story.open(&def, &sim, me, borin).unwrap();
        story.begin(&def, &mut sim, &talk);
        // Heard out ("see you"), so told for the day.
        story.choose(&def, &mut sim, &talk, 1);
        sim.advance_hours(24);
    }
    assert_eq!(story.affinity(borin, me), 300);
    let talk = story.open(&def, &sim, me, borin).unwrap();
    assert_eq!(
        def.storylets[talk.storylet].id, "borin_propose",
        "the higher priority wins"
    );
    story.begin(&def, &mut sim, &talk);
    let (end, deeds) = story.choose(&def, &mut sim, &talk, 0);
    assert!(end.is_none());
    assert_eq!(deeds, vec![Deed::FadeToBlack]);
    assert_eq!(story.spouse(me), Some(borin));
    assert!(story.flag(me, "wed"));
    // Once only; afterwards his daily chat again.
    sim.advance_hours(24);
    let talk = story.open(&def, &sim, me, borin).unwrap();
    assert_eq!(def.storylets[talk.storylet].id, "borin_chat");
    // Married to her, he cannot marry another player.
    let village = sim.world().region("village").unwrap();
    let other = sim.player_actor(8, village);
    let asked = Conversation {
        storylet: def
            .storylets
            .iter()
            .position(|s| s.id == "borin_propose")
            .unwrap(),
        node: "ask".into(),
        player: other,
        person: borin,
    };
    assert!(
        story
            .view(&def, &sim, &asked)
            .choices
            .iter()
            .all(|(_, s)| s != "me.marry")
    );
    sim.run_year();
    let ending = story.ending(&def, &sim).unwrap();
    assert!(ending.id == "wed" || ending.id == "none", "{}", ending.id);
    if !matches!(sim.outcome(), Some(Outcome::GoalDefeated { .. })) {
        assert_eq!(ending.id, "wed");
    }
}

#[test]
fn the_church_calls_a_trusted_player_to_the_hero_party_or_they_turn_to_the_dark() {
    let (def, mut sim, mut story, me) = setup();
    let oswin = id(&sim, "oswin");
    assert!(
        story.open(&def, &sim, me, oswin).is_none(),
        "the church does not know her"
    );
    for _ in 0..10 {
        sim.slain_enemy(me);
    }
    let talk = story.open(&def, &sim, me, oswin).unwrap();
    // Joining the hero party.
    let mut joined = (story.clone(), sim.clone());
    let (_, deeds) = joined.0.choose(&def, &mut joined.1, &talk, 0);
    assert!(deeds.is_empty());
    assert!(joined.1.party().members.contains(&me));
    // Or going over to the demons: an enemy of her friends, and the dark ending.
    let (_, deeds) = story.choose(&def, &mut sim, &talk, 1);
    assert_eq!(deeds, vec![Deed::Defected("demons".into())]);
    let demons = sim.world().faction("demons").unwrap();
    assert_eq!(sim.world().get(me).faction, demons);
    assert_eq!(story.ending(&def, &sim).unwrap().id, "dark");
}

#[test]
fn a_story_that_leads_nowhere_is_refused() {
    let broken = r#"(storylets: [(id: "x", with: "borin", start: "a", nodes: {
        "a": (line: "l", choices: [(says: "s", next: "missing")]),
    })])"#;
    assert!(StoryDef::parse(broken).unwrap().validate().is_err());
}

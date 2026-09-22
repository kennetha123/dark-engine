use super::*;

fn rates() -> Rates {
    Rates::default()
}

/// A mild day (18 °C), awake, in the open, no clothes to speak of.
fn mild() -> Surroundings {
    Surroundings {
        air: 1800,
        ..Surroundings::default()
    }
}

fn hours(body: &mut Body, around: &Surroundings, hours: u32) -> Vec<Minute> {
    (0..hours * 60)
        .map(|_| body.minute(around, &rates()))
        .collect()
}

#[test]
fn needs_grow_by_their_hourly_rates_exactly() {
    let mut body = Body::new();
    hours(&mut body, &mild(), 5);
    let r = rates().awake;
    assert_eq!(body.need(Need::Hunger), 5 * r.hunger);
    assert_eq!(body.need(Need::Thirst), 5 * r.thirst);
    assert_eq!(body.need(Need::Fatigue), 5 * r.fatigue);
    assert_eq!(body.need(Need::Hygiene), 5 * r.hygiene);
    assert_eq!(body.level(Need::Thirst), Level::Fine);
    hours(&mut body, &mild(), 4);
    assert_eq!(body.level(Need::Thirst), Level::Wanting, "540 ‰ after 9 h");
}

#[test]
fn sleep_rests_best_at_an_inn_and_worst_in_the_open() {
    let rested = |shelter| {
        let mut body = Body::new();
        body.set_need(Need::Fatigue, 900);
        let around = Surroundings {
            shelter,
            asleep: true,
            ..mild()
        };
        hours(&mut body, &around, 4);
        body.need(Need::Fatigue)
    };
    let (open, tent, inn) = (
        rested(Shelter::Open),
        rested(Shelter::Tent),
        rested(Shelter::Inn),
    );
    assert!(inn < tent && tent < open, "{inn} < {tent} < {open}");
    assert_eq!(open, 900 - 4 * rates().rest_open);
}

#[test]
fn a_bladder_left_too_long_is_an_accident() {
    let mut body = Body::new();
    body.set_need(Need::Bladder, 990);
    let minutes = hours(&mut body, &mild(), 1);
    let accidents: Vec<_> = minutes.iter().flat_map(|m| m.accidents.clone()).collect();
    assert_eq!(accidents, vec![Need::Bladder]);
    assert!(body.soiled);
    assert_eq!(body.level(Need::Hygiene), Level::Failed);
    assert!(body.condition(&rates()).statuses.contains(&Status::Soiled));
    body.wash();
    assert!(!body.soiled);
    body.set_need(Need::Bowel, 700);
    body.relieve();
    assert_eq!(body.need(Need::Bowel), 0);
}

#[test]
fn drink_makes_merry_then_clumsy_then_unconscious_and_wears_off() {
    let ale = Consumable {
        thirst: -150,
        bladder: 200,
        alcohol: 1000,
        ..Consumable::default()
    };
    let mut body = Body::new();
    body.eat_or_drink(&ale);
    assert_eq!(body.drunkenness(&rates()), Drunk::Tipsy);
    assert_eq!(body.condition(&rates()).wobble, 1);
    body.eat_or_drink(&ale);
    body.eat_or_drink(&ale);
    assert_eq!(body.drunkenness(&rates()), Drunk::Drunk);
    assert!(body.condition(&rates()).speed < 90);
    body.eat_or_drink(&ale);
    assert_eq!(body.drunkenness(&rates()), Drunk::Wasted, "four drinks");
    body.eat_or_drink(&ale);
    let first = body.minute(&mild(), &rates());
    assert!(first.passed_out, "five drinks: out cold");
    assert!(body.condition(&rates()).passed_out);
    // Asleep it off: comes to once the drink has mostly gone.
    let mut came_to = false;
    for _ in 0..12 * 60 {
        came_to |= body.minute(&mild(), &rates()).came_to;
    }
    assert!(came_to);
    assert!(!body.passed_out);
}

#[test]
fn exhaustion_knocks_you_out() {
    let mut body = Body::new();
    body.set_need(Need::Fatigue, 999);
    assert!(hours(&mut body, &mild(), 1).iter().any(|m| m.passed_out));
    assert!(body.passed_out);
}

#[test]
fn cold_without_clothes_is_dangerous_and_warmth_saves_you() {
    let frost = Surroundings {
        air: -1000,
        ..Surroundings::default()
    };
    let core_after = |around: Surroundings, h: u32| {
        let mut body = Body::new();
        hours(&mut body, &around, h);
        body
    };
    let naked = core_after(frost, 3);
    assert_eq!(naked.temperature(&rates()), Temperature::Hypothermic);
    let cloaked = core_after(
        Surroundings {
            insulation: 1500,
            ..frost
        },
        3,
    );
    assert!(cloaked.core > naked.core, "a cloak slows the cold");
    let by_the_fire = core_after(
        Surroundings {
            insulation: 1500,
            fire: true,
            ..frost
        },
        3,
    );
    assert_eq!(by_the_fire.temperature(&rates()), Temperature::Comfortable);
    let indoors = core_after(
        Surroundings {
            shelter: Shelter::Inn,
            ..frost
        },
        3,
    );
    assert_eq!(indoors.temperature(&rates()), Temperature::Comfortable);
    // Heating magic, for as long as it lasts.
    let mut warmed = Body::new();
    warmed.warm(3500, 120);
    hours(&mut warmed, &frost, 2);
    assert_eq!(warmed.temperature(&rates()), Temperature::Comfortable);
    assert_eq!(warmed.warmth_minutes, 0);
}

#[test]
fn freezing_and_starving_hurt_by_the_hour() {
    let mut body = Body::new();
    body.set_need(Need::Hunger, 1000);
    let lost: u32 = hours(&mut body, &mild(), 2)
        .iter()
        .map(|m| u32::from(m.damage))
        .sum();
    assert_eq!(lost, 2 * rates().harm.starving);
    assert!(
        body.condition(&rates())
            .statuses
            .contains(&Status::Starving)
    );
}

#[test]
fn a_rested_fed_warm_body_is_at_full_speed() {
    let body = Body::new();
    let c = body.condition(&rates());
    assert_eq!((c.speed, c.wobble, c.passed_out), (100, 0, false));
    assert!(c.statuses.is_empty());
}

#[test]
fn items_are_eaten_worn_and_set_down() {
    let def = LifeDef::parse(
        r#"(
            items: {
                "bread": (name: "b", use: Consume((hunger: -400, bowel: 80))),
                "cloak": (name: "c", use: Wear(insulation: 1500)),
                "tent": (name: "t", use: Place(Tent)),
                "charm": (name: "h", use: Warm(bonus: 3000, minutes: 60)),
            },
            start: [("bread", 1), ("cloak", 1), ("tent", 1), ("charm", 1)],
        )"#,
    )
    .unwrap();
    def.validate().unwrap();
    let mut body = Body::new();
    body.set_need(Need::Hunger, 600);
    let mut inv = Inventory::with(&def.start);
    assert_eq!(
        use_item(&mut body, &mut inv, &def.items, 0),
        Used::Consumed("bread".into())
    );
    assert_eq!(body.need(Need::Hunger), 200);
    assert_eq!(inv.count("bread"), 0);
    assert_eq!(
        use_item(&mut body, &mut inv, &def.items, 0),
        Used::Nothing,
        "none left"
    );
    // The cloak goes on and off, and stays in the pack.
    assert_eq!(
        use_item(&mut body, &mut inv, &def.items, 1),
        Used::PutOn("cloak".into())
    );
    assert_eq!(inv.insulation(&def.items), 1500);
    assert_eq!(
        use_item(&mut body, &mut inv, &def.items, 1),
        Used::TookOff("cloak".into())
    );
    assert_eq!(inv.insulation(&def.items), 0);
    assert_eq!(inv.count("cloak"), 1);
    assert_eq!(
        use_item(&mut body, &mut inv, &def.items, 2),
        Used::Place("tent".into(), Structure::Tent)
    );
    assert_eq!(
        use_item(&mut body, &mut inv, &def.items, 3),
        Used::Warmed("charm".into())
    );
    assert_eq!(body.warmth_minutes, 60);
    assert_eq!(use_item(&mut body, &mut inv, &def.items, 9), Used::Nothing);
    // Unknown start items are refused.
    let bad = LifeDef::parse(r#"(start: [("nothing", 1)])"#).unwrap();
    assert!(bad.validate().is_err());
}

#[test]
fn the_air_is_coldest_before_dawn_and_warmest_in_the_afternoon() {
    let c = Climate {
        day: 2200,
        night: 400,
    };
    assert_eq!(c.air(5 * 60), 400);
    assert_eq!(c.air(17 * 60), 2200);
    assert_eq!(c.air(11 * 60), 1300, "halfway up");
    assert_eq!(c.air(23 * 60), 1300, "halfway down");
    assert_eq!(c.air(0), c.air(24 * 60));
    let def = LifeDef::parse(r#"(climates: {"peak": (day: -200, night: -1500)})"#).unwrap();
    assert_eq!(def.air(Some("peak"), 5 * 60), -1500);
    assert_eq!(def.air(Some("elsewhere"), 17 * 60), Climate::default().day);
    assert_eq!(def.air(None, 17 * 60), Climate::default().day);
}

#[test]
fn a_new_character_wears_what_the_project_says() {
    let def = LifeDef::parse(
        r#"(
            items: {
                "cloak": (name: "c", use: Wear(insulation: 1500)),
                "soap": (name: "s", use: Wash),
            },
            start: [("cloak", 1), ("soap", 1)],
            wear: "cloak",
        )"#,
    )
    .unwrap();
    def.validate().unwrap();
    let mut inv = def.new_inventory();
    assert_eq!(inv.insulation(&def.items), 1500);
    let mut body = Body::new();
    body.set_need(Need::Hygiene, 900);
    body.soiled = true;
    assert_eq!(
        use_item(&mut body, &mut inv, &def.items, 1),
        Used::Washed("soap".into())
    );
    assert!(!body.soiled);
    assert_eq!(body.need(Need::Hygiene), 0);
    let bad = LifeDef::parse(
        r#"(items: {"soap": (name: "s", use: Wash)}, start: [("soap", 1)], wear: "soap")"#,
    )
    .unwrap();
    assert!(bad.validate().is_err(), "soap is not clothing");
}

#[test]
fn a_little_cold_adds_up_instead_of_rounding_away() {
    // Two degrees under the comfortable range: well under one hundredth a minute.
    let chilly = Surroundings {
        air: rates().comfort.0 - 200,
        ..Surroundings::default()
    };
    let mut body = Body::new();
    hours(&mut body, &chilly, 6);
    assert!(body.core < WARM_CORE - 100, "{}", body.core);
    assert_eq!(body.temperature(&rates()), Temperature::Cold);
    // Back in comfort it warms up again.
    hours(&mut body, &mild(), 6);
    assert_eq!(body.temperature(&rates()), Temperature::Comfortable);
}

use super::*;

/// Two attacks (5+3+10 ticks, chaining from 4 ticks into recovery) and an 18-tick dodge.
fn moveset() -> Moveset {
    let attack = |clip: &str| AttackDef {
        clip: clip.into(),
        startup: 5,
        active: 3,
        recovery: 10,
        chain_from: 4,
        damage: 10,
        poise_damage: 10,
        knockback: 100.0,
        hitstun: 12,
        reach: 16.0,
        radius: 10.0,
        lunge: 30.0,
        hitstop: 4,
    };
    Moveset {
        health: 30,
        poise: 15,
        poise_recovery: 60,
        combo: vec![attack("a1"), attack("a2")],
        dodge: Some(DodgeDef {
            clip: "dodge".into(),
            ticks: 18,
            moving: 12,
            speed: 200.0,
            invulnerable: (1, 10),
        }),
        ..Moveset::default()
    }
}

const RIGHT: Vec2 = Vec2::X;

fn idle() -> Wants {
    Wants::default()
}

fn attack() -> Wants {
    Wants {
        attack: true,
        ..Wants::default()
    }
}

fn dodge() -> Wants {
    Wants {
        dodge: true,
        ..Wants::default()
    }
}

/// Steps with `wants` first, then idle, until the fighter is free; returns what started.
fn run(f: &mut Fighter, m: &Moveset, first: Wants) -> Vec<Started> {
    let mut started = Vec::new();
    let mut wants = first;
    for _ in 0..200 {
        started.extend(f.step(m, wants, RIGHT, true).started);
        wants = idle();
        if f.is_free() {
            break;
        }
    }
    started
}

#[test]
fn an_attack_winds_up_strikes_and_recovers() {
    let m = moveset();
    let mut f = Fighter::new(&m);
    let t = f.step(&m, attack(), RIGHT, true);
    assert_eq!(t.started, Some(Started::Attack(0)));
    assert_eq!(t.velocity, Some(RIGHT * 30.0), "lunges while winding up");
    let mut active = Vec::new();
    for tick in 1..18 {
        f.step(&m, idle(), RIGHT, true);
        if f.hitbox(&m, Vec2::ZERO, RIGHT).is_some() {
            active.push(tick);
        }
    }
    assert_eq!(active, vec![5, 6, 7], "the hitbox is out only while active");
    assert_eq!(f.hitbox(&m, Vec2::ZERO, RIGHT), None, "not during recovery");
    f.step(&m, idle(), RIGHT, true);
    assert!(f.is_free(), "18 ticks in all");
}

#[test]
fn pressing_again_chains_the_combo_and_it_wraps() {
    let m = moveset();
    let mut f = Fighter::new(&m);
    let mut started = Vec::new();
    // Hold attack every tick: each attack chains into the next from its chain point.
    for _ in 0..60 {
        started.extend(f.step(&m, attack(), RIGHT, true).started);
    }
    assert_eq!(
        &started[..4],
        &[
            Started::Attack(0),
            Started::Attack(1),
            Started::Attack(0),
            Started::Attack(1)
        ]
    );
}

#[test]
fn a_press_a_little_early_is_buffered() {
    let m = moveset();
    let mut f = Fighter::new(&m);
    f.step(&m, attack(), RIGHT, true);
    // Press during active frames (tick 6), well before recovery allows chaining (tick 12).
    for _ in 1..6 {
        f.step(&m, idle(), RIGHT, true);
    }
    f.step(&m, attack(), RIGHT, true);
    let mut started = Vec::new();
    for _ in 0..10 {
        started.extend(f.step(&m, idle(), RIGHT, true).started);
    }
    assert_eq!(started, vec![Started::Attack(1)]);
}

#[test]
fn the_combo_starts_over_once_its_window_closes() {
    let m = moveset();
    let mut f = Fighter::new(&m);
    assert_eq!(run(&mut f, &m, attack()), vec![Started::Attack(0)]);
    // Straight after: the combo continues.
    assert_eq!(run(&mut f, &m, attack()), vec![Started::Attack(1)]);
    for _ in 0..COMBO_WINDOW + 1 {
        f.step(&m, idle(), RIGHT, true);
    }
    assert_eq!(run(&mut f, &m, attack()), vec![Started::Attack(0)]);
}

#[test]
fn a_dodge_cuts_recovery_short_and_cannot_be_hit_while_it_lasts() {
    let m = moveset();
    let mut f = Fighter::new(&m);
    f.step(&m, attack(), RIGHT, true);
    let mut started = None;
    for _ in 0..20 {
        if let Some(s) = f.step(&m, dodge(), RIGHT, true).started {
            started = Some(s);
            break;
        }
    }
    assert_eq!(started, Some(Started::Dodge));
    // Tick 0 of the dodge is not yet invulnerable; ticks 1..=10 are.
    assert!(!f.invulnerable(&m));
    f.step(&m, idle(), RIGHT, true);
    assert!(f.invulnerable(&m));
    let before = f.health;
    assert_eq!(f.take_hit(&m, &m.combo[0], RIGHT, 7), HitOutcome::Missed);
    assert_eq!(f.health, before);
}

#[test]
fn a_dodge_goes_where_the_stick_points_else_where_the_fighter_faces() {
    let m = moveset();
    let mut f = Fighter::new(&m);
    let up = Wants {
        movement: Vec2::NEG_Y,
        dodge: true,
        ..Wants::default()
    };
    assert_eq!(
        f.step(&m, up, RIGHT, true).velocity,
        Some(Vec2::NEG_Y * 200.0)
    );
    let mut g = Fighter::new(&m);
    assert_eq!(
        g.step(&m, dodge(), RIGHT, true).velocity,
        Some(RIGHT * 200.0)
    );
    // No dodging or attacking in the air.
    let mut h = Fighter::new(&m);
    assert_eq!(h.step(&m, dodge(), RIGHT, false).started, None);
}

#[test]
fn poise_decides_whether_a_hit_staggers() {
    let m = moveset();
    let mut f = Fighter::new(&m);
    // 15 poise: the first 10-poise hit only hurts, the second staggers.
    assert_eq!(f.take_hit(&m, &m.combo[0], RIGHT, 1), HitOutcome::Damaged);
    assert!(f.is_free());
    assert_eq!(f.take_hit(&m, &m.combo[0], RIGHT, 1), HitOutcome::Staggered);
    // Pushed back, fading, until the stagger ends.
    let first = f.step(&m, attack(), RIGHT, true).velocity.unwrap();
    assert!(first.x > 0.0 && first.x < 100.0);
    assert!(
        run(&mut f, &m, idle()).is_empty(),
        "cannot act while staggered"
    );
    assert!(f.is_free());
    assert_eq!(f.hurts, 2);
    assert_eq!(f.hit_by, Some(1));
}

#[test]
fn poise_comes_back_after_a_while_unhurt() {
    let m = moveset();
    let mut f = Fighter::new(&m);
    f.take_hit(&m, &m.combo[0], RIGHT, 1);
    assert_eq!(f.poise, 5);
    for _ in 0..m.poise_recovery {
        f.step(&m, idle(), RIGHT, true);
    }
    assert_eq!(f.poise, 15);
}

#[test]
fn the_last_hit_kills_and_the_dead_stay_down() {
    let m = moveset();
    let mut f = Fighter::new(&m);
    f.take_hit(&m, &m.combo[0], RIGHT, 1);
    f.take_hit(&m, &m.combo[0], RIGHT, 1);
    assert_eq!(f.take_hit(&m, &m.combo[0], RIGHT, 1), HitOutcome::Killed);
    assert!(f.is_dead());
    assert_eq!(f.take_hit(&m, &m.combo[0], RIGHT, 1), HitOutcome::Missed);
    assert_eq!(f.step(&m, attack(), RIGHT, true).started, None);
    f.revive(&m);
    assert!(f.is_free());
    assert_eq!(f.health, 30);
}

#[test]
fn a_body_without_a_combo_or_dodge_just_moves() {
    let m = Moveset::default();
    let mut f = Fighter::new(&m);
    let t = f.step(
        &m,
        Wants {
            attack: true,
            dodge: true,
            movement: RIGHT,
        },
        RIGHT,
        true,
    );
    assert_eq!(
        t,
        Tick {
            velocity: None,
            started: None
        }
    );
}

#[test]
fn definitions_are_checked() {
    let good = r#"(
        movesets: { "a": (health: 10, combo: [(startup: 1, active: 1, recovery: 4, chain_from: 2, damage: 1, reach: 8, radius: 4)]) },
        enemies: { "e": (name: "n", sheet: "s", moveset: "a", ai: (sight: 100, leash: 200, attack_range: 20)) },
    )"#;
    CombatDef::parse(good).unwrap().validate().unwrap();
    for (from, to) in [
        ("chain_from: 2", "chain_from: 9"),
        ("active: 1", "active: 0"),
        (r#"moveset: "a""#, r#"moveset: "b""#),
        ("health: 10", "health: 0"),
        ("sight: 100", "sight: 0"),
    ] {
        let text = good.replacen(from, to, 1);
        assert!(
            CombatDef::parse(&text).unwrap().validate().is_err(),
            "accepted {to}"
        );
    }
}

#[test]
fn suffering_costs_health_without_a_hit_and_can_kill() {
    let m = moveset();
    let mut f = Fighter::new(&m);
    f.step(&m, attack(), Vec2::X, true);
    assert!(!f.suffer(1));
    assert_eq!(f.health, m.health - 1);
    assert!(matches!(f.action, Action::Attack { .. }), "no stagger");
    assert_eq!(f.hurts, 0, "not a hit: nothing flashes");
    assert!(f.suffer(u16::MAX));
    assert!(f.is_dead());
    assert!(!f.suffer(5), "the dead suffer no more");
}

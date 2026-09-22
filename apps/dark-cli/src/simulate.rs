//! `dark-cli simulate`: fast-forwards the world simulation through a whole year, headless.
//! One seed prints the year's chronicle; many seeds print balance statistics.

use dark_assets::{Localization, Project};
use dark_sim::{Happening, Outcome, WorldDef, WorldSim};

pub struct Options {
    pub seed: u64,
    pub runs: u64,
    pub lang: Option<String>,
}

pub fn parse(args: &[String]) -> Result<(String, Options), String> {
    let (project, rest) = args
        .split_first()
        .ok_or("simulate needs a project directory")?;
    let mut options = Options {
        seed: 1,
        runs: 1,
        lang: None,
    };
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        let number = || {
            value
                .parse::<u64>()
                .map_err(|_| format!("{flag}: {value:?} is not a number"))
        };
        match flag.as_str() {
            "--seed" => options.seed = number()?,
            "--runs" => options.runs = number()?.max(1),
            "--lang" => options.lang = Some(value.clone()),
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    Ok((project.clone(), options))
}

pub fn run(project: &str, options: &Options) -> Result<(), Box<dyn std::error::Error>> {
    let project = Project::open(project)?;
    let def = WorldDef::load(&project.path("world.ron"))?;
    let mut strings = Localization::load(&project)?;
    if let Some(lang) = &options.lang
        && !strings.set_language(lang)
    {
        return Err(format!("the project has no language {lang:?}").into());
    }
    let text = |key: &str| strings.text(key).to_owned();
    let story = dark_story::StoryDef::load_or_default(&project.path("story.ron"))?;
    // Nobody played: the ending as the world alone leaves it.
    let ending = |sim: &WorldSim| {
        dark_story::Story::default()
            .ending(&story, sim)
            .map(|e| e.id.clone())
    };

    if options.runs == 1 {
        let started = std::time::Instant::now();
        let mut sim = WorldSim::new(&def, options.seed)?;
        sim.run_year();
        for event in sim.take_events() {
            println!("{}", sim.describe(&event, &text));
        }
        if let Some(id) = ending(&sim) {
            println!("ending: {id}");
        }
        eprintln!(
            "(seed {}, simulated in {:.0?})",
            options.seed,
            started.elapsed()
        );
        return Ok(());
    }

    let started = std::time::Instant::now();
    let mut stats = Stats::default();
    for seed in options.seed..options.seed + options.runs {
        let mut sim = WorldSim::new(&def, seed)?;
        sim.run_year();
        let events = sim.take_events();
        stats.add(&sim, &events);
        if let Some(id) = ending(&sim) {
            *stats.endings.entry(id).or_default() += 1;
        }
    }
    stats.print(options.runs, started.elapsed());
    Ok(())
}

#[derive(Default)]
struct Stats {
    won_days: Vec<u32>,
    deaths: u64,
    title_changes: u64,
    lieutenants: u64,
    party_fell: u64,
    encounters: u64,
    retreats: u64,
    endings: std::collections::BTreeMap<String, u64>,
}

impl Stats {
    fn add(&mut self, sim: &WorldSim, events: &[dark_sim::WorldEvent]) {
        if let Some(Outcome::GoalDefeated { day }) = sim.outcome() {
            self.won_days.push(day + 1);
        }
        let party = sim.party();
        for e in events {
            match &e.what {
                Happening::Died { actor, .. } if !sim.world().get(*actor).boss => self.deaths += 1,
                Happening::TitlePassed { .. } | Happening::TitleVacant { .. } => {
                    self.title_changes += 1
                }
                Happening::Battle {
                    boss, won: true, ..
                } if party.lieutenants.contains(boss) => self.lieutenants += 1,
                Happening::Battle { won: false, .. } => self.retreats += 1,
                Happening::PartyFell => self.party_fell += 1,
                Happening::Encounter { .. } => self.encounters += 1,
                _ => {}
            }
        }
    }

    fn print(&self, runs: u64, took: std::time::Duration) {
        let per = |n: u64| n as f64 / runs as f64;
        let won = self.won_days.len() as u64;
        println!("{runs} years simulated in {took:.1?}");
        println!(
            "goal defeated:        {won}/{runs} ({:.0}%)",
            100.0 * won as f64 / runs as f64
        );
        if let (Some(first), Some(last)) = (self.won_days.iter().min(), self.won_days.iter().max())
        {
            let mean = self.won_days.iter().map(|&d| u64::from(d)).sum::<u64>() / won;
            println!("  on day:             mean {mean}, earliest {first}, latest {last}");
        }
        println!("party deaths / year:  {:.2}", per(self.deaths));
        println!("title changes / year: {:.2}", per(self.title_changes));
        println!("lieutenants slain:    {:.2} / year", per(self.lieutenants));
        println!("boss fights lost:     {:.2} / year", per(self.retreats));
        println!("monster encounters:   {:.1} / year", per(self.encounters));
        println!("party wiped out:      {}/{runs}", self.party_fell);
        for (id, n) in &self.endings {
            println!("ending {id:<14} {n}/{runs}");
        }
    }
}

//! Maps the host simulates at once, and bodies moving within and between them.

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, SystemSet};
use dark_assets::{AssetError, PlacedProp, Project, SceneDef};
use dark_core::{App, FixedDelta, FixedUpdate, Plugin};
use dark_physics::{Body, Cell, Collider, MoveParams, Shape, World};
use glam::Vec2;

/// Scattered props only go where the ground is flat within this radius.
const FLAT_RADIUS: f32 = 10.0;

/// Index into [`Maps::maps`]; the start map is 0.
#[derive(
    Component, Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct MapId(pub u16);

#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct BodyState(pub Body);

/// Where the body was at the start of the tick, for render interpolation.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct PreviousBody {
    pub position: Vec2,
    pub elevation: f32,
}

/// What the controller (player input or AI) wants this tick. `jump` is consumed.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct MoveIntent {
    pub velocity: Vec2,
    pub jump: bool,
    /// Skip this body's physics this tick entirely (see [`crate::ControlInput::hold`]).
    pub hold: bool,
}

/// Never taken through an exit: characters that belong to one map (enemies at their posts).
#[derive(Component, Clone, Copy, Debug)]
pub struct StaysInMap;

/// Movement runs in this set; controllers set intents before it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Physics;

pub struct Exit {
    pub min: Vec2,
    pub max: Vec2,
    pub to: MapId,
    pub spawn: Vec2,
}

impl Exit {
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x < self.max.x && p.y >= self.min.y && p.y < self.max.y
    }
}

pub struct Map {
    /// Scene path inside the project.
    pub name: String,
    pub def: SceneDef,
    /// Props exactly as placed; the view draws these same props.
    pub props: Vec<PlacedProp>,
    pub collision: World,
    pub exits: Vec<Exit>,
}

#[derive(Resource)]
pub struct Maps {
    pub maps: Vec<Map>,
    pub params: MoveParams,
}

/// Bodies are checked against spawn points with this radius at load time.
const SPAWN_CHECK_RADIUS: f32 = 6.0;

impl Maps {
    /// Loads `start` and every scene reachable from it through exits. Scene paths are
    /// normalised (`./a\b.ron` is `a/b.ron`), so each scene is one map however it is written.
    pub fn load(project: &Project, start: &str) -> Result<Self, AssetError> {
        let tile = project.settings.tile_size;
        let start = normalise(start);
        let mut ids: HashMap<String, MapId> = HashMap::from([(start.clone(), MapId(0))]);
        let mut queue = vec![start];
        let mut loaded: Vec<(String, SceneDef)> = Vec::new();
        while let Some(name) = queue.pop() {
            let mut def = project.load_scene(&name)?;
            for exit in &mut def.exits {
                exit.to = normalise(&exit.to);
                if !ids.contains_key(&exit.to) {
                    let id = u16::try_from(ids.len())
                        .map_err(|_| invalid(project, &name, "more than 65535 maps".into()))?;
                    ids.insert(exit.to.clone(), MapId(id));
                    queue.push(exit.to.clone());
                }
            }
            loaded.push((name, def));
        }
        loaded.sort_by_key(|(name, _)| ids[name].0);

        // Where other scenes' exits land in each scene, and where its NPCs, enemies and inn beds
        // are: scattered props must stay clear of them.
        let mut arrivals: Vec<Vec<(f32, f32)>> = loaded
            .iter()
            .map(|(_, def)| {
                def.npcs
                    .iter()
                    .map(|n| n.position)
                    .chain(def.enemies.iter().map(|e| e.position))
                    .chain(def.inns.iter().map(|i| i.bed))
                    .collect()
            })
            .collect();
        for (_, def) in &loaded {
            for exit in &def.exits {
                arrivals[ids[&exit.to].0 as usize].push(exit.spawn);
            }
        }

        let mut maps = Vec::with_capacity(loaded.len());
        for ((name, def), arrivals) in loaded.into_iter().zip(&arrivals) {
            let map = build_map(name, def, tile, &ids, arrivals)
                .map_err(|(name, message)| invalid(project, &name, message))?;
            maps.push(map);
        }
        validate_spawns(&maps).map_err(|(name, message)| invalid(project, &name, message))?;
        Ok(Self {
            maps,
            params: MoveParams::default(),
        })
    }

    pub fn get(&self, id: MapId) -> &Map {
        &self.maps[id.0 as usize]
    }
}

/// A scene path as maps know it: `./a\b.ron` is `a/b.ron`.
fn normalise(path: &str) -> String {
    let path = path.replace('\\', "/");
    path.trim_start_matches("./").to_owned()
}

fn invalid(project: &Project, scene: &str, message: String) -> AssetError {
    AssetError::Invalid {
        path: project.path(scene),
        message,
    }
}

/// Every place a body can appear must be clear ground, and an arrival must not sit inside the
/// destination's own exit (the body would bounce between maps every tick).
fn validate_spawns(maps: &[Map]) -> Result<(), (String, String)> {
    let fits = |map: &Map, at: Vec2| {
        let world = &map.collision;
        world.ground_under(at, SPAWN_CHECK_RADIUS).is_finite()
            && !world
                .colliders
                .iter()
                .any(|c| c.height > 0.0 && overlaps(c, at, SPAWN_CHECK_RADIUS))
    };
    for map in maps {
        if let Some(player) = &map.def.player {
            let at = Vec2::from(player.spawn);
            if !fits(map, at) {
                return Err((
                    map.name.clone(),
                    format!("player spawn {at} is inside a wall or prop"),
                ));
            }
        }
        // Where a villager's day sends them must be somewhere they can stand and reach: level
        // with where they were placed, since nobody walks up a ledge or into another map (§21).
        for npc in &map.def.npcs {
            let post = Vec2::from(npc.position);
            let stands_at = map.collision.ground_under(post, SPAWN_CHECK_RADIUS);
            for entry in &npc.day {
                let at = Vec2::from(entry.at);
                if !(0.0..24.0).contains(&entry.from) {
                    return Err((
                        map.name.clone(),
                        format!("a villager's day has an hour of {}", entry.from),
                    ));
                }
                let level = (map.collision.ground_under(at, SPAWN_CHECK_RADIUS) - stands_at).abs();
                if !fits(map, at) || level > 0.5 {
                    return Err((
                        map.name.clone(),
                        format!("a villager's day sends them to {at}, which is no place to stand"),
                    ));
                }
                if map.exits.iter().any(|e| e.contains(at)) {
                    return Err((
                        map.name.clone(),
                        format!("a villager's day sends them into an exit at {at}"),
                    ));
                }
            }
        }
        let posts = map.def.npcs.iter().map(|n| n.position);
        for at in posts.chain(map.def.enemies.iter().map(|e| e.position)) {
            let at = Vec2::from(at);
            if !fits(map, at) {
                return Err((
                    map.name.clone(),
                    format!("an NPC or enemy at {at} is inside a wall or prop"),
                ));
            }
            if map.exits.iter().any(|e| e.contains(at)) {
                return Err((
                    map.name.clone(),
                    format!("an NPC or enemy at {at} stands in an exit"),
                ));
            }
        }
        for inn in &map.def.inns {
            let at = Vec2::from(inn.bed);
            let in_exit = map.exits.iter().any(|e| e.contains(at));
            if !fits(map, at) || !inn.contains(inn.bed) || in_exit {
                return Err((
                    map.name.clone(),
                    format!("an inn's bed at {at} must be clear ground in its area, not an exit"),
                ));
            }
        }
        for exit in &map.exits {
            let destination = &maps[exit.to.0 as usize];
            if !fits(destination, exit.spawn) {
                return Err((
                    map.name.clone(),
                    format!(
                        "exit spawn {} in {} is inside a wall or prop",
                        exit.spawn, destination.name
                    ),
                ));
            }
            if destination.exits.iter().any(|e| e.contains(exit.spawn)) {
                return Err((
                    map.name.clone(),
                    format!(
                        "exit spawn {} lands inside an exit of {}",
                        exit.spawn, destination.name
                    ),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn overlaps(collider: &Collider, at: Vec2, radius: f32) -> bool {
    match collider.shape {
        Shape::Circle { radius: r } => collider.center.distance(at) < r + radius,
        Shape::Rect { half } => {
            let closest = at.clamp(collider.center - half, collider.center + half);
            closest.distance(at) < radius
        }
    }
}

fn build_map(
    name: String,
    def: SceneDef,
    tile: u32,
    ids: &HashMap<String, MapId>,
    arrivals: &[(f32, f32)],
) -> Result<Map, (String, String)> {
    let terrain = def.build_terrain(tile).map_err(|m| (name.clone(), m))?;
    let mut collision = World::new(terrain);
    // Scatter only where the prop's whole footprint (collider, offset included) is on one flat
    // level that the group allows.
    let accept = |group: &dark_assets::ScatterDef, (x, y): (f32, f32)| {
        let pivot = Vec2::new(x, y);
        let (center, radius) = group.collider.map_or((pivot, 0.0), |c| {
            let r = match c.shape {
                Shape::Circle { radius } => radius,
                Shape::Rect { half } => half.length(),
            };
            (pivot + Vec2::from(c.offset), r)
        });
        let (lo, hi) = collision.ground_span(pivot, FLAT_RADIUS);
        let (flo, fhi) = collision.ground_span(center, radius.max(0.5));
        let flat = lo == hi && flo == fhi && lo == flo && hi.is_finite();
        let level_ok = group.levels.as_ref().is_none_or(|levels| {
            let (col, row) = collision.terrain.tile_of(pivot);
            collision
                .terrain
                .cell(col, row)
                .and_then(Cell::level)
                .is_some_and(|l| levels.contains(&l))
        });
        flat && level_ok
    };
    let props = def.placed_props(crate::SPAWN_CLEARING, arrivals, accept);
    for prop in &props {
        let at = Vec2::from(prop.position);
        let base = collision.ground_under(at, 0.5);
        for c in &prop.colliders {
            collision.colliders.push(Collider {
                center: at + Vec2::from(c.offset),
                shape: c.shape,
                base: if base.is_finite() { base } else { 0.0 },
                height: c.height,
            });
        }
    }
    let exits = def
        .exits
        .iter()
        .map(|e| {
            let (x, y, w, h) = e.area;
            Exit {
                min: Vec2::new(x, y),
                max: Vec2::new(x + w, y + h),
                to: ids[&e.to],
                spawn: Vec2::from(e.spawn),
            }
        })
        .collect();
    Ok(Map {
        name,
        def,
        props,
        collision,
        exits,
    })
}

impl Map {
    /// Scene `path` built on its own, as the game would build it, for an editor to show: exits
    /// lead nowhere (they all point at this map), and nothing is checked beyond what building
    /// needs. `landings` are where other scenes' exits arrive in this one: scattered props keep
    /// clear of them, as in the game.
    pub fn preview(
        path: &str,
        def: SceneDef,
        tile: u32,
        landings: &[(f32, f32)],
    ) -> Result<Map, String> {
        let ids: HashMap<String, MapId> =
            def.exits.iter().map(|e| (e.to.clone(), MapId(0))).collect();
        let own = normalise(path);
        let arrivals: Vec<(f32, f32)> = def
            .npcs
            .iter()
            .map(|n| n.position)
            .chain(def.enemies.iter().map(|e| e.position))
            .chain(def.inns.iter().map(|i| i.bed))
            .chain(landings.iter().copied())
            // An exit back into this same scene lands here too.
            .chain(
                def.exits
                    .iter()
                    .filter(|e| normalise(&e.to) == own)
                    .map(|e| e.spawn),
            )
            .collect();
        build_map(own, def, tile, &ids, &arrivals).map_err(|(_, m)| m)
    }

    /// Where exits of `scenes` (path and scene) arrive in scene `path`.
    pub fn landings<'a>(
        path: &str,
        scenes: impl IntoIterator<Item = (&'a str, &'a SceneDef)>,
    ) -> Vec<(f32, f32)> {
        let own = normalise(path);
        scenes
            .into_iter()
            .filter(|(from, _)| normalise(from) != own)
            .flat_map(|(_, def)| &def.exits)
            .filter(|e| normalise(&e.to) == own)
            .map(|e| e.spawn)
            .collect()
    }
}

/// Simulates bodies in all loaded maps.
pub struct MapsPlugin(pub Maps);

impl Plugin for MapsPlugin {
    fn build(self, app: &mut App) {
        app.insert_resource(self.0).add_systems(
            FixedUpdate,
            // Paused, bodies still take their positions as the previous ones, so nothing jitters.
            (
                remember_previous,
                // Room is made after exits are taken: a body standing by a door must not push
                // anyone out of it before they have gone through.
                (
                    step_bodies,
                    take_exits,
                    crate::crowd::make_room_for_each_other,
                )
                    .chain()
                    .run_if(crate::running),
            )
                .chain()
                .in_set(Physics),
        );
    }
}

/// Everything a moving character needs, at `spawn` in `map`.
pub fn body_bundle(map: MapId, spawn: Vec2, radius: f32) -> impl Bundle {
    (
        map,
        BodyState(Body::new(spawn, radius)),
        PreviousBody {
            position: spawn,
            elevation: 0.0,
        },
        MoveIntent::default(),
    )
}

fn remember_previous(mut bodies: Query<(&BodyState, &mut PreviousBody)>) {
    for (body, mut previous) in &mut bodies {
        *previous = PreviousBody {
            position: body.0.position,
            elevation: body.0.elevation,
        };
    }
}

fn step_bodies(
    maps: Res<Maps>,
    dt: Res<FixedDelta>,
    mut bodies: Query<(&MapId, &mut BodyState, &mut MoveIntent), Without<crate::Dormant>>,
) {
    let dt = dt.0.as_secs_f32();
    for (map, mut body, mut intent) in &mut bodies {
        if intent.hold {
            continue;
        }
        let world = &maps.get(*map).collision;
        world.step(&mut body.0, intent.velocity, intent.jump, dt, &maps.params);
        intent.jump = false;
    }
}

/// A body that can walk into an exit.
type Traveller = (
    &'static mut MapId,
    &'static mut BodyState,
    &'static mut PreviousBody,
);

fn take_exits(
    maps: Res<Maps>,
    mut bodies: Query<Traveller, (Without<StaysInMap>, Without<crate::Dormant>)>,
) {
    for (mut map, mut body, mut previous) in &mut bodies {
        let p = body.0.position;
        let Some(exit) = maps.get(*map).exits.iter().find(|e| e.contains(p)) else {
            continue;
        };
        let destination = &maps.get(exit.to).collision;
        let mut arrived = Body::new(exit.spawn, body.0.radius);
        arrived.elevation = destination
            .ground_under(exit.spawn, arrived.radius)
            .max(0.0);
        tracing::info!(
            "body moved from {} to {}",
            maps.get(*map).name,
            maps.get(exit.to).name
        );
        *map = exit.to;
        body.0 = arrived;
        // No interpolation across a map change.
        *previous = PreviousBody {
            position: arrived.position,
            elevation: arrived.elevation,
        };
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A throwaway project: `a.ron` with a level-1 block and an exit east into `b.ron`.
    pub(crate) fn project() -> Project {
        // One folder per call: tests run in parallel and must not read each other's half-written files.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("dark_world_maps_{}_{n}", std::process::id()));
        std::fs::create_dir_all(dir.join("scenes")).unwrap();
        std::fs::write(
            dir.join("project.ron"),
            r#"(name: "t", tile_size: 16, resolution: (320, 180))"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("scenes/a.ron"),
            r#"(
                size: (320, 160),
                ground: (sheet: "g", frame: 0),
                terrain: (fill: [(tiles: (8, 0, 2, 5), cell: Level(1))]),
                props: [(sheet: "p", frame: 0, position: (60, 40),
                         colliders: [(shape: Circle(radius: 6))])],
                exits: [(area: (300, 0, 20, 160), to: "scenes/b.ron", spawn: (40, 40))],
            )"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("scenes/b.ron"),
            r#"(size: (160, 160), ground: (sheet: "g", frame: 0),
                exits: [(area: (0, 150, 160, 10), to: "scenes/a.ron", spawn: (20, 20))])"#,
        )
        .unwrap();
        Project::open(dir).unwrap()
    }

    /// The standard test project with `scenes/b.ron` replaced.
    fn project_with_b(b: &str) -> Project {
        let project = project();
        std::fs::write(project.path("scenes/b.ron"), b).unwrap();
        project
    }

    #[test]
    fn a_preview_scatters_props_exactly_as_the_game_does() {
        // Dense scatter in b, which a's exit lands in: the landing must be kept clear in both.
        let project = project_with_b(
            r#"(size: (160, 160), ground: (sheet: "g", frame: 0),
                exits: [(area: (0, 150, 160, 10), to: "scenes/a.ron", spawn: (20, 20))],
                scatter: [(sheet: "p", frames: [0], count: 60, min_spacing: 12, seed: 5)])"#,
        );
        let maps = Maps::load(&project, "scenes/a.ron").unwrap();
        let game = &maps.get(MapId(1)).props;
        let (a, b) = (
            project.load_scene("scenes/a.ron").unwrap(),
            project.load_scene("scenes/b.ron").unwrap(),
        );
        let landings = Map::landings(
            "scenes/b.ron",
            [("scenes/a.ron", &a), ("./scenes/b.ron", &b)],
        );
        assert_eq!(landings, vec![(40.0, 40.0)], "a's exit, not b's own");
        let preview = Map::preview("scenes/b.ron", b.clone(), 16, &landings).unwrap();
        assert_eq!(&preview.props, game);
        let careless = Map::preview("scenes/b.ron", b, 16, &[]).unwrap();
        assert_ne!(&careless.props, game, "the landing does clear props");
    }

    #[test]
    fn scene_paths_are_normalised_so_each_scene_loads_once() {
        let maps = Maps::load(&project(), r".\scenes\a.ron").unwrap();
        assert_eq!(maps.maps.len(), 2, "b's exit back to a must reuse map 0");
        assert_eq!(maps.get(MapId(1)).exits[0].to, MapId(0));
    }

    #[test]
    fn exit_spawn_inside_a_wall_is_rejected() {
        let project = project_with_b(
            r#"(size: (160, 160), ground: (sheet: "g", frame: 0),
                terrain: (fill: [(tiles: (0, 0, 10, 10), cell: Wall)]))"#,
        );
        let err = Maps::load(&project, "scenes/a.ron")
            .err()
            .expect("spawn in a wall");
        assert!(err.to_string().contains("inside a wall or prop"), "{err}");
    }

    /// A day that sends a villager somewhere they cannot stand is caught as the map loads,
    /// not by a villager pressing into a cliff all afternoon.
    #[test]
    fn a_villagers_day_must_send_them_somewhere_they_can_stand() {
        let bad_day = |day: &str| {
            let project = project();
            let scene = std::fs::read_to_string(project.path("scenes/a.ron")).unwrap();
            let npcs = format!(
                r#"npcs: [(sheet: "n", position: (40, 40), lines: ["hm"], day: [{day}])],"#
            );
            std::fs::write(
                project.path("scenes/a.ron"),
                scene.replace("props:", &format!("{npcs}\n props:")),
            )
            .unwrap();
            Maps::load(&project, "scenes/a.ron")
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        };
        // The map's level-1 block starts at tile 8, its exit runs down the east edge.
        assert!(
            bad_day("(from: 8.0, at: (140.0, 40.0))").contains("no place to stand"),
            "a step up is no place to stand"
        );
        assert!(
            bad_day("(from: 8.0, at: (310.0, 40.0))").contains("into an exit"),
            "a doorway is no place to be sent"
        );
        assert!(
            bad_day("(from: 30.0, at: (60.0, 40.0))").contains("hour of 30"),
            "an hour outside the day"
        );
        // Clear of the map's prop (a 6 px circle at 60,40), its block and its exit.
        let fine = bad_day("(from: 8.0, at: (80.0, 100.0))");
        assert!(fine.is_empty(), "clear ground is fine, but: {fine}");
    }

    #[test]
    fn exit_spawn_inside_the_destination_exit_is_rejected() {
        let project = project_with_b(
            r#"(size: (160, 160), ground: (sheet: "g", frame: 0),
                exits: [(area: (0, 0, 80, 80), to: "scenes/a.ron", spawn: (20, 20))])"#,
        );
        let err = Maps::load(&project, "scenes/a.ron")
            .err()
            .expect("ping-pong exit");
        assert!(err.to_string().contains("lands inside an exit"), "{err}");
    }

    #[test]
    fn an_npc_standing_in_an_exit_is_rejected() {
        let project = project_with_b(
            r#"(size: (160, 160), ground: (sheet: "g", frame: 0),
                exits: [(area: (0, 0, 40, 40), to: "scenes/a.ron", spawn: (100, 100))],
                npcs: [(sheet: "n", position: (20, 20), lines: ["x"])])"#,
        );
        let err = Maps::load(&project, "scenes/a.ron")
            .err()
            .expect("NPC in an exit");
        assert!(err.to_string().contains("stands in an exit"), "{err}");
    }

    fn app(maps: Maps) -> App {
        let mut app = App::new(dark_core::DEFAULT_TICK_RATE);
        app.add_plugin(MapsPlugin(maps));
        app
    }

    #[test]
    fn loads_reachable_maps_with_terrain_and_prop_colliders() {
        let maps = Maps::load(&project(), "scenes/a.ron").unwrap();
        assert_eq!(maps.maps.len(), 2);
        assert_eq!(maps.maps[0].name, "scenes/a.ron");
        assert_eq!(maps.get(MapId(0)).exits[0].to, MapId(1));
        assert_eq!(maps.get(MapId(1)).exits[0].to, MapId(0));
        let a = &maps.get(MapId(0)).collision;
        assert_eq!(a.colliders.len(), 1);
        assert_eq!(a.ground_under(Vec2::new(136.0, 40.0), 1.0), 16.0);
    }

    #[test]
    fn bodies_in_different_maps_move_in_the_same_tick() {
        let mut app = app(Maps::load(&project(), "scenes/a.ron").unwrap());
        let a = app
            .world
            .spawn(body_bundle(MapId(0), Vec2::new(20.0, 100.0), 5.0))
            .id();
        let b = app
            .world
            .spawn(body_bundle(MapId(1), Vec2::new(80.0, 20.0), 5.0))
            .id();
        for entity in [a, b] {
            app.world.get_mut::<MoveIntent>(entity).unwrap().velocity = Vec2::new(0.0, 60.0);
        }
        for _ in 0..30 {
            app.tick();
        }
        // Half a second at 60 px/s moves each 30 px, in its own map.
        for (entity, expected) in [(a, 130.0), (b, 50.0)] {
            let y = app.world.get::<BodyState>(entity).unwrap().0.position.y;
            assert!((y - expected).abs() < 0.1, "expected y={expected}, got {y}");
        }
    }

    #[test]
    fn a_body_that_stays_in_its_map_walks_through_exits_unmoved() {
        let mut app = app(Maps::load(&project(), "scenes/a.ron").unwrap());
        let guard = app
            .world
            .spawn((
                body_bundle(MapId(0), Vec2::new(250.0, 120.0), 5.0),
                StaysInMap,
            ))
            .id();
        app.world.get_mut::<MoveIntent>(guard).unwrap().velocity = Vec2::new(90.0, 0.0);
        for _ in 0..60 {
            app.tick();
        }
        assert_eq!(*app.world.get::<MapId>(guard).unwrap(), MapId(0));
    }

    #[test]
    fn walking_into_an_exit_changes_map_and_places_at_spawn() {
        let mut app = app(Maps::load(&project(), "scenes/a.ron").unwrap());
        // Row 7 is below the level-1 block, so the way east is clear.
        let hero = app
            .world
            .spawn(body_bundle(MapId(0), Vec2::new(250.0, 120.0), 5.0))
            .id();
        app.world.get_mut::<MoveIntent>(hero).unwrap().velocity = Vec2::new(90.0, 0.0);
        for _ in 0..60 {
            app.tick();
            if *app.world.get::<MapId>(hero).unwrap() == MapId(1) {
                break;
            }
        }
        assert_eq!(*app.world.get::<MapId>(hero).unwrap(), MapId(1));
        let body = app.world.get::<BodyState>(hero).unwrap().0;
        let previous = app.world.get::<PreviousBody>(hero).unwrap();
        assert!(body.position.distance(Vec2::new(40.0, 40.0)) < 2.0);
        assert_eq!(
            previous.position,
            Vec2::new(40.0, 40.0),
            "no interpolation across maps"
        );
    }

    #[test]
    fn jump_intent_is_consumed() {
        let mut app = app(Maps::load(&project(), "scenes/a.ron").unwrap());
        let hero = app
            .world
            .spawn(body_bundle(MapId(0), Vec2::new(40.0, 120.0), 5.0))
            .id();
        app.world.get_mut::<MoveIntent>(hero).unwrap().jump = true;
        app.tick();
        assert!(!app.world.get::<MoveIntent>(hero).unwrap().jump);
        assert!(app.world.get::<BodyState>(hero).unwrap().0.elevation > 0.0);
    }
}

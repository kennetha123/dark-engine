//! Showcase: the game project's maps with characters that walk, run, jump, attack and talk, for
//! the host and for joined players.
//!
//! All gameplay is `dark_world` simulation. This module loads the project's scene, sheets, font
//! and strings, and draws: a list of [`DrawCharacter`]s in one map, from the host's own world or
//! from a client's [`dark_world::ClientSession`], with speech bubbles in the reader's language,
//! the tents and fires set down there, and the local player's body and hotbar. Looks made from
//! Spine skeletons are posed each frame from the host's clip and frame, and drawn as meshes.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use bevy_ecs::prelude::{Has, Without};
use dark_assets::{AssetError, Image, LoadedSheet, Localization, LookDef, Project};
use dark_combat::CombatDef;
use dark_core::{App, FrameTime};
use dark_life::{LifeDef, Need, Structure};
use dark_physics::{Cell, Shape};
use dark_render::{
    Mesh, MeshVertex, Outline, RenderError, Renderer, Sprite, SpriteKind, TextLayout, TextSystem,
    TextureId, layer,
};
use dark_spine::{Pose, Rig};
use dark_sprite::{Rect, SpriteSheet};
use dark_world::{
    BodyState, CharacterSheets, CharacterState, CharactersPlugin, CombatPlugin, Dormant,
    DrawCharacter, Hostile, LifePlugin, LifeView, Map, MapId, Maps, MapsPlugin, NetHost, NetId,
    Npc, PlayerAvatar, PreviousBody, ReplicationPlugin, Speech, StructureSnapshot, TalkPlugin,
    sheet_of, talk_target,
};
use glam::Vec2;

use crate::fx::{self, CombatFx, HealthTrails};

/// The project's font, read but not yet turned into glyphs (that needs no GPU, but is only
/// useful with one).
struct Font {
    data: Vec<u8>,
    size: f32,
    line_height: f32,
}

/// CPU side, before the window exists: maps and every sheet they use.
pub struct DemoScene {
    maps: Option<Maps>,
    sheets: HashMap<String, LoadedSheet>,
    characters: CharacterSheets,
    /// Each look by `LookId`, with its dialogue face if it has one.
    looks: Vec<(LookDef, Option<Image>)>,
    font: Option<Font>,
    strings: Localization,
    combat: CombatDef,
    life: LifeDef,
    /// Each item's hotbar icon, by item id.
    icons: Vec<(String, Image)>,
    /// Skeletons, by the `*.spine.ron` sheet path they are loaded from.
    rigs: HashMap<String, Rig>,
    story: dark_story::StoryDef,
}

/// One frame's world, as the view draws it.
pub struct Frame<'a> {
    pub maps: &'a Maps,
    pub map: MapId,
    pub characters: &'a [DrawCharacter],
    /// Day and hour.
    pub time: Option<(u32, f32)>,
    /// The local player's body and pack.
    pub life: Option<&'a LifeView>,
    /// Tents and campfires in `map`.
    pub structures: &'a [StructureSnapshot],
    /// The others in the local player's party.
    pub party: &'a [NetId],
    /// The local player's conversation choices, fades and the year's ending.
    pub story: &'a dark_world::StoryView,
}

impl DemoScene {
    pub fn load(project: &Project, scene: &str) -> Result<Self, AssetError> {
        let maps = Maps::load(project, scene)?;
        let combat_path = project.path("combat.ron");
        let combat = CombatDef::load_or_default(&combat_path).map_err(|e| AssetError::Invalid {
            path: combat_path,
            message: e.to_string(),
        })?;
        let looks = CharacterSheets::defs_in(&maps, &combat)
            .map_err(|message| AssetError::Invalid {
                path: project.path(scene),
                message,
            })?
            .into_iter()
            .map(|look| {
                let face = match &look.face {
                    Some(face) => Some(project.load_face(face, FACE_SIZE)?),
                    None => None,
                };
                Ok((look, face))
            })
            .collect::<Result<Vec<_>, AssetError>>()?;
        let life_path = project.path("life.ron");
        let life = LifeDef::load_or_default(&life_path).map_err(|e| AssetError::Invalid {
            path: life_path,
            message: e.to_string(),
        })?;
        let story_path = project.path("story.ron");
        let story = dark_story::StoryDef::load_or_default(&story_path).map_err(|e| {
            AssetError::Invalid {
                path: story_path,
                message: e.to_string(),
            }
        })?;
        let icons = life
            .items
            .iter()
            .filter_map(|(id, item)| item.icon.as_ref().map(|icon| (id, icon)))
            .map(|(id, icon)| {
                let image = project.load_icon(&icon.image, icon.size, icon.cell, ICON_SIZE)?;
                Ok((id.clone(), image))
            })
            .collect::<Result<Vec<_>, AssetError>>()?;
        let mut paths: Vec<String> = looks
            .iter()
            .flat_map(|(look, _)| std::iter::once(look.sheet.clone()).chain(look.attack.clone()))
            .collect();
        paths.extend(life.art.tent.iter().chain(&life.art.campfire).cloned());
        for map in &maps.maps {
            paths.push(map.def.ground.sheet.clone());
            paths.extend(map.props.iter().map(|p| p.sheet.clone()));
        }
        let mut sheets = HashMap::new();
        for path in paths {
            if let Entry::Vacant(slot) = sheets.entry(path) {
                let sheet = project.load_sheet(slot.key())?;
                tracing::info!("{}: {} frames", slot.key(), sheet.sheet.frames.len());
                slot.insert(sheet);
            }
        }
        // Spine sheets carry a skeleton, not an image: load it for the view.
        let mut rigs = HashMap::new();
        for (path, loaded) in &sheets {
            if let Some(spine) = &loaded.spine {
                let rig = Rig::load(project, &spine.def).map_err(|e| AssetError::Invalid {
                    path: project.path(path),
                    message: e.to_string(),
                })?;
                if spine.bake.skeleton_hash != rig.hash() {
                    tracing::warn!("{path}: the skeleton changed since it was baked; bake again");
                }
                rigs.insert(path.clone(), rig);
            }
        }
        // The same looks the headless host loads, from the sheets loaded here.
        let characters = CharacterSheets::build(&maps, &combat, project, |path| {
            Ok(sheets[path].sheet.clone())
        })?;
        let font = match &project.settings.font {
            Some(def) => {
                let path = project.path(&def.path);
                let data =
                    std::fs::read(&path).map_err(|source| AssetError::Io { path, source })?;
                Some(Font {
                    data,
                    size: def.size,
                    line_height: def.line_height.unwrap_or(def.size * 1.25),
                })
            }
            None => None,
        };
        let strings = Localization::load(project)?;
        tracing::info!(
            "maps: {}",
            maps.maps
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        Ok(Self {
            maps: Some(maps),
            sheets,
            characters,
            looks,
            font,
            strings,
            combat,
            life,
            icons,
            rigs,
            story,
        })
    }

    /// The project's storylets and endings, for the host.
    pub fn story_def(&self) -> dark_story::StoryDef {
        self.story.clone()
    }

    pub fn character_sheets(&self) -> CharacterSheets {
        self.characters.clone()
    }

    /// The maps, for the host simulation or a client session. Taken once.
    pub fn take_maps(&mut self) -> Maps {
        self.maps.take().expect("maps are taken once")
    }

    /// Adds maps, characters, NPCs, enemies and replication to the host simulation.
    pub fn install_host(&mut self, app: &mut App) {
        let sheets = self.character_sheets();
        let maps = self.take_maps();
        app.add_plugin(MapsPlugin(maps))
            .add_plugin(CharactersPlugin(sheets))
            .add_plugin(ReplicationPlugin)
            .add_plugin(TalkPlugin)
            .add_plugin(CombatPlugin(self.combat.clone()))
            .add_plugin(LifePlugin(self.life.clone()));
    }

    /// Uploads textures and lays out each map's static sprites.
    pub fn build_view(self, renderer: &mut Renderer, maps: &Maps) -> Result<DemoView, RenderError> {
        let mut textures = HashMap::new();
        for (path, loaded) in &self.sheets {
            let image = &loaded.image;
            let id = renderer.create_texture(path, image.width, image.height, &image.rgba)?;
            textures.insert(path.clone(), id);
        }
        let white = renderer.create_texture("white", 1, 1, &[255; 4])?;
        let jump_apex =
            maps.params.jump_speed * maps.params.jump_speed / (2.0 * maps.params.gravity);
        let views = maps
            .maps
            .iter()
            .map(|map| MapView::build(map, &self.sheets, &textures, white, jump_apex))
            .collect();
        let text = self.font.and_then(|font| {
            TextSystem::new(font.data, font.size, font.line_height)
                .inspect_err(|err| tracing::error!("cannot use the project font: {err}"))
                .ok()
        });
        let mut icons = HashMap::new();
        for (id, image) in &self.icons {
            let texture = renderer.create_texture(
                &format!("icon {id}"),
                image.width,
                image.height,
                &image.rgba,
            )?;
            icons.insert(id.clone(), texture);
        }
        let structure = |path: &Option<String>| {
            path.as_ref()
                .map(|p| (textures[p], self.sheets[p].sheet.clone()))
        };
        let tent = structure(&self.life.art.tent);
        let campfire = structure(&self.life.art.campfire);
        // Each skeleton's pages, smooth (painted art drawn turned and small), and its clips.
        let mut skeletons: Vec<SkeletonView> = Vec::new();
        let mut skeleton_of = HashMap::new();
        for (path, rig) in self.rigs {
            let spine = self.sheets[&path]
                .spine
                .as_ref()
                .expect("rigs come from Spine sheets");
            let mut pages = Vec::new();
            for page in &rig.pages {
                let image = &page.image;
                pages.push(renderer.create_texture_smooth(
                    &format!("{path} {}", page.name),
                    image.width,
                    image.height,
                    &image.rgba,
                )?);
            }
            let clips = spine
                .bake
                .clips
                .iter()
                .map(|(name, c)| (name.clone(), (c.animation.clone(), c.looping)))
                .collect();
            skeleton_of.insert(path, skeletons.len());
            skeletons.push(SkeletonView {
                height: spine.bake.height,
                rig,
                pages,
                clips,
            });
        }
        let mut looks = Vec::with_capacity(self.looks.len());
        for (i, (look, face)) in self.looks.iter().enumerate() {
            let base = textures[&look.sheet];
            let face = match face {
                Some(image) => Some(renderer.create_texture(
                    &format!("face {i}"),
                    image.width,
                    image.height,
                    &image.rgba,
                )?),
                None => None,
            };
            looks.push(LookView {
                base,
                attack: look.attack.as_ref().map_or(base, |a| textures[a]),
                face,
                name: look.name.clone(),
                skeleton: skeleton_of.get(&look.sheet).copied(),
            });
        }
        Ok(DemoView {
            looks,
            sheets: self.characters,
            maps: views,
            white,
            debug: false,
            frame_sprites: Vec::new(),
            camera_floor: 0.0,
            camera_map: None,
            text,
            strings: self.strings,
            layouts: HashMap::new(),
            language_shown: 0.0,
            recent_dialogue: None,
            fx: CombatFx::default(),
            trails: HealthTrails::default(),
            camera: Vec2::ZERO,
            icons,
            tent,
            campfire,
            comfort: self.life.rates.comfort,
            seconds: 0.0,
            skeletons,
            poses: HashMap::new(),
            frame_meshes: Vec::new(),
            faded: None,
            fade_left: 0.0,
            choosing: Vec::new(),
        })
    }
}

/// The host's characters in its own player's map, interpolated for this frame.
pub fn host_characters(app: &mut App) -> Option<(MapId, Vec<DrawCharacter>)> {
    let alpha = app.world.resource::<FrameTime>().alpha;
    let me = app.world.resource::<NetHost>().0.local_player()?;
    let mut query = app.world.query_filtered::<(
        &NetId,
        &MapId,
        &BodyState,
        &PreviousBody,
        &CharacterState,
        Option<&PlayerAvatar>,
        Has<Npc>,
        Option<&Speech>,
        Has<Hostile>,
    ), Without<Dormant>>();
    let map = query
        .iter(&app.world)
        .find(|(.., avatar, _, _, _)| avatar.is_some_and(|a| a.0 == me))
        .map(|(_, map, ..)| *map)?;
    let characters = query
        .iter(&app.world)
        .filter(|(_, m, ..)| **m == map)
        .map(
            |(id, _, body, previous, state, avatar, npc, speech, hostile)| DrawCharacter {
                id: *id,
                ground: previous.position.lerp(body.0.position, alpha),
                elevation: previous.elevation + (body.0.elevation - previous.elevation) * alpha,
                body: body.0,
                state: *state,
                you: avatar.is_some_and(|a| a.0 == me),
                npc,
                hostile,
                speech: speech.cloned(),
            },
        )
        .collect();
    Some((map, characters))
}

/// The host's own player's body, the structures in `map`, the others in their party, and what
/// their screen shows of the story.
pub fn host_life(
    app: &mut App,
    map: MapId,
) -> (
    Option<LifeView>,
    Vec<StructureSnapshot>,
    Vec<NetId>,
    dark_world::StoryView,
) {
    let me = app.world.resource::<NetHost>().0.local_player();
    let life = me.and_then(|me| dark_world::life_of(&mut app.world, me));
    let structures = dark_world::structures_in(&mut app.world, map);
    let avatar = me.and_then(|me| {
        let mut q = app
            .world
            .query::<(bevy_ecs::entity::Entity, &PlayerAvatar)>();
        q.iter(&app.world).find(|(_, a)| a.0 == me).map(|(e, _)| e)
    });
    let party = match (avatar, app.world.get_resource::<dark_world::PartyRoster>()) {
        (Some(avatar), Some(roster)) => dark_world::party_members(roster, avatar),
        _ => Vec::new(),
    };
    let story = me
        .map(|me| dark_world::story_view(&mut app.world, me))
        .unwrap_or_default();
    (life, structures, party, story)
}

/// Static sprites and debug overlay of one map.
struct MapView {
    size: Vec2,
    statics: Vec<Sprite>,
    overlay: Vec<Sprite>,
}

impl MapView {
    fn build(
        map: &Map,
        sheets: &HashMap<String, LoadedSheet>,
        textures: &HashMap<String, TextureId>,
        white: TextureId,
        jump_apex: f32,
    ) -> Self {
        let def = &map.def;
        let terrain = &map.collision.terrain;
        let size = Vec2::from(def.size);
        let mut statics = Vec::new();
        let mut overlay = Vec::new();

        let ground_sheet = &sheets[&def.ground.sheet].sheet;
        let ground_tex = textures[&def.ground.sheet];
        let frame_rect = |index: u32| ground_sheet.frames.get(index as usize).map(|f| f.rect);
        if let Some(rect) = frame_rect(def.ground.frame) {
            let mut sprite = Sprite::new(ground_tex, rect, Vec2::ZERO, Vec2::ZERO);
            sprite.repeat = size / Vec2::new(rect.w as f32, rect.h as f32);
            sprite.layer = layer::GROUND;
            statics.push(sprite);
        }

        // Raised tiles. A top is a floor: drawn in the terrain layer (per level) below every
        // character and prop, so whatever stands on it is always visible. Two pieces are drawn in
        // the world layer to hide things correctly:
        //  - a north cap, the strip of top that can overlap someone standing just behind (north
        //    of) the plateau, sorted by the plateau's north edge;
        //  - a cliff face down to the tile in front, sorted by its south edge, so anything
        //    standing in front draws after it.
        let top = frame_rect(def.terrain.top_frame.unwrap_or(def.ground.frame));
        let face = frame_rect(def.terrain.face_frame.unwrap_or(def.ground.frame));
        let (tile, lh) = (terrain.tile(), terrain.level_height());
        let rim = [0.0, 0.0, 0.0, 0.45];
        for row in 0..i64::from(terrain.rows()) {
            for col in 0..i64::from(terrain.cols()) {
                let cell = terrain.cell(col, row).unwrap_or_default();
                let origin = Vec2::new(col as f32, row as f32) * tile;
                if cell == Cell::Wall {
                    overlay.push(tint(
                        white,
                        origin,
                        Vec2::splat(tile),
                        [0.9, 0.1, 0.1, 0.35],
                        0.0,
                    ));
                    continue;
                }
                let level = cell.level().unwrap_or(0);
                if level == 0 {
                    continue;
                }
                let height = f32::from(level) * lh;
                let lifted = origin - Vec2::new(0.0, height);
                // Clamped so absurdly high levels never reach the world layer.
                let floor_layer = layer::TERRAIN + i32::from(level.min(40));
                overlay.push(tint(
                    white,
                    lifted,
                    Vec2::splat(tile),
                    [0.2, 0.4, 1.0, 0.12 * f32::from(level)],
                    0.0,
                ));
                let neighbour =
                    |dc: i64, dr: i64| terrain.cell(col + dc, row + dr).and_then(Cell::level);
                let lower = |dc: i64, dr: i64| neighbour(dc, dr).is_some_and(|n| n < level);
                let on_floor = |mut s: Sprite| {
                    s.layer = floor_layer;
                    s.sort_y = origin.y;
                    s
                };
                if let Some(rect) = top {
                    statics.push(on_floor(Sprite::new(
                        ground_tex,
                        sub_rect(rect, origin, Vec2::splat(tile)),
                        lifted,
                        Vec2::ZERO,
                    )));
                }
                // Darkened rims where the neighbour is lower, so plateaus read at a glance.
                if lower(-1, 0) {
                    statics.push(on_floor(tint(
                        white,
                        lifted,
                        Vec2::new(1.0, tile),
                        rim,
                        0.0,
                    )));
                }
                if lower(1, 0) {
                    statics.push(on_floor(tint(
                        white,
                        lifted + Vec2::new(tile - 1.0, 0.0),
                        Vec2::new(1.0, tile),
                        rim,
                        0.0,
                    )));
                }
                if let Some(north) = neighbour(0, -1)
                    && north < level
                {
                    // Anyone standing north of the edge overlaps at most the height difference
                    // of this top (clipped to one tile).
                    let cap = (f32::from(level - north) * lh).min(tile);
                    if let Some(rect) = top {
                        let mut s = Sprite::new(
                            ground_tex,
                            sub_rect(rect, origin, Vec2::new(tile, cap)),
                            lifted,
                            Vec2::ZERO,
                        );
                        s.sort_y = origin.y;
                        statics.push(s);
                    }
                    statics.push(tint(white, lifted, Vec2::new(tile, 1.0), rim, origin.y));
                }
                let front = neighbour(0, 1);
                if let (Some(rect), Some(front)) = (face, front)
                    && front < level
                {
                    let south = origin.y + tile;
                    for step in front..level {
                        let y = south - f32::from(step + 1) * lh;
                        let src = sub_rect(rect, Vec2::new(origin.x, y), Vec2::new(tile, lh));
                        let mut s =
                            Sprite::new(ground_tex, src, Vec2::new(origin.x, y), Vec2::ZERO);
                        s.color = [0.75, 0.75, 0.75, 1.0];
                        s.sort_y = south;
                        statics.push(s);
                    }
                }
            }
        }

        for prop in &map.props {
            let Some(frame) = sheets[&prop.sheet].sheet.frames.get(prop.frame as usize) else {
                tracing::warn!("{}: no frame {}", prop.sheet, prop.frame);
                continue;
            };
            let at = Vec2::from(prop.position);
            let ground = map.collision.ground_under(at, 0.5);
            let ground = if ground.is_finite() { ground } else { 0.0 };
            let mut sprite = Sprite::new(
                textures[&prop.sheet],
                frame.rect,
                at - Vec2::new(0.0, ground),
                frame.pivot,
            );
            sprite.sort_y = at.y;
            statics.push(sprite);
        }
        for c in &map.collision.colliders {
            let (half, outline) = match c.shape {
                Shape::Circle { radius } => (Vec2::splat(radius), Outline::Circle),
                Shape::Rect { half } => (half, Outline::Rect),
            };
            // Orange: blocks at any height. Cyan: low enough to jump over or stand on.
            let color = if c.height <= jump_apex {
                [0.3, 0.9, 1.0, 0.9]
            } else {
                [1.0, 0.6, 0.0, 0.9]
            };
            overlay.push(Sprite::outline(
                white,
                outline,
                c.center - half - Vec2::new(0.0, c.base),
                half * 2.0,
                color,
            ));
        }
        for exit in &map.exits {
            overlay.push(tint(
                white,
                exit.min,
                exit.max - exit.min,
                [1.0, 1.0, 0.2, 0.35],
                0.0,
            ));
        }
        tracing::info!("{}: {} static sprites", map.name, statics.len());
        Self {
            size,
            statics,
            overlay,
        }
    }
}

/// Southmost ground-plane y of a prop's footprint.
fn south_edge(prop: &dark_physics::Collider) -> f32 {
    match prop.shape {
        Shape::Circle { radius } => prop.center.y + radius,
        Shape::Rect { half } => prop.center.y + half.y,
    }
}

/// A flat coloured rectangle, sorted at `sort_y`.
fn tint(white: TextureId, top_left: Vec2, size: Vec2, color: [f32; 4], sort_y: f32) -> Sprite {
    let mut s = Sprite::fill(white, top_left, size, color);
    s.sort_y = sort_y;
    s
}

/// The `size` part of a seamless texture that lines up with world position `at`. Clipped to
/// the texture: a level taller than the face texture would show a gap (none in current data).
fn sub_rect(texture: Rect, at: Vec2, size: Vec2) -> Rect {
    let (w, h) = (
        (size.x as u32).min(texture.w),
        (size.y as u32).min(texture.h),
    );
    let wrap = |v: f32, span: u32, len: u32| (v.max(0.0) as u32 % span).min(span - len);
    Rect::new(
        texture.x + wrap(at.x, texture.w, w),
        texture.y + wrap(at.y, texture.h, h),
        w,
        h,
    )
}

/// Bubbles and prompts sit this far above a character's feet: about head height for the 64 px
/// character cells in use, whose figures stand some 36–40 px tall.
const HEAD_HEIGHT: f32 = 42.0;
/// Dialogue faces are shown at this size: half an RPG Maker face.
const FACE_SIZE: u32 = 72;
/// Speech bubbles wrap at this width.
const BUBBLE_WIDTH: f32 = 168.0;
/// How long the dialogue window holds its last line when neither side is speaking.
const DIALOGUE_GRACE_SECS: f32 = 0.25;
/// Seconds the language name stays on screen after switching.
const LANGUAGE_BANNER_SECS: f32 = 2.0;
/// Laid-out texts kept between frames before the cache is cleared.
const MAX_CACHED_LAYOUTS: usize = 256;

const INK: [f32; 4] = [0.07, 0.05, 0.09, 1.0];
const PAPER: [f32; 4] = [1.0, 0.98, 0.93, 1.0];
/// Health just lost, before it drains away.
const TRAIL: [f32; 4] = [1.0, 0.86, 0.55, 1.0];

/// What a health bar shows.
#[derive(Clone, Copy)]
struct Bar {
    health: u16,
    /// Where the lingering part ends.
    trail: f32,
    max: u16,
    hostile: bool,
}
/// Hotbar icons are painted art, resized to this.
const ICON_SIZE: u32 = 16;
/// A hotbar slot's size, and how many there are (keys 1 to 8).
const SLOT: f32 = 20.0;
const HOTBAR_SLOTS: usize = 8;
/// A need gauge's size.
const GAUGE: Vec2 = Vec2::new(6.0, 16.0);
/// The year's ending card wraps at this width.
const ENDING_WIDTH: f32 = 360.0;
/// Party members' health bars, top right.
const PARTY_BAR: f32 = 40.0;
/// Statuses listed under the gauges.
const MAX_STATUSES: usize = 4;

/// Each need's gauge colour.
fn need_colour(need: Need) -> [f32; 4] {
    match need {
        Need::Hunger => [0.95, 0.65, 0.25, 1.0],
        Need::Thirst => [0.35, 0.65, 1.0, 1.0],
        Need::Fatigue => [0.65, 0.5, 0.95, 1.0],
        Need::Bladder => [0.95, 0.9, 0.35, 1.0],
        Need::Bowel => [0.65, 0.45, 0.3, 1.0],
        Need::Hygiene => [0.5, 0.8, 0.45, 1.0],
    }
}

/// Speaker names in the dialogue window.
const NAME: [f32; 4] = [1.0, 0.78, 0.32, 1.0];
/// The dialogue window: its height (a face plus padding, room for a name and three lines),
/// distance from the view edges, and inner padding.
const WINDOW_HEIGHT: f32 = 84.0;
const WINDOW_MARGIN: f32 = 8.0;
const WINDOW_PAD: f32 = 6.0;

/// How a look draws, by `LookId`.
#[derive(Clone)]
struct LookView {
    base: TextureId,
    attack: TextureId,
    face: Option<TextureId>,
    /// String-table key.
    name: Option<String>,
    /// A Spine skeleton instead of sprites: index into [`DemoView::skeletons`].
    skeleton: Option<usize>,
}

/// A loaded skeleton for drawing.
struct SkeletonView {
    rig: Rig,
    pages: Vec<TextureId>,
    /// Engine clip name to the Spine animation and whether it loops.
    clips: HashMap<String, (String, bool)>,
    /// World pixels, for what shows over heads.
    height: f32,
}

/// How far above `c`'s feet bubbles, prompts and health bars go: a skeleton's own height, else
/// the sprite characters' head.
fn head_height(looks: &[LookView], skeletons: &[SkeletonView], c: &DrawCharacter) -> f32 {
    looks
        .get(usize::from(c.state.look.0))
        .and_then(|l| l.skeleton)
        .map_or(HEAD_HEIGHT, |s| skeletons[s].height + 6.0)
}

/// GPU side: turns characters and maps into sprites each frame.
pub struct DemoView {
    looks: Vec<LookView>,
    sheets: CharacterSheets,
    maps: Vec<MapView>,
    white: TextureId,
    debug: bool,
    frame_sprites: Vec<Sprite>,
    /// Height of what the local character last stood on. The camera follows that, not the jump
    /// itself, so a jump reads as the character rising rather than the world dropping.
    camera_floor: f32,
    camera_map: Option<MapId>,
    /// `None` when the project has no font: speech then goes unshown.
    text: Option<TextSystem>,
    strings: Localization,
    layouts: HashMap<String, TextLayout>,
    /// Seconds left to show the language name.
    language_shown: f32,
    /// The last line of the local player's conversation, and how much longer to hold it.
    recent_dialogue: Option<(DrawCharacter, f32)>,
    fx: CombatFx,
    trails: HealthTrails,
    /// Where the camera looked last frame: where the listener is.
    camera: Vec2,
    /// Hotbar icons by item id.
    icons: HashMap<String, TextureId>,
    /// Art for set-down structures.
    tent: Option<(TextureId, SpriteSheet)>,
    campfire: Option<(TextureId, SpriteSheet)>,
    /// Felt temperatures shown plain; colder shows blue, warmer red (the project's `Rates`).
    comfort: (i32, i32),
    /// Seconds drawn, for animating structures.
    seconds: f32,
    skeletons: Vec<SkeletonView>,
    /// Each skeletal character's posed skeleton, by who it is.
    poses: HashMap<NetId, (usize, Pose)>,
    frame_meshes: Vec<Mesh>,
    /// Fades to black seen so far (`None` before the first frame), and seconds left of the one
    /// showing.
    faded: Option<u32>,
    fade_left: f32,
    /// Conversation choices showing (their node indices, in order): number keys answer instead of
    /// using the hotbar.
    choosing: Vec<u8>,
}

impl DemoView {
    pub fn toggle_debug(&mut self) {
        self.debug = !self.debug;
    }

    /// Switches to the project's next language and shows its name for a moment.
    pub fn cycle_language(&mut self) {
        self.strings.cycle();
        self.language_shown = LANGUAGE_BANNER_SECS;
        tracing::info!("language: {}", self.strings.language());
    }

    fn look(&self, c: &DrawCharacter) -> LookView {
        self.looks
            .get(usize::from(c.state.look.0))
            .unwrap_or(&self.looks[0])
            .clone()
    }

    /// Sounds for this frame's events: FMOD event paths and where they happened.
    pub fn sounds(&self) -> &[(&'static str, Vec2)] {
        &self.fx.sounds
    }

    /// The choice number key `n` (from 1) answers with, as a `TickInput::choice`, if choices
    /// are showing.
    pub fn choice_for(&self, n: u8) -> Option<Option<u8>> {
        if self.choosing.is_empty() {
            return None;
        }
        Some(
            self.choosing
                .get(usize::from(n).wrapping_sub(1))
                .map(|index| index + 1),
        )
    }

    /// Where the camera is looking: where the player hears from.
    pub fn camera(&self) -> Vec2 {
        self.camera
    }

    /// False if the project has no such language.
    pub fn set_language(&mut self, code: &str) -> bool {
        self.strings.set_language(code)
    }

    /// Draws `frame`, following the local player's character.
    pub fn draw(&mut self, renderer: &mut Renderer, frame: &Frame, dt: f32) {
        let (maps, map, characters) = (frame.maps, frame.map, frame.characters);
        self.seconds += dt;
        // Hitstop holds characters as they were when a hit landed; flashes and sparks start here.
        let shown = self.fx.update(characters, &self.sheets, dt);
        let characters = shown.as_slice();
        let collision = &maps.get(map).collision;
        self.frame_sprites.clear();
        self.frame_meshes.clear();
        self.draw_structures(frame.structures, collision);
        let map_view = map.0 as usize;
        self.frame_sprites
            .extend_from_slice(&self.maps[map_view].statics);
        let mut focus = None;

        for c in characters {
            // What the character stands on places its blob shadow, and a prop underfoot must draw
            // before it even where the feet are north of the prop's own pivot.
            let surface = collision.support(c.ground, c.body.radius, c.elevation, &maps.params);
            let surface = if surface.is_finite() {
                surface.min(c.elevation)
            } else {
                0.0
            };
            let sort_y = collision
                .supporting_prop(c.ground, c.body.radius, c.elevation, &maps.params)
                .map_or(c.ground.y, |prop| c.ground.y.max(south_edge(prop) + 0.1));

            let sheet = sheet_of(&c.state, &self.sheets);
            let look = self.look(c);
            let texture = if c.state.on_attack_sheet {
                look.attack
            } else {
                look.base
            };
            // Flushed red when hit; fading when a dead enemy is about to go.
            let flash = self.fx.flash(c.id);
            let tint = [
                1.0,
                1.0 - 0.7 * flash,
                1.0 - 0.7 * flash,
                fx::corpse_alpha(c),
            ];
            if let Some(skeleton) = look.skeleton {
                let feet = (c.ground - Vec2::new(0.0, c.elevation)).round();
                self.draw_skeleton(c, skeleton, feet, sort_y, tint);
            } else if let Some(frame) = c
                .state
                .anim
                .frame(&sheet.clips)
                .and_then(|f| sheet.frames.get(f as usize))
            {
                // A mirrored clip mirrors the pivot too, so the feet stay put.
                let flip = c.state.anim.flipped(&sheet.clips);
                let pivot = if flip {
                    Vec2::new(frame.rect.w as f32 - frame.pivot.x, frame.pivot.y)
                } else {
                    frame.pivot
                };
                let mut sprite = Sprite::new(texture, frame.rect, c.ground, pivot);
                sprite.flip_x = flip;
                sprite.kind = SpriteKind::Character;
                sprite.lift = c.elevation;
                sprite.sort_y = sort_y;
                sprite.color = tint;
                self.frame_sprites.push(sprite);
            }
            // Blob shadow on the surface below, shrinking with height; a little below the pivot
            // so it peeks out from under the feet when standing.
            let shrink = 1.0 - ((c.elevation - surface) / 60.0).clamp(0.0, 0.35);
            let size = Vec2::new(16.0, 6.0) * shrink;
            let center = c.ground - Vec2::new(0.0, surface - 2.0);
            let mut blob =
                Sprite::fill(self.white, center - size / 2.0, size, [0.0, 0.0, 0.0, 0.4]);
            blob.kind = SpriteKind::Blob;
            blob.sort_y = sort_y - 0.01;
            self.frame_sprites.push(blob);

            if self.debug {
                let r = c.body.radius;
                let color = if c.you {
                    [0.2, 1.0, 0.4, 1.0]
                } else {
                    [1.0, 0.4, 1.0, 1.0]
                };
                let mut feet = Sprite::outline(
                    self.white,
                    Outline::Circle,
                    c.ground - Vec2::new(0.0, c.elevation) - Vec2::splat(r),
                    Vec2::splat(r * 2.0),
                    color,
                );
                feet.layer = layer::DEBUG;
                self.frame_sprites.push(feet);
            }
            if c.you {
                focus = Some((c.ground, c.body.grounded, c.body.elevation, surface));
            }
        }
        if self.debug {
            for s in &self.maps[map_view].overlay {
                let mut s = *s;
                s.layer = layer::DEBUG;
                self.frame_sprites.push(s);
            }
        }

        // The camera follows what the local character stands on, eased, so a jump reads as
        // rising and landing on a ledge does not snap the view. A new map snaps it.
        let (w, h) = renderer.internal_size();
        let half = Vec2::new(w as f32, h as f32) / 2.0;
        let focus_point = match focus {
            Some((ground, grounded, elevation, surface)) => {
                if self.camera_map != Some(map) {
                    self.camera_map = Some(map);
                    self.camera_floor = surface;
                } else if grounded {
                    self.camera_floor +=
                        (elevation - self.camera_floor) * (1.0 - (-12.0 * dt).exp());
                }
                ground - Vec2::new(0.0, self.camera_floor)
            }
            None => self.maps[map_view].size / 2.0,
        };
        let size = self.maps[map_view].size;
        let camera = focus_point.clamp(half, (size - half).max(half)) + self.fx.shake();
        self.camera = camera;
        self.draw_sparks();
        self.draw_health(characters, dt);
        let screen = (camera - half, half * 2.0);
        self.draw_interface(renderer, characters, frame, screen, dt);
        self.draw_fade(frame.story.faded, screen, dt);
        // Skeletons of those no longer here are dropped.
        self.poses
            .retain(|id, _| characters.iter().any(|c| c.id == *id));
        renderer.render_with(
            camera,
            [0.0; 3],
            &mut self.frame_sprites,
            &self.frame_meshes,
        );
    }

    /// Speech bubbles, sleepers' snores, the talk prompt, the local player's dialogue window, the
    /// day and time, and the language banner, kept inside the view whose top-left is `view_min`.
    fn draw_interface(
        &mut self,
        renderer: &mut Renderer,
        characters: &[DrawCharacter],
        frame: &Frame,
        (view_min, view_size): (Vec2, Vec2),
        dt: f32,
    ) {
        let (time, life) = (frame.time, frame.life);
        self.choosing = frame.story.choices.iter().map(|(i, _)| *i).collect();
        let Some(text) = &mut self.text else {
            return;
        };
        if self.layouts.len() > MAX_CACHED_LAYOUTS {
            self.layouts.clear();
        }
        let mut layout = |s: &str, width: f32| {
            self.layouts
                .entry(format!("{width}\u{0}{s}"))
                .or_insert_with(|| text.layout(s, width))
                .clone()
        };
        let me = characters.iter().find(|c| c.you);
        // The local player's own conversation: the latest line said by them or to them. It goes
        // in the dialogue window; everyone else's speech stays in bubbles.
        let mine = |c: &&DrawCharacter| {
            me.zip(c.speech.as_ref())
                .is_some_and(|(me, s)| c.id == me.id || s.to == Some(me.id))
        };
        let latest = characters
            .iter()
            .filter(mine)
            .max_by_key(|c| c.speech.as_ref().map_or(0, |s| s.ticks_left));
        // Between one side's line and the other's there can be a moment with neither: the local
        // character's speech is current, the other side's is drawn 100 ms behind. Hold the last line
        // briefly so the window does not blink shut.
        let dialogue = match latest {
            Some(speaker) => {
                self.recent_dialogue = Some((speaker.clone(), DIALOGUE_GRACE_SECS));
                Some(speaker.clone())
            }
            None => match &mut self.recent_dialogue {
                Some((speaker, left)) if *left > 0.0 => {
                    *left -= dt;
                    Some(speaker.clone())
                }
                _ => None,
            },
        };

        // (layout, anchor above the head, sort order, is a prompt)
        let mut bubbles: Vec<(TextLayout, Vec2, f32, bool)> = Vec::new();
        for c in characters.iter().filter(|c| !mine(c)) {
            if let Some(speech) = &c.speech {
                let head = c.ground
                    - Vec2::new(
                        0.0,
                        c.elevation + head_height(&self.looks, &self.skeletons, c),
                    );
                let said = layout(self.strings.text(&speech.line), BUBBLE_WIDTH);
                bubbles.push((said, head, c.ground.y, false));
            }
        }
        // Sleepers snore in a small dark tag (the same look as the talk prompt).
        // Out cold looks the same from outside.
        for c in characters
            .iter()
            .filter(|c| (c.state.sleeping || c.state.impaired.out) && c.speech.is_none())
        {
            let head = c.ground
                - Vec2::new(
                    0.0,
                    c.elevation + head_height(&self.looks, &self.skeletons, c),
                );
            bubbles.push((layout("z Z z", BUBBLE_WIDTH), head, c.ground.y, true));
        }
        // Who the local player would talk to, unless a conversation is already showing.
        if let Some(me) = me
            && dialogue.is_none()
        {
            let npcs = characters
                .iter()
                .filter(|c| c.npc)
                .map(|c| (c, c.ground, c.elevation));
            if let Some(npc) = talk_target((me.ground, me.elevation), npcs)
                && npc.speech.is_none()
            {
                let prompt = format!("E  {}", self.strings.text("ui.talk"));
                let head = npc.ground
                    - Vec2::new(
                        0.0,
                        npc.elevation + head_height(&self.looks, &self.skeletons, npc),
                    );
                bubbles.push((layout(&prompt, BUBBLE_WIDTH), head, npc.ground.y, true));
            }
        }

        // The window: a panel along the bottom, the speaker's face on the left, name then text.
        let window = dialogue.as_ref().and_then(|speaker| {
            let speech = speaker.speech.as_ref()?;
            let look = self
                .looks
                .get(usize::from(speaker.state.look.0))
                .unwrap_or(&self.looks[0]);
            let size = Vec2::new(view_size.x - 2.0 * WINDOW_MARGIN, WINDOW_HEIGHT);
            let face_room = look.face.map_or(0.0, |_| FACE_SIZE as f32 + WINDOW_PAD);
            let text_width = size.x - face_room - 2.0 * WINDOW_PAD;
            let name = look
                .name
                .as_ref()
                .map(|key| layout(self.strings.text(key), text_width));
            let said = layout(self.strings.text(&speech.line), text_width);
            Some((look.face, name, said, size, face_room))
        });

        // "Day 12  14:00" in the top-right corner.
        let clock = time.map(|(day, hour)| {
            let day = self
                .strings
                .text("ui.day")
                .replace("{day}", &(day + 1).to_string());
            let (h, m) = (hour as u32, (hour.fract() * 60.0) as u32);
            layout(&format!("{day}  {h:02}:{m:02}"), BUBBLE_WIDTH)
        });
        // The body: the air's temperature (tinted by how it feels, clothes and fire included),
        // and what ails it.
        let body = life.map(|life| {
            let air = format!("{}°C", (life.air as f32 / 100.0).round());
            let statuses: Vec<TextLayout> = life
                .statuses
                .iter()
                .take(MAX_STATUSES)
                .map(|s| layout(self.strings.text(s.key()), BUBBLE_WIDTH))
                .collect();
            let counts: Vec<Option<TextLayout>> = life
                .slots
                .iter()
                .map(|(_, n)| (*n != 1).then(|| layout(&n.to_string(), BUBBLE_WIDTH)))
                .collect();
            (life, layout(&air, BUBBLE_WIDTH), statuses, counts)
        });
        // The party, top right under the clock: whoever of it is in this map, by name.
        let party: Vec<(TextLayout, Bar)> = frame
            .party
            .iter()
            .filter_map(|id| characters.iter().find(|c| c.id == *id))
            .map(|c| {
                let look = self.looks.get(usize::from(c.state.look.0));
                let name = look
                    .and_then(|l| l.name.as_deref())
                    .map_or("?", |key| self.strings.text(key));
                let max = self.sheets.look(c.state.look).moveset.health.max(1);
                let health = c.state.fighter.health;
                let bar = Bar {
                    health,
                    trail: self.trails.trail(c.id).unwrap_or(f32::from(health)),
                    max,
                    hostile: false,
                };
                (layout(name, BUBBLE_WIDTH), bar)
            })
            .collect();
        // Conversation choices, numbered for the keys that pick them; the year's ending.
        let choices: Vec<TextLayout> = frame
            .story
            .choices
            .iter()
            .enumerate()
            .map(|(i, (_, key))| {
                let line = format!("{}  {}", i + 1, self.strings.text(key));
                layout(&line, view_size.x - 4.0 * WINDOW_MARGIN)
            })
            .collect();
        let ending = frame
            .story
            .ending
            .as_ref()
            .map(|key| layout(self.strings.text(key), ENDING_WIDTH));
        self.language_shown = (self.language_shown - dt).max(0.0);
        let banner = (self.language_shown > 0.0)
            .then(|| layout(self.strings.text("ui.language"), BUBBLE_WIDTH));

        let texture = match text.texture(renderer) {
            Ok(texture) => texture,
            Err(err) => {
                tracing::error!("cannot upload glyphs: {err}");
                return;
            }
        };
        for (said, head, order, prompt) in bubbles {
            self.bubble(&said, head, order, prompt, texture, view_min, view_size);
        }
        if let Some((face, name, said, size, face_room)) = window {
            let order = 2e9;
            let at = view_min + Vec2::new(WINDOW_MARGIN, view_size.y - WINDOW_MARGIN - size.y);
            self.panel(at, size, [0.03, 0.03, 0.07, 0.88], Some(PAPER), order);
            if let Some(face) = face {
                let mut s = Sprite::new(
                    face,
                    Rect::new(0, 0, FACE_SIZE, FACE_SIZE),
                    at + Vec2::splat(WINDOW_PAD),
                    Vec2::ZERO,
                );
                s.layer = layer::UI;
                s.sort_y = order + 0.01;
                self.frame_sprites.push(s);
            }
            let mut pen = at + Vec2::new(face_room + WINDOW_PAD, WINDOW_PAD - 2.0);
            if let Some(name) = name {
                self.frame_sprites.extend(TextSystem::sprites(
                    &name,
                    texture,
                    pen,
                    NAME,
                    layer::UI,
                    order + 0.02,
                ));
                pen.y += name.size.y;
            }
            self.frame_sprites.extend(TextSystem::sprites(
                &said,
                texture,
                pen,
                PAPER,
                layer::UI,
                order + 0.02,
            ));
        }
        if let Some(clock) = clock {
            let at = view_min + Vec2::new(view_size.x - clock.size.x - 8.0, 6.0);
            let pad = Vec2::new(4.0, 1.0);
            self.panel(
                at - pad,
                clock.size + pad * 2.0,
                [0.0, 0.0, 0.0, 0.6],
                None,
                3e9,
            );
            self.frame_sprites.extend(TextSystem::sprites(
                &clock,
                texture,
                at,
                PAPER,
                layer::UI,
                3e9 + 1.0,
            ));
        }
        // The local player's health, top left.
        if let Some(me) = characters.iter().find(|c| c.you) {
            let max = self.sheets.look(me.state.look).moveset.health.max(1);
            let health = me.state.fighter.health;
            let bar = Bar {
                health,
                trail: self.trails.trail(me.id).unwrap_or(f32::from(health)),
                max,
                hostile: false,
            };
            self.bar(view_min + Vec2::new(8.0, 8.0), 80.0, bar, 3e9);
        }
        // Choices sit above the dialogue window.
        if !choices.is_empty() {
            let height: f32 = choices.iter().map(|c| c.size.y).sum::<f32>() + 2.0 * WINDOW_PAD;
            let width = view_size.x - 2.0 * WINDOW_MARGIN;
            let at = view_min
                + Vec2::new(
                    WINDOW_MARGIN,
                    view_size.y - 2.0 * WINDOW_MARGIN - WINDOW_HEIGHT - height,
                );
            self.panel(
                at,
                Vec2::new(width, height),
                [0.03, 0.03, 0.07, 0.92],
                Some(NAME),
                2.1e9,
            );
            let mut pen = at + Vec2::new(WINDOW_PAD, WINDOW_PAD - 2.0);
            for choice in &choices {
                self.frame_sprites.extend(TextSystem::sprites(
                    choice,
                    texture,
                    pen,
                    PAPER,
                    layer::UI,
                    2.1e9 + 0.01,
                ));
                pen.y += choice.size.y;
            }
        }
        if let Some(ending) = &ending {
            let pad = Vec2::splat(12.0);
            let size = ending.size + pad * 2.0;
            let at = (view_min + (view_size - size) / 2.0).round();
            self.panel(at, size, [0.02, 0.02, 0.04, 0.94], Some(NAME), 3.5e9);
            self.frame_sprites.extend(TextSystem::sprites(
                ending,
                texture,
                at + pad,
                PAPER,
                layer::UI,
                3.5e9 + 0.01,
            ));
        }
        let mut pen = view_min + Vec2::new(view_size.x - 8.0, 24.0);
        for (name, bar) in &party {
            let at = Vec2::new(pen.x - name.size.x.max(PARTY_BAR), pen.y);
            self.shadowed(name, texture, at, PAPER, 3e9);
            self.bar(at + Vec2::new(0.0, name.size.y - 2.0), PARTY_BAR, *bar, 3e9);
            pen.y += name.size.y + 6.0;
        }
        if let Some((life, air, statuses, counts)) = body {
            self.draw_body(life, &air, &statuses, texture, view_min);
            self.draw_hotbar(life, &counts, texture, view_min, view_size);
        }
        if let Some(banner) = banner {
            let at = view_min + Vec2::new(8.0, 18.0);
            let pad = Vec2::new(4.0, 1.0);
            self.panel(
                at - pad,
                banner.size + pad * 2.0,
                [0.0, 0.0, 0.0, 0.6],
                None,
                3e9,
            );
            self.frame_sprites.extend(TextSystem::sprites(
                &banner,
                texture,
                at,
                PAPER,
                layer::UI,
                3e9 + 1.0,
            ));
        }
    }

    /// A rounded box with a tail pointing down at `head`, holding `said`. Kept inside the view;
    /// the tail stays over the speaker.
    #[allow(clippy::too_many_arguments)]
    fn bubble(
        &mut self,
        said: &TextLayout,
        head: Vec2,
        order: f32,
        prompt: bool,
        texture: TextureId,
        view_min: Vec2,
        view_size: Vec2,
    ) {
        let pad = Vec2::new(5.0, 2.0);
        let size = said.size + pad * 2.0;
        let tail = 4.0;
        let margin = 2.0;
        let lo = view_min + Vec2::splat(margin);
        let hi = (view_min + view_size - size - Vec2::splat(margin)).max(lo);
        let top_left = (head - Vec2::new(size.x / 2.0, size.y + tail))
            .clamp(lo, hi)
            .round();
        let (fill, ink) = if prompt {
            ([0.0, 0.0, 0.0, 0.65], PAPER)
        } else {
            (PAPER, INK)
        };
        let border = (!prompt).then_some(INK);
        self.panel(top_left, size, fill, border, order);
        if !prompt {
            // The tail: shrinking rows under the box, outlined, pointing at the speaker.
            let x = head
                .x
                .clamp(top_left.x + 4.0, top_left.x + size.x - 5.0)
                .round();
            let y = top_left.y + size.y;
            for row in 0..tail as i32 {
                let half = (tail as i32 - 1 - row) as f32;
                let y = y + row as f32;
                self.rect(
                    Vec2::new(x - half - 1.0, y),
                    Vec2::new(2.0 * half + 3.0, 1.0),
                    INK,
                    order,
                );
                self.rect(
                    Vec2::new(x - half, y),
                    Vec2::new(2.0 * half + 1.0, 1.0),
                    fill,
                    order + 0.001,
                );
            }
        }
        self.frame_sprites.extend(TextSystem::sprites(
            said,
            texture,
            top_left + pad,
            ink,
            layer::UI,
            order + 0.002,
        ));
    }

    /// Tents and campfires, standing in the world like props; a fire flickers through its clip.
    fn draw_structures(
        &mut self,
        structures: &[StructureSnapshot],
        collision: &dark_physics::World,
    ) {
        let ticks = (self.seconds * 60.0) as u32;
        for s in structures {
            let (art, clip) = match s.structure {
                Structure::Tent => (&self.tent, None),
                Structure::Campfire { .. } => (&self.campfire, Some("burn")),
            };
            let Some((texture, sheet)) = art else {
                continue;
            };
            let index = clip
                .and_then(|name| sheet.clip_id(name))
                .and_then(|id| sheet.clips.get(usize::from(id.0)))
                .filter(|c| !c.frames.is_empty())
                .map_or(0, |c| {
                    c.frames[(ticks / c.ticks_per_frame.max(1)) as usize % c.frames.len()]
                });
            let Some(frame) = sheet.frames.get(index as usize) else {
                continue;
            };
            let ground = collision.ground_under(s.at, 0.5);
            let ground = if ground.is_finite() { ground } else { 0.0 };
            let mut sprite = Sprite::new(
                *texture,
                frame.rect,
                s.at - Vec2::new(0.0, ground),
                frame.pivot,
            );
            sprite.sort_y = s.at.y;
            self.frame_sprites.push(sprite);
        }
    }

    /// Under the health bar: a gauge per need (fuller is worse), the air's temperature coloured
    /// by how it feels, and the body's statuses.
    fn draw_body(
        &mut self,
        life: &LifeView,
        air: &TextLayout,
        statuses: &[TextLayout],
        texture: TextureId,
        view_min: Vec2,
    ) {
        let order = 3e9;
        let top = view_min + Vec2::new(8.0, 16.0);
        for (i, need) in Need::ALL.into_iter().enumerate() {
            let at = top + Vec2::new(i as f32 * (GAUGE.x + 2.0), 0.0);
            let level = f32::from(life.needs[i].min(1000)) / 1000.0;
            let colour = if life.needs[i] >= 800 {
                [0.95, 0.2, 0.15, 1.0]
            } else {
                need_colour(need)
            };
            let inner = GAUGE - Vec2::splat(2.0);
            let filled = (inner.y * level).round();
            self.rect(at, GAUGE, INK, order);
            self.rect(
                at + Vec2::ONE,
                inner,
                [0.18, 0.18, 0.22, 1.0],
                order + 0.001,
            );
            self.rect(
                at + Vec2::new(1.0, 1.0 + inner.y - filled),
                Vec2::new(inner.x, filled),
                colour,
                order + 0.002,
            );
        }
        // Blue when it feels cold, red when hot (the body's comfortable range).
        let (low, high) = self.comfort;
        let tone = if life.felt < low {
            [0.6, 0.8, 1.0, 1.0]
        } else if life.felt > high {
            [1.0, 0.6, 0.45, 1.0]
        } else {
            PAPER
        };
        let at = top + Vec2::new(6.0 * (GAUGE.x + 2.0) + 4.0, -1.0);
        self.shadowed(air, texture, at, tone, order);
        let mut pen = top + Vec2::new(0.0, GAUGE.y + 2.0);
        for status in statuses {
            self.shadowed(status, texture, pen, [1.0, 0.85, 0.6, 1.0], order);
            pen.y += status.size.y;
        }
    }

    /// The hotbar along the bottom: what each slot holds and how many, the worn one outlined.
    fn draw_hotbar(
        &mut self,
        life: &LifeView,
        counts: &[Option<TextLayout>],
        texture: TextureId,
        view_min: Vec2,
        view_size: Vec2,
    ) {
        let order = 1e9;
        let width = HOTBAR_SLOTS as f32 * (SLOT + 2.0) - 2.0;
        let left = view_min.x + ((view_size.x - width) / 2.0).round();
        let y = view_min.y + view_size.y - SLOT - 8.0;
        for i in 0..HOTBAR_SLOTS {
            let at = Vec2::new(left + i as f32 * (SLOT + 2.0), y);
            let slot = life.slots.get(i);
            let worn = slot.is_some_and(|(id, _)| life.worn.as_deref() == Some(id.as_str()));
            let border = if worn { NAME } else { INK };
            self.panel(
                at,
                Vec2::splat(SLOT),
                [0.05, 0.05, 0.08, 0.8],
                Some(border),
                order,
            );
            let Some((id, count)) = slot else {
                continue;
            };
            if let Some(&icon) = self.icons.get(id) {
                let inset = ((SLOT - ICON_SIZE as f32) / 2.0).floor();
                let mut s = Sprite::new(
                    icon,
                    Rect::new(0, 0, ICON_SIZE, ICON_SIZE),
                    at + Vec2::splat(inset),
                    Vec2::ZERO,
                );
                s.layer = layer::UI;
                s.sort_y = order + 0.01;
                // Used up: kept in its place, greyed out.
                if *count == 0 {
                    s.color = [0.35, 0.35, 0.35, 0.6];
                }
                self.frame_sprites.push(s);
            }
            if let Some(Some(n)) = counts.get(i) {
                // The digits' ink sits in the top of their line: this puts them in the corner.
                let pen = at + Vec2::new(SLOT - n.size.x + 1.0, SLOT - 16.0);
                self.shadowed(n, texture, pen, PAPER, order + 0.02);
            }
        }
    }

    /// Text with a one-pixel dark shadow, readable over anything.
    fn shadowed(
        &mut self,
        text: &TextLayout,
        texture: TextureId,
        at: Vec2,
        ink: [f32; 4],
        order: f32,
    ) {
        self.frame_sprites.extend(TextSystem::sprites(
            text,
            texture,
            at + Vec2::ONE,
            [0.0, 0.0, 0.0, 0.8],
            layer::UI,
            order + 0.01,
        ));
        self.frame_sprites.extend(TextSystem::sprites(
            text,
            texture,
            at,
            ink,
            layer::UI,
            order + 0.02,
        ));
    }

    /// A skeletal character: its skeleton posed at the replicated clip and frame (one frame is one
    /// tick in a baked clip), as meshes standing at `feet`.
    fn draw_skeleton(
        &mut self,
        c: &DrawCharacter,
        skeleton: usize,
        feet: Vec2,
        sort_y: f32,
        tint: [f32; 4],
    ) {
        let sheet = sheet_of(&c.state, &self.sheets);
        let Some(clip) = sheet.clips.get(usize::from(c.state.anim.clip().0)) else {
            return;
        };
        let view = &self.skeletons[skeleton];
        let Some((animation, looping)) = view.clips.get(&clip.name) else {
            return;
        };
        let time = c.state.anim.step() as f32 / 60.0;
        let entry = self
            .poses
            .entry(c.id)
            .or_insert_with(|| (skeleton, Pose::new(&view.rig)));
        if entry.0 != skeleton {
            *entry = (skeleton, Pose::new(&view.rig));
        }
        let pose = &mut entry.1;
        pose.pose(animation, *looping, time);
        for mesh in pose.meshes(&view.rig) {
            let vertices = mesh
                .vertices
                .iter()
                .map(|v| MeshVertex {
                    position: feet + v.position,
                    uv: v.uv,
                    color: [
                        v.color[0] * tint[0],
                        v.color[1] * tint[1],
                        v.color[2] * tint[2],
                        v.color[3] * tint[3],
                    ],
                })
                .collect();
            self.frame_meshes.push(Mesh {
                texture: view.pages[mesh.page],
                vertices,
                layer: layer::WORLD,
                sort_y,
                body: true,
            });
        }
    }

    /// A fade to black and back when the count of fades goes up: a second down, a second dark,
    /// a second up. What happens in the dark is never shown.
    fn draw_fade(&mut self, faded: u32, (view_min, view_size): (Vec2, Vec2), dt: f32) {
        match self.faded {
            Some(seen) if faded > seen => self.fade_left = 3.0,
            _ => {}
        }
        self.faded = Some(faded);
        if self.fade_left <= 0.0 {
            return;
        }
        self.fade_left = (self.fade_left - dt).max(0.0);
        let t = 3.0 - self.fade_left;
        let alpha = if t < 1.0 {
            t
        } else if t < 2.0 {
            1.0
        } else {
            3.0 - t
        };
        self.rect(view_min, view_size, [0.0, 0.0, 0.0, alpha], 4e9);
    }

    /// A small white burst where each recent hit landed.
    fn draw_sparks(&mut self) {
        let sparks: Vec<(Vec2, f32)> = self
            .fx
            .sparks
            .iter()
            .map(|s| (s.at - Vec2::new(0.0, s.lift), s.left))
            .collect();
        for (at, left) in sparks {
            let arm = (left * 60.0).round().clamp(1.0, 5.0);
            let white = [1.0, 1.0, 0.9, 1.0];
            let order = at.y + 1e6;
            self.rect(
                at - Vec2::new(arm, 0.0),
                Vec2::new(arm * 2.0 + 1.0, 1.0),
                white,
                order,
            );
            self.rect(
                at - Vec2::new(0.0, arm),
                Vec2::new(1.0, arm * 2.0 + 1.0),
                white,
                order,
            );
            self.rect(at - Vec2::ONE, Vec2::splat(3.0), white, order);
        }
    }

    /// Bars over enemies and over anyone else who is hurt; the local player's is in the corner.
    fn draw_health(&mut self, characters: &[DrawCharacter], dt: f32) {
        let sheets = &self.sheets;
        self.trails
            .update(characters, |c| sheets.look(c.state.look).moveset.health, dt);
        for c in characters {
            let moveset = &self.sheets.look(c.state.look).moveset;
            let max = moveset.health.max(1);
            let health = c.state.fighter.health;
            // Over every fighter's head: enemies always, friends (you too) while hurt or just hit.
            let fighter = c.hostile || !c.npc || !moveset.combo.is_empty();
            let showing = c.hostile || health < max || self.trails.recently_hit(c.id);
            if !fighter || !showing || c.state.fighter.is_dead() {
                continue;
            }
            let trail = self.trails.trail(c.id).unwrap_or(f32::from(health));
            let head = c.ground
                - Vec2::new(
                    0.0,
                    c.elevation + head_height(&self.looks, &self.skeletons, c) - 6.0,
                );
            let bar = Bar {
                health,
                trail,
                max,
                hostile: c.hostile,
            };
            self.bar(head - Vec2::new(10.0, 0.0), 20.0, bar, c.ground.y);
        }
    }

    /// A health bar `width` wide at `top_left`: red for enemies, green for friends, with the
    /// health just lost showing pale until it drains away.
    fn bar(&mut self, top_left: Vec2, width: f32, bar: Bar, order: f32) {
        let inner = width - 2.0;
        let fill = inner * f32::from(bar.health) / f32::from(bar.max);
        let trail = (inner * bar.trail / f32::from(bar.max)).max(fill);
        let colour = if bar.hostile {
            [0.85, 0.15, 0.1, 1.0]
        } else {
            [0.3, 0.85, 0.3, 1.0]
        };
        self.rect(top_left.round(), Vec2::new(width, 4.0), INK, order);
        if trail.round() > fill.round() {
            self.rect(
                top_left.round() + Vec2::ONE,
                Vec2::new(trail.round(), 2.0),
                TRAIL,
                order + 0.0005,
            );
        }
        self.rect(
            top_left.round() + Vec2::ONE,
            Vec2::new(fill.round(), 2.0),
            colour,
            order + 0.001,
        );
    }

    /// A box with its corner pixels cut, optionally outlined.
    fn panel(
        &mut self,
        top_left: Vec2,
        size: Vec2,
        fill: [f32; 4],
        border: Option<[f32; 4]>,
        order: f32,
    ) {
        if let Some(border) = border {
            self.rect(
                top_left - Vec2::X,
                size + Vec2::new(2.0, 0.0),
                border,
                order,
            );
            self.rect(
                top_left - Vec2::Y,
                size + Vec2::new(0.0, 2.0),
                border,
                order,
            );
        }
        self.rect(top_left, size, fill, order + 0.001);
    }

    fn rect(&mut self, top_left: Vec2, size: Vec2, color: [f32; 4], order: f32) {
        let mut s = Sprite::fill(self.white, top_left, size, color);
        s.layer = layer::UI;
        s.sort_y = order;
        self.frame_sprites.push(s);
    }
}

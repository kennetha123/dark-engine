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
use dark_render::{
    Mesh, MeshVertex, Outline, RenderError, Renderer, Sprite, SpriteKind, TextLayout, TextSystem,
    TextureId, layer,
};
use dark_spine::{Pose, Rig};
use dark_sprite::{Rect, SpriteSheet};
use dark_view::{MapView, south_edge};
use dark_world::{
    BodyState, CharacterSheets, CharacterState, CharactersPlugin, CombatPlugin, Dormant,
    DrawCharacter, Hostile, LifePlugin, LifeView, MapId, Maps, MapsPlugin, NetHost, NetId, Npc,
    PlayerAvatar, PreviousBody, ReplicationPlugin, SPEECH_TICKS, Speech, StructureSnapshot,
    TalkPlugin, sheet_of, talk_target,
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
    /// A 3D model to stand in for the player, for looking at one in the world. Set
    /// `DARK_MODEL` to a `*.model.ron` (journals/engine/04, phase 4b). Nothing in the game reads
    /// a model as a character's look yet; this is how one is looked at until it does.
    stand_in: Option<StandIn>,
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
    /// Items lying on the ground in `map`.
    pub drops: &'a [dark_world::DropSnapshot],
    /// The others in the local player's party.
    pub party: &'a [NetId],
    /// The local player's conversation choices, fades and the year's ending.
    pub story: &'a dark_world::StoryView,
    /// The Esc menu, while it is open.
    pub menu: Option<Menu>,
    /// The menu's line about leaving for the title screen (§22): the string it reads — saving
    /// the year in a game of one's own, simply going in someone else's, or that the goodbye is
    /// already on its way — and whether Q still does anything. None in a game the command line
    /// chose, which the player did not come to the title from.
    pub leaving: Option<(&'static str, bool)>,
}

/// The Esc menu: whether the world waits, where the player stands and who feels for them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    /// Nobody else is online: the world stands still.
    Paused,
    /// Others are playing, so the world goes on.
    OthersPlaying,
}

/// A model standing in for a character: its geometry, the clip it plays, and its pictures once
/// they are on the GPU.
struct StandIn {
    model: dark_model::Model,
    pose: dark_model::Pose,
    animation: String,
    looping: bool,
    /// Each part ready to draw: the renderer's vertex layout, its indices, its picture and
    /// whether it is drawn from both faces. Built once — skinning happens in the shader, so
    /// nothing here changes from frame to frame.
    parts: Vec<StandInPart>,
}

struct StandInPart {
    vertices: Vec<dark_render::ModelVertex>,
    indices: Vec<u32>,
    texture: TextureId,
    double_sided: bool,
}

/// Loads what `DARK_MODEL` points at, if anything. A model that will not load is said about and
/// then let go: it is a way of looking at something, not part of the game.
fn load_stand_in(project: &Project) -> Option<StandIn> {
    let path = std::env::var("DARK_MODEL").ok()?;
    let loaded = project
        .load_model(&path)
        .inspect_err(|err| tracing::error!("DARK_MODEL={path}: {err}"))
        .ok()?;
    let model = dark_model::Model::load(project, &loaded.def)
        .inspect_err(|err| tracing::error!("DARK_MODEL={path}: {err}"))
        .ok()?;
    // Whatever it calls standing still, else the first clip it has.
    let (name, clip) = loaded
        .bake
        .clips
        .get_key_value("idle")
        .or_else(|| loaded.bake.clips.iter().next())?;
    tracing::info!(
        "{path}: standing in for the player, playing {name} ({} bones)",
        model.skeleton.bones.len()
    );
    Some(StandIn {
        pose: dark_model::Pose::new(&model),
        model,
        animation: clip.animation.clone(),
        looping: clip.looping,
        parts: Vec::new(),
    })
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
            // The pictures a scene's scatter groups use. On a drawn map they are in the list of
            // props already; on a made one what grows is never in any list — it is worked out
            // from the seed as the land is made (docs/PLAN.md §24.5) — so they are asked for
            // here, or a wood would grow with nothing to draw it.
            paths.extend(map.def.scatter.iter().map(|group| group.sheet.clone()));
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
            stand_in: load_stand_in(project),
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
        let stand_in = match self.stand_in {
            Some(mut stand) => {
                let mut textures = Vec::new();
                for (nth, image) in stand.model.images.iter().enumerate() {
                    textures.push(renderer.create_texture_smooth(
                        &format!("model image {nth}"),
                        image.width,
                        image.height,
                        &image.rgba,
                    )?);
                }
                stand.parts = stand
                    .model
                    .parts
                    .iter()
                    .map(|part| StandInPart {
                        vertices: part
                            .vertices
                            .iter()
                            .map(|v| dark_render::ModelVertex {
                                position: v.position.to_array(),
                                normal: v.normal.to_array(),
                                uv: v.uv,
                                joints: v.joints,
                                weights: v.weights.to_array(),
                            })
                            .collect(),
                        indices: part.indices.clone(),
                        texture: part
                            .image
                            .and_then(|i| textures.get(i).copied())
                            .unwrap_or(white),
                        double_sided: part.double_sided,
                    })
                    .collect();
                Some(stand)
            }
            None => None,
        };
        Ok(DemoView {
            stand_in,
            looks,
            sheets: self.characters,
            maps: views,
            map_sheets: self.sheets,
            map_textures: textures,
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
            money: self.life.money.clone(),
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
    Vec<dark_world::DropSnapshot>,
    Vec<NetId>,
    dark_world::StoryView,
) {
    let me = app.world.resource::<NetHost>().0.local_player();
    let life = me.and_then(|me| dark_world::life_of(&mut app.world, me));
    let structures = dark_world::structures_in(&mut app.world, map);
    let drops = dark_world::drops_in(&mut app.world, map);
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
    (life, structures, drops, party, story)
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
/// Night behind the title screen: the world is not drawn there.
const TITLE_SKY: [f64; 3] = [0.006, 0.006, 0.012];
/// Between the title screen's heading and its list.
const MENU_GAP: f32 = 12.0;
/// The Esc menu's lines wrap at this width.
const MENU_WIDTH: f32 = 220.0;
/// People listed in the Esc menu: those who feel most strongly, the warmest first.
const MAX_FEELINGS: usize = 8;

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

/// Where each part of the interface over the world sits, back to front. They are far enough
/// apart that a part's own pieces (a bar's trail and fill, a gauge's ink and filling) can settle
/// themselves between two of them, and far enough from the world that nothing standing in it can
/// reach them (docs/PLAN.md §24.0).
mod over {
    /// The day and the hour, top right.
    pub const CLOCK: f32 = 3.00e9;
    /// The local player's health, top left.
    pub const HEALTH: f32 = 3.01e9;
    /// What the body needs, under the health.
    pub const BODY: f32 = 3.02e9;
    /// Who is travelling with the player, and how they are faring.
    pub const PARTY: f32 = 3.03e9;
    /// The language, shown for a moment after it changes.
    pub const LANGUAGE: f32 = 3.04e9;
}

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
    /// See [`DemoScene::stand_in`].
    stand_in: Option<StandIn>,
    looks: Vec<LookView>,
    sheets: CharacterSheets,
    maps: Vec<MapView>,
    /// The pictures a map's own things are drawn with, kept because a piece of a map is made
    /// when the camera reaches it (docs/PLAN.md §24.4).
    map_sheets: HashMap<String, LoadedSheet>,
    map_textures: HashMap<String, TextureId>,
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
    /// The item that is money, counted beside the hotbar (the project's `life.ron`).
    money: Option<String>,
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
    /// Reads on in the project's next language, and says which that is.
    pub fn cycle_language(&mut self) -> String {
        self.strings.cycle();
        self.language_shown = LANGUAGE_BANNER_SECS;
        tracing::info!("language: {}", self.strings.language());
        self.strings.language().to_owned()
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
    /// What a character stands on: a prop underfoot, a ledge, or the ground. The camera rides
    /// it and the blob shadow lies on it.
    fn surface_under(
        &self,
        c: &DrawCharacter,
        collision: &dark_physics::World,
        maps: &dark_world::Maps,
    ) -> f32 {
        let surface = collision.support(c.ground, c.body.radius, c.elevation, &maps.params);
        if surface.is_finite() {
            surface.min(c.elevation)
        } else {
            0.0
        }
    }

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
        self.draw_drops(frame.drops);
        let map_view = map.0 as usize;

        // The camera is worked out first, because what it can see decides which pieces of the
        // map are worth copying at all (docs/PLAN.md §24.2). It follows what the local character
        // stands on, eased, so a jump reads as rising and landing on a ledge does not snap the
        // view. A new map snaps it.
        let (w, h) = renderer.internal_size();
        let half = Vec2::new(w as f32, h as f32) / 2.0;
        let you = characters.iter().find(|c| c.you);
        let focus_point = match you {
            Some(c) => {
                let surface = self.surface_under(c, collision, maps);
                if self.camera_map != Some(map) {
                    self.camera_map = Some(map);
                    self.camera_floor = surface;
                } else if c.body.grounded {
                    self.camera_floor +=
                        (c.body.elevation - self.camera_floor) * (1.0 - (-12.0 * dt).exp());
                }
                c.ground - Vec2::new(0.0, self.camera_floor)
            }
            None => self.maps[map_view].size / 2.0,
        };
        let size = self.maps[map_view].size;
        let camera = focus_point.clamp(half, (size - half).max(half)) + self.fx.shake();
        self.camera = camera;
        let (seen_min, seen_max) = (camera - half, camera + half);
        {
            // Only while the map's pieces are being asked for: what they are made from is kept
            // on the view, and everything else here wants the view itself.
            let scenery = dark_view::Scenery {
                map: maps.get(map),
                sheets: &self.map_sheets,
                textures: &self.map_textures,
            };
            self.maps[map_view].seen(&scenery, seen_min, seen_max, &mut self.frame_sprites);
        }

        for c in characters {
            // What the character stands on places its blob shadow, and a prop underfoot must draw
            // before it even where the feet are north of the prop's own pivot.
            let surface = self.surface_under(c, collision, maps);
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
            // Just behind the feet it belongs to. A sub-layer rather than a hair off `sort_y`,
            // which an f32 stops being able to hold far from the origin (docs/PLAN.md §24.0).
            blob.sort_y = sort_y;
            blob.sub = -1;
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
        }
        if self.debug {
            let scenery = dark_view::Scenery {
                map: maps.get(map),
                sheets: &self.map_sheets,
                textures: &self.map_textures,
            };
            self.maps[map_view].seen_overlay(
                &scenery,
                seen_min,
                seen_max,
                &mut self.frame_sprites,
                |mut s| {
                    s.layer = layer::DEBUG;
                    s
                },
            );
        }
        self.draw_sparks();
        self.draw_health(characters, dt);
        let screen = (camera - half, half * 2.0);
        self.draw_interface(renderer, characters, frame, screen, dt);
        self.draw_fade(frame.story.faded, screen, dt);
        // Skeletons of those no longer here are dropped.
        self.poses
            .retain(|id, _| characters.iter().any(|c| c.id == *id));

        // A model standing in for the player, if one was asked for. Its palette carries where it
        // stands, so the shader multiplies one matrix a vertex (docs/PLAN.md §16.3).
        let mut model_draws = Vec::new();
        let mut palette = Vec::new();
        if let Some(stand) = &mut self.stand_in
            && let Some(you) = characters.iter().find(|c| c.you)
        {
            stand
                .pose
                .pose(&stand.model, &stand.animation, stand.looping, self.seconds);
            let place = dark_view::stand_at(you.ground, you.body.elevation, stand.model.scale);
            palette.extend(stand.pose.palette().iter().map(|bone| place * *bone));
            for part in &stand.parts {
                model_draws.push(dark_render::ModelDraw {
                    vertices: &part.vertices,
                    indices: &part.indices,
                    palette: &palette,
                    texture: part.texture,
                    double_sided: part.double_sided,
                    layer: layer::WORLD,
                    // Where a character sorts: by their feet, as their sprite would.
                    sort_y: you.ground.y,
                });
            }
        }
        renderer.render_scene(dark_render::Scene {
            camera,
            clear: [0.0; 3],
            sprites: &mut self.frame_sprites,
            meshes: &self.frame_meshes,
            models: &model_draws,
            model_camera: dark_view::model_camera(camera, renderer.internal_size()),
            light: dark_render::Light::default(),
        });
    }

    /// The project's string table, in the language showing (the title screen reads it too).
    pub fn strings(&self) -> &Localization {
        &self.strings
    }

    /// A screen of choosing — the title screen (docs/PLAN.md §22) — over an empty background:
    /// a heading, a list, and a mark against the one picked.
    /// `note` is a line under the list: what went wrong when something did.
    pub fn draw_menu(
        &mut self,
        renderer: &mut Renderer,
        heading: &str,
        items: &[String],
        picked: usize,
        note: Option<&str>,
    ) {
        self.frame_sprites.clear();
        self.frame_meshes.clear();
        let (w, h) = renderer.internal_size();
        let size = Vec2::new(w as f32, h as f32);
        let Some(text) = &mut self.text else {
            renderer.render(size / 2.0, TITLE_SKY, &mut []);
            return;
        };
        let mut layout = |s: &str| text.layout(s, size.x - 4.0 * WINDOW_MARGIN);
        let heading = layout(heading);
        // A long list (many saved games) scrolls: the ones around the one picked are shown, so
        // the cursor is always on the screen.
        // A note is laid out first and its room taken off the top, so a long list scrolls
        // instead of pushing the note off the bottom, however many lines the note itself runs to.
        let note = note.map(&mut layout);
        let spare = note.as_ref().map_or(0.0, |note| note.size.y + MENU_GAP);
        let room = (((size.y - heading.size.y - spare) / heading.size.y.max(1.0)).floor() as usize)
            .saturating_sub(2)
            .max(1);
        let first = picked
            .saturating_sub(room / 2)
            .min(items.len().saturating_sub(room));
        let shown = items.iter().enumerate().skip(first).take(room);
        let lines: Vec<TextLayout> = shown
            // A plain arrow: the project's font is a pixel font, and not every one has the
            // pointing triangles.
            .map(|(i, item)| layout(&format!("{} {item}", if i == picked { ">" } else { " " })))
            .collect();
        let texture = match text.texture(renderer) {
            Ok(texture) => texture,
            Err(err) => {
                tracing::error!("cannot upload glyphs: {err}");
                return;
            }
        };
        let line_height = lines.first().map_or(0.0, |l| l.size.y);
        let block = heading.size.y
            + MENU_GAP
            + line_height * lines.len() as f32
            + note.as_ref().map_or(0.0, |n| MENU_GAP + n.size.y);
        let mut pen = Vec2::new(0.0, ((size.y - block) / 2.0).max(MENU_GAP)).round();
        let centre =
            |line: &TextLayout, pen: Vec2| Vec2::new(((size.x - line.size.x) / 2.0).round(), pen.y);
        self.frame_sprites.extend(TextSystem::sprites(
            &heading,
            texture,
            centre(&heading, pen),
            NAME,
            layer::UI,
            1.0,
        ));
        pen.y += heading.size.y + MENU_GAP;
        // The list is left-aligned as a block, so the marker does not shift the words.
        let widest = lines.iter().map(|l| l.size.x).fold(0.0, f32::max);
        let left = ((size.x - widest) / 2.0).round();
        for (i, line) in lines.iter().enumerate() {
            let ink = if first + i == picked { NAME } else { PAPER };
            self.frame_sprites.extend(TextSystem::sprites(
                line,
                texture,
                Vec2::new(left, pen.y),
                ink,
                layer::UI,
                2.0,
            ));
            pen.y += line.size.y;
        }
        if let Some(note) = &note {
            pen.y += MENU_GAP;
            self.frame_sprites.extend(TextSystem::sprites(
                note,
                texture,
                centre(note, pen),
                NAME,
                layer::UI,
                2.0,
            ));
        }
        renderer.render_with(size / 2.0, TITLE_SKY, &mut self.frame_sprites, &[]);
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
            // A sleeper is not talked to (§21), so no prompt offers it.
            let npcs = characters
                .iter()
                .filter(|c| c.npc && !c.state.sleeping)
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
        // What the player has to spend, beside the hotbar: the money item's own icon and count.
        let purse = life.zip(self.money.as_ref()).map(|(life, money)| {
            let count = life
                .slots
                .iter()
                .find(|(item, _)| item == money)
                .map_or(0, |(_, n)| *n);
            (money.clone(), layout(&count.to_string(), BUBBLE_WIDTH))
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
        // "Press E to go on", in the window's corner: on a conversation's held line (not a
        // remark in passing), while no choices wait for a number key.
        let held = dialogue
            .as_ref()
            .and_then(|d| d.speech.as_ref())
            .is_some_and(|s| s.ticks_left > SPEECH_TICKS);
        let more = (held && choices.is_empty()).then(|| layout("E ▼", BUBBLE_WIDTH));
        let ending = frame
            .story
            .ending
            .as_ref()
            .map(|key| layout(self.strings.text(key), ENDING_WIDTH));
        let menu: Vec<(TextLayout, [f32; 4])> = frame
            .menu
            .map(|menu| {
                let title = match menu {
                    Menu::Paused => "ui.paused",
                    Menu::OthersPlaying => "ui.others_playing",
                };
                let mut lines = vec![(layout(self.strings.text(title), MENU_WIDTH), NAME)];
                let mut section = |header: &str, entries: &[(String, i32)]| {
                    if entries.is_empty() {
                        return;
                    }
                    lines.push((layout(self.strings.text(header), MENU_WIDTH), NAME));
                    for (name, value) in entries {
                        let line = format!("  {}  {value:+}", self.strings.text(name));
                        lines.push((layout(&line, MENU_WIDTH), PAPER));
                    }
                };
                section("ui.standing", &frame.story.standing);
                // The strongest feelings, warm or cold, then the warmest first.
                let mut feelings = frame.story.feelings.clone();
                feelings.sort_by_key(|(_, felt)| std::cmp::Reverse(felt.abs()));
                feelings.truncate(MAX_FEELINGS);
                feelings.sort_by_key(|(_, felt)| std::cmp::Reverse(*felt));
                section("ui.feelings", &feelings);
                let resume = format!("Esc  {}", self.strings.text("ui.resume"));
                lines.push((layout(&resume, MENU_WIDTH), NAME));
                if let Some((key, takes)) = frame.leaving {
                    let said = self.strings.text(key);
                    // No key is offered for what is already happening.
                    let leave = if takes {
                        format!("Q  {said}")
                    } else {
                        said.to_owned()
                    };
                    lines.push((layout(&leave, MENU_WIDTH), NAME));
                }
                lines
            })
            .unwrap_or_default();
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
            self.panel(at, size, [0.03, 0.03, 0.07, 0.88], Some(PAPER), order, 0);
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
            if let Some(more) = &more {
                let corner = at + size - more.size - Vec2::new(WINDOW_PAD, WINDOW_PAD - 2.0);
                self.frame_sprites.extend(TextSystem::sprites(
                    more,
                    texture,
                    corner,
                    NAME,
                    layer::UI,
                    order + 0.02,
                ));
            }
        }
        if let Some(clock) = clock {
            let at = view_min + Vec2::new(view_size.x - clock.size.x - 8.0, 6.0);
            let pad = Vec2::new(4.0, 1.0);
            self.panel(
                at - pad,
                clock.size + pad * 2.0,
                [0.0, 0.0, 0.0, 0.6],
                None,
                over::CLOCK,
                0,
            );
            self.frame_sprites.extend(TextSystem::sprites(
                &clock,
                texture,
                at,
                PAPER,
                layer::UI,
                over::CLOCK,
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
            self.bar(view_min + Vec2::new(8.0, 8.0), 80.0, bar, over::HEALTH, 0);
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
                0,
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
            self.panel(at, size, [0.02, 0.02, 0.04, 0.94], Some(NAME), 3.5e9, 0);
            self.frame_sprites.extend(TextSystem::sprites(
                ending,
                texture,
                at + pad,
                PAPER,
                layer::UI,
                3.5e9 + 0.01,
            ));
        }
        if !menu.is_empty() {
            let pad = Vec2::splat(10.0);
            let width = menu.iter().map(|(l, _)| l.size.x).fold(0.0, f32::max);
            let height: f32 = menu.iter().map(|(l, _)| l.size.y).sum();
            let size = Vec2::new(width, height) + pad * 2.0;
            let at = (view_min + (view_size - size) / 2.0).round();
            self.panel(at, size, [0.02, 0.02, 0.04, 0.92], Some(NAME), 3.6e9, 0);
            let mut pen = at + pad;
            for (line, colour) in &menu {
                self.frame_sprites.extend(TextSystem::sprites(
                    line,
                    texture,
                    pen,
                    *colour,
                    layer::UI,
                    3.6e9 + 0.01,
                ));
                pen.y += line.size.y;
            }
        }
        let mut pen = view_min + Vec2::new(view_size.x - 8.0, 24.0);
        for (name, bar) in &party {
            let at = Vec2::new(pen.x - name.size.x.max(PARTY_BAR), pen.y);
            self.shadowed(name, texture, at, PAPER, over::PARTY);
            self.bar(
                at + Vec2::new(0.0, name.size.y - 2.0),
                PARTY_BAR,
                *bar,
                over::PARTY,
                2,
            );
            pen.y += name.size.y + 6.0;
        }
        if let Some((life, air, statuses, counts)) = body {
            self.draw_body(life, &air, &statuses, texture, view_min);
            self.draw_hotbar(life, &counts, texture, view_min, view_size);
            if let Some((money, count)) = &purse {
                self.draw_purse(money, count, texture, view_min, view_size);
            }
        }
        if let Some(banner) = banner {
            let at = view_min + Vec2::new(8.0, 18.0);
            let pad = Vec2::new(4.0, 1.0);
            self.panel(
                at - pad,
                banner.size + pad * 2.0,
                [0.0, 0.0, 0.0, 0.6],
                None,
                over::LANGUAGE,
                0,
            );
            self.frame_sprites.extend(TextSystem::sprites(
                &banner,
                texture,
                at,
                PAPER,
                layer::UI,
                over::LANGUAGE,
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
        self.panel(top_left, size, fill, border, order, 0);
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
                    0,
                );
                self.rect(
                    Vec2::new(x - half, y),
                    Vec2::new(2.0 * half + 1.0, 1.0),
                    fill,
                    order,
                    0,
                );
            }
        }
        self.frame_sprites.extend(
            TextSystem::sprites(said, texture, top_left + pad, ink, layer::UI, order).map(
                |mut s| {
                    s.sub = 1;
                    s
                },
            ),
        );
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

    /// Items lying on the ground: the icon the hotbar uses, laid where it fell. An item with no
    /// icon (nothing draws it) is invisible, as it is in the pack.
    fn draw_drops(&mut self, drops: &[dark_world::DropSnapshot]) {
        for drop in drops {
            let Some(&icon) = self.icons.get(&drop.item) else {
                continue;
            };
            let at = Vec2::new(drop.at.0, drop.at.1);
            let mut sprite = Sprite::new(
                icon,
                Rect::new(0, 0, ICON_SIZE, ICON_SIZE),
                at - Vec2::new(0.0, drop.elevation),
                // Standing on its own middle, so it lies on the ground it was dropped on.
                Vec2::splat(ICON_SIZE as f32 / 2.0),
            );
            // Sorted by the icon's lowest pixel, as everything else is by its feet.
            sprite.sort_y = at.y + ICON_SIZE as f32 / 2.0;
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
        let order = over::BODY;
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
            self.rect(at, GAUGE, INK, order, 0);
            self.rect(at + Vec2::ONE, inner, [0.18, 0.18, 0.22, 1.0], order, 1);
            self.rect(
                at + Vec2::new(1.0, 1.0 + inner.y - filled),
                Vec2::new(inner.x, filled),
                colour,
                order,
                2,
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

    /// What the player has to spend (docs/PLAN.md §14): the money item's icon and how many of
    /// it, left of the hotbar.
    fn draw_purse(
        &mut self,
        money: &str,
        count: &TextLayout,
        texture: TextureId,
        view_min: Vec2,
        view_size: Vec2,
    ) {
        let order = 1e9;
        let width = HOTBAR_SLOTS as f32 * (SLOT + 2.0) - 2.0;
        let left = view_min.x + ((view_size.x - width) / 2.0).round();
        let at = Vec2::new(
            left - SLOT - 10.0 - count.size.x,
            view_min.y + view_size.y - SLOT - 4.0,
        );
        if let Some(&icon) = self.icons.get(money) {
            let mut s = Sprite::new(icon, Rect::new(0, 0, ICON_SIZE, ICON_SIZE), at, Vec2::ZERO);
            s.layer = layer::UI;
            s.sort_y = order;
            self.frame_sprites.push(s);
        }
        let beside = at + Vec2::new(ICON_SIZE as f32 + 3.0, 0.0);
        self.shadowed(count, texture, beside, PAPER, order + 0.01);
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
                0,
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
        self.rect(view_min, view_size, [0.0, 0.0, 0.0, alpha], 4e9, 0);
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
                0,
            );
            self.rect(
                at - Vec2::new(0.0, arm),
                Vec2::new(1.0, arm * 2.0 + 1.0),
                white,
                order,
                0,
            );
            self.rect(at - Vec2::ONE, Vec2::splat(3.0), white, order, 0);
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
            self.bar(head - Vec2::new(10.0, 0.0), 20.0, bar, c.ground.y, 0);
        }
    }

    /// A health bar `width` wide at `top_left`: red for enemies, green for friends, with the
    /// health just lost showing pale until it drains away.
    fn bar(&mut self, top_left: Vec2, width: f32, bar: Bar, order: f32, sub: i16) {
        let inner = width - 2.0;
        let fill = inner * f32::from(bar.health) / f32::from(bar.max);
        let trail = (inner * bar.trail / f32::from(bar.max)).max(fill);
        let colour = if bar.hostile {
            [0.85, 0.15, 0.1, 1.0]
        } else {
            [0.3, 0.85, 0.3, 1.0]
        };
        self.rect(top_left.round(), Vec2::new(width, 4.0), INK, order, sub);
        if trail.round() > fill.round() {
            self.rect(
                top_left.round() + Vec2::ONE,
                Vec2::new(trail.round(), 2.0),
                TRAIL,
                order,
                sub + 1,
            );
        }
        self.rect(
            top_left.round() + Vec2::ONE,
            Vec2::new(fill.round(), 2.0),
            colour,
            order,
            sub + 2,
        );
    }

    /// A box with its corner pixels cut, optionally outlined. Its border and face are both at
    /// `sub`, one after the other: they are drawn together, so the order they are given in
    /// settles them, and the level is for saying where the whole panel sits against things drawn
    /// elsewhere in the frame.
    fn panel(
        &mut self,
        top_left: Vec2,
        size: Vec2,
        fill: [f32; 4],
        border: Option<[f32; 4]>,
        order: f32,
        sub: i16,
    ) {
        if let Some(border) = border {
            self.rect(
                top_left - Vec2::X,
                size + Vec2::new(2.0, 0.0),
                border,
                order,
                sub,
            );
            self.rect(
                top_left - Vec2::Y,
                size + Vec2::new(0.0, 2.0),
                border,
                order,
                sub,
            );
        }
        self.rect(top_left, size, fill, order, sub);
    }

    /// A flat rectangle of the interface. `sub` settles ties with everything else drawn at the
    /// same `order`: larger is nearer the front. A whole number, because much of the interface
    /// is ordered by a character's world y, and an `f32` cannot hold a thousandth off a large
    /// one — over a kilometre from the origin the nudges this replaced were already lost
    /// (docs/PLAN.md §24.0).
    fn rect(&mut self, top_left: Vec2, size: Vec2, color: [f32; 4], order: f32, sub: i16) {
        let mut s = Sprite::fill(self.white, top_left, size, color);
        s.layer = layer::UI;
        s.sort_y = order;
        s.sub = sub;
        self.frame_sprites.push(s);
    }
}

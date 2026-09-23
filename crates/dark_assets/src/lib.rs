//! Game project files: `project.ron`, images and sprite sheet definitions.
//!
//! No GPU: the host loads sheets too, because automatic slicing needs pixels and hitboxes
//! (M4) need frames. Paths inside definitions are relative to the project root.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dark_physics::{Cell, Shape, Terrain};
use dark_sprite::{AutoSlice, CharacterLayout, Clip, Facing, Frame, Pivot, Rect, SpriteSheet};
use serde::{Deserialize, Serialize};

mod spine;
pub use spine::{BakedClip, SpineBake, SpineDef, SpineSheet, skeleton_hash};

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Image {
        path: PathBuf,
        source: Box<image::ImageError>,
    },
    #[error("{path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<ron::error::SpannedError>,
    },
    #[error("{path}: {message}")]
    Invalid { path: PathBuf, message: String },
}

pub const TILE_SIZE_RANGE: std::ops::RangeInclusive<u32> = 4..=1024;

/// The scene a game starts in when neither the project nor the command line says another.
pub const DEFAULT_SCENE: &str = "scenes/meadow.ron";

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ProjectSettings {
    pub name: String,
    /// Grid size in pixels, 4–1024 (docs/PLAN.md §1).
    pub tile_size: u32,
    /// Internal render resolution; the window shows it scaled by whole numbers.
    pub resolution: (u32, u32),
    /// Languages with a `locale/<code>.ron` string table; the first is the default and the
    /// fallback for missing strings.
    #[serde(default = "default_languages")]
    pub languages: Vec<String>,
    /// Scene the game starts in, unless `--scene` says otherwise; `scenes/meadow.ron` when unset.
    #[serde(default)]
    pub start_scene: Option<String>,
    /// Font for in-game text; one that covers every language's characters.
    #[serde(default)]
    pub font: Option<FontDef>,
    /// FMOD Studio: the runtime library per platform and the built banks. No sound if unset.
    #[serde(default)]
    pub audio: Option<AudioDef>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct AudioDef {
    /// The FMOD Studio runtime library, by platform (`windows`, `linux`), relative to the
    /// project. `DARK_FMOD_LIB` overrides it.
    pub library: std::collections::BTreeMap<String, String>,
    /// Built banks in load order (master and strings first), relative to the project.
    pub banks: Vec<String>,
    /// The runtime library's version, e.g. "2.02.30": banks must come from a Studio of the
    /// same minor version.
    pub fmod_version: String,
}

fn default_languages() -> Vec<String> {
    vec!["en".into()]
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct FontDef {
    /// Font file (TTF or OTF) relative to the project root.
    pub path: String,
    /// Pixel size; a pixel font stays crisp only at its design size or whole multiples.
    pub size: f32,
    /// Line spacing in pixels; defaults to the size plus a quarter.
    #[serde(default)]
    pub line_height: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct Project {
    root: PathBuf,
    pub settings: ProjectSettings,
}

impl Project {
    pub const FILE: &'static str = "project.ron";

    pub fn open(root: impl Into<PathBuf>) -> Result<Self, AssetError> {
        let root = root.into();
        let path = root.join(Self::FILE);
        let settings: ProjectSettings = read_ron(&path)?;
        if !TILE_SIZE_RANGE.contains(&settings.tile_size) {
            return Err(invalid(
                &path,
                format!("tile_size {} is outside 4..=1024", settings.tile_size),
            ));
        }
        if settings.resolution.0 == 0 || settings.resolution.1 == 0 {
            return Err(invalid(&path, "resolution must be non-zero".into()));
        }
        if settings.languages.is_empty() {
            return Err(invalid(&path, "languages must name at least one".into()));
        }
        Ok(Self { root, settings })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.root.join(relative)
    }

    /// Loads a sheet definition and the image it slices; a `*.spine.ron` sheet loads its baked
    /// clips instead (see [`SpineDef`]).
    pub fn load_sheet(&self, definition: impl AsRef<Path>) -> Result<LoadedSheet, AssetError> {
        let def_path = self.path(definition);
        if def_path.to_string_lossy().ends_with(".spine.ron") {
            return self.load_spine_sheet(&def_path);
        }
        let def: SheetDef = read_ron(&def_path)?;
        let image_path = self.path(&def.image);
        let mut image = load_image(&image_path)?;
        if !(1..=16).contains(&def.downscale) {
            return Err(invalid(
                &def_path,
                format!("downscale {} must be 1..=16", def.downscale),
            ));
        }
        if def.downscale > 1 {
            image = downscale(&image, def.downscale);
        }
        let (sheet, repacked) = def
            .build(&image, &image_path)
            .map_err(|m| invalid(&def_path, m))?;
        let image = repacked.unwrap_or(image);
        Ok(LoadedSheet {
            image_path,
            image,
            sheet,
            spine: None,
        })
    }
}

/// Decoded RGBA8 pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn load_image(path: &Path) -> Result<Image, AssetError> {
    let decoded = image::open(path).map_err(|source| AssetError::Image {
        path: path.to_owned(),
        source: Box::new(source),
    })?;
    let rgba = decoded.into_rgba8();
    Ok(Image {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

/// Shrinks art drawn at `factor`× (every art pixel a `factor`×`factor` block) back to one texel
/// per pixel, taking each block's centre. Extra rows or columns that do not fill a block are cut.
pub fn downscale(image: &Image, factor: u32) -> Image {
    let (width, height) = (image.width / factor, image.height / factor);
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let s =
                ((y * factor + factor / 2) * image.width + x * factor + factor / 2) as usize * 4;
            rgba.extend_from_slice(&image.rgba[s..s + 4]);
        }
    }
    Image {
        width,
        height,
        rgba,
    }
}

pub struct LoadedSheet {
    pub image_path: PathBuf,
    pub image: Image,
    pub sheet: SpriteSheet,
    /// A skeleton instead of an image (`image` is then a transparent pixel).
    pub spine: Option<SpineSheet>,
}

/// A `*.sheet.ron` file.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SheetDef {
    /// Image path relative to the project root.
    pub image: String,
    /// The image is drawn at this many times its pixel size (RPG Maker MZ art is often 3×); it is
    /// shrunk back before slicing, so `cell`s, rects and pivots are in the shrunk image.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub downscale: u32,
    pub slicing: Slicing,
    #[serde(default, skip_serializing_if = "is_default")]
    pub pivot: Pivot,
    /// Extra clips; character slicing already provides `walk_<dir>` and `idle_<dir>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<ClipDef>,
    /// Where a clip's frames hit and can be hit, by clip name: combat uses these instead of
    /// the moveset's circle (docs/PLAN.md §13), as it uses a skeleton's baked boxes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub boxes: BTreeMap<String, ClipBoxes>,
}

/// One clip's circles, frame by frame: `(x, y, radius)` on the ground plane from the feet, or
/// none on a frame without one. A clip with no hitboxes listed uses its moveset's; one with no
/// hurtboxes, the body's footprint.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct ClipBoxes {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hitboxes: Vec<Option<(f32, f32, f32)>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hurtboxes: Vec<Option<(f32, f32, f32)>>,
}

fn is_one(v: &u32) -> bool {
    *v == 1
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Slicing {
    Grid {
        cell: (u32, u32),
        #[serde(default)]
        offset: (u32, u32),
        #[serde(default)]
        spacing: (u32, u32),
    },
    /// RPG Maker-style character sheet. `layout` defaults to the file-name convention.
    Character {
        #[serde(default)]
        layout: Option<CharacterLayout>,
        #[serde(default)]
        index: u32,
        /// Ticks per walk frame at 60 Hz.
        #[serde(default = "default_walk_ticks")]
        walk_ticks: u32,
    },
    Auto(#[serde(default)] AutoSlice),
    Manual(Vec<Rect>),
    /// A grid where each row is one direction and each run of columns one action, e.g.
    /// directions `[Down, Left, Right, Up]` top to bottom and actions idle (4 frames), walk (4),
    /// run (4), jump (4) left to right. Makes an `<action>_<direction>` clip for every pair.
    Directional {
        cell: (u32, u32),
        /// One per row, top to bottom.
        directions: Vec<Facing>,
        /// Left to right.
        actions: Vec<ActionDef>,
    },
}

/// One animation on a [`Slicing::Directional`] sheet.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ActionDef {
    pub name: String,
    pub frames: u32,
    pub ticks_per_frame: u32,
    #[serde(default = "yes")]
    pub looping: bool,
}

fn default_walk_ticks() -> u32 {
    9
}

fn one() -> u32 {
    1
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClipDef {
    pub name: String,
    pub frames: Vec<u32>,
    pub ticks_per_frame: u32,
    #[serde(default)]
    pub looping: bool,
    /// Mirror the frames (a right-facing clip from left-facing art).
    #[serde(default)]
    pub flip_x: bool,
    /// Anchor for this clip's frames, overriding the sheet's: for sheets whose rows place the
    /// character differently (battler art, where each move stands somewhere else in its cell).
    #[serde(default)]
    pub pivot: Option<(f32, f32)>,
}

impl SheetDef {
    /// Slices `image`. Automatic slicing also returns a new atlas holding only each island's
    /// own pixels, which replaces the source image: on a packed sheet one sprite's box often
    /// encloses a neighbour, and drawing the box would draw both.
    pub fn build(
        &self,
        image: &Image,
        image_path: &Path,
    ) -> Result<(SpriteSheet, Option<Image>), String> {
        let size = (image.width, image.height);
        let mut sheet = SpriteSheet::default();
        let mut repacked = None;
        let rects = match &self.slicing {
            Slicing::Grid {
                cell,
                offset,
                spacing,
            } => {
                if cell.0 == 0 || cell.1 == 0 {
                    return Err("grid cell must be non-empty".into());
                }
                dark_sprite::slice_grid(size, *cell, *offset, *spacing)
            }
            Slicing::Character {
                layout,
                index,
                walk_ticks,
            } => {
                let layout = layout.unwrap_or_else(|| {
                    let stem = image_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    CharacterLayout::from_file_stem(stem)
                });
                if layout.frames == 0 || layout.count() == 0 {
                    return Err(
                        "character layout needs at least one character and one frame".into(),
                    );
                }
                if *index >= layout.count() {
                    return Err(format!("character {index} is not on this sheet"));
                }
                let (cw, ch) = layout.cell(size);
                if cw == 0 || ch == 0 {
                    return Err(format!(
                        "{}x{} image is too small for layout {layout:?}",
                        size.0, size.1
                    ));
                }
                let rows = dark_sprite::slice_character(size, layout, *index);
                for (row, facing) in Facing::CARDINAL.into_iter().enumerate() {
                    let first = row as u32 * layout.frames;
                    let dir = facing.name();
                    sheet.add_clip(Clip::walk_cycle(
                        format!("walk_{dir}"),
                        first,
                        layout.frames,
                        *walk_ticks,
                    ));
                    sheet.add_clip(Clip::idle(format!("idle_{dir}"), first, layout.frames));
                }
                rows.into_iter().flatten().collect()
            }
            Slicing::Auto(options) => {
                let islands =
                    dark_sprite::find_islands(&image.rgba, image.width, image.height, *options);
                if islands.rects.is_empty() {
                    return Err(
                        "automatic slicing found no sprites; is alpha_threshold too high?".into(),
                    );
                }
                let (atlas, rects) = isolate(image, &islands);
                repacked = Some(atlas);
                rects
            }
            Slicing::Manual(rects) => rects.clone(),
            Slicing::Directional {
                cell,
                directions,
                actions,
            } => {
                if cell.0 == 0 || cell.1 == 0 {
                    return Err("grid cell must be non-empty".into());
                }
                let (cols, rows) = (size.0 / cell.0, size.1 / cell.1);
                let used: u32 = actions.iter().map(|a| a.frames).sum();
                if used > cols || directions.len() as u32 > rows {
                    return Err(format!(
                        "{} directions of {used} frames need {}x{} cells; the image has {cols}x{rows}",
                        directions.len(),
                        used,
                        directions.len()
                    ));
                }
                if let Some(a) = actions.iter().find(|a| a.frames == 0) {
                    return Err(format!("action {} has no frames", a.name));
                }
                for (row, facing) in directions.iter().enumerate() {
                    let mut col = 0;
                    for action in actions {
                        let first = row as u32 * cols + col;
                        sheet.add_clip(Clip {
                            name: format!("{}_{}", action.name, facing.name()),
                            frames: (first..first + action.frames).collect(),
                            ticks_per_frame: action.ticks_per_frame,
                            looping: action.looping,
                            flip_x: false,
                        });
                        col += action.frames;
                    }
                }
                dark_sprite::slice_grid(size, *cell, (0, 0), (0, 0))
            }
        };
        let bounds = repacked.as_ref().unwrap_or(image);
        if let Some(bad) = rects.iter().find(|r| {
            r.w == 0 || r.h == 0 || r.right() > bounds.width || r.bottom() > bounds.height
        }) {
            return Err(format!(
                "frame {bad:?} is outside the {}x{} image",
                bounds.width, bounds.height
            ));
        }
        let pixels = repacked.as_ref().unwrap_or(image);
        sheet.frames = rects
            .into_iter()
            .map(|rect| {
                let footprint = (self.pivot == Pivot::Footprint)
                    .then(|| dark_sprite::footprint(&pixels.rgba, pixels.width, rect))
                    .flatten();
                Frame {
                    rect,
                    pivot: footprint.map_or_else(|| self.pivot.resolve(&rect), glam::Vec2::from),
                }
            })
            .collect();
        for clip in &self.clips {
            if let Some(&bad) = clip
                .frames
                .iter()
                .find(|&&f| f as usize >= sheet.frames.len())
            {
                return Err(format!(
                    "clip {} uses frame {bad}, sheet has {}",
                    clip.name,
                    sheet.frames.len()
                ));
            }
            if let Some(pivot) = clip.pivot {
                for &f in &clip.frames {
                    sheet.frames[f as usize].pivot = glam::Vec2::from(pivot);
                }
            }
            sheet.add_clip(Clip {
                name: clip.name.clone(),
                frames: clip.frames.clone(),
                ticks_per_frame: clip.ticks_per_frame,
                looping: clip.looping,
                flip_x: clip.flip_x,
            });
        }
        for (name, boxes) in &self.boxes {
            let id = sheet
                .clip_id(name)
                .ok_or_else(|| format!("boxes for clip {name}, which the sheet does not have"))?;
            let frames = sheet.clips[usize::from(id.0)].frames.len();
            if boxes.hitboxes.len() > frames || boxes.hurtboxes.len() > frames {
                return Err(format!(
                    "clip {name} has {frames} frames, and boxes for more"
                ));
            }
            let circles = |list: &[Option<(f32, f32, f32)>]| -> Vec<Option<dark_sprite::Circle>> {
                if list.is_empty() {
                    return Vec::new();
                }
                // Frames past the list have none.
                let mut circles: Vec<_> = list
                    .iter()
                    .map(|c| {
                        c.map(|(x, y, radius)| dark_sprite::Circle {
                            offset: glam::Vec2::new(x, y),
                            radius,
                        })
                    })
                    .collect();
                circles.resize(frames, None);
                circles
            };
            let index = usize::from(id.0);
            if sheet.timing.len() <= index {
                sheet
                    .timing
                    .resize(index + 1, dark_sprite::ClipTiming::default());
            }
            sheet.timing[index].hitboxes = circles(&boxes.hitboxes);
            sheet.timing[index].hurtboxes = circles(&boxes.hurtboxes);
        }
        Ok((sheet, repacked))
    }
}

/// Copies each island's labelled pixels into a freshly packed atlas; returns the new rects.
fn isolate(image: &Image, islands: &dark_sprite::Islands) -> (Image, Vec<Rect>) {
    const ATLAS_WIDTH: u32 = 2048;
    let sizes: Vec<(u32, u32)> = islands.rects.iter().map(|r| (r.w, r.h)).collect();
    let (positions, (width, height)) = dark_sprite::shelf_pack(&sizes, ATLAS_WIDTH, 1);
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    let mut rects = Vec::with_capacity(islands.rects.len());
    for (i, (src, &(dx, dy))) in islands.rects.iter().zip(&positions).enumerate() {
        let label = i as u32 + 1;
        for y in 0..src.h {
            for x in 0..src.w {
                let s = ((src.y + y) * image.width + src.x + x) as usize;
                if islands.labels[s] != label {
                    continue;
                }
                let d = ((dy + y) * width + dx + x) as usize;
                rgba[d * 4..d * 4 + 4].copy_from_slice(&image.rgba[s * 4..s * 4 + 4]);
            }
        }
        rects.push(Rect::new(dx, dy, src.w, src.h));
    }
    (
        Image {
            width,
            height,
            rgba,
        },
        rects,
    )
}

/// A `scenes/*.ron` file: ground, the local player's sheets, and props.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SceneDef {
    /// World size in pixels, origin top-left.
    pub size: (f32, f32),
    /// The world region (`world.ron`) this map is part of.
    #[serde(default)]
    pub region: Option<String>,
    pub ground: GroundDef,
    /// The local player's character, in the scene the game starts in.
    #[serde(default)]
    pub player: Option<PlayerDef>,
    #[serde(default)]
    pub terrain: TerrainDef,
    #[serde(default)]
    pub props: Vec<PlacedProp>,
    #[serde(default)]
    pub scatter: Vec<ScatterDef>,
    #[serde(default)]
    pub exits: Vec<ExitDef>,
    #[serde(default)]
    pub npcs: Vec<NpcDef>,
    #[serde(default)]
    pub enemies: Vec<EnemyPlacement>,
    /// Inns: sleeping inside one is sleeping indoors.
    #[serde(default)]
    pub inns: Vec<InnDef>,
}

/// An inn's rooms. There are no interiors yet, so the inn is an `area` (by its door): sleeping
/// there is sleeping indoors (warm, the best rest, and enemies leave you be). A character sent to
/// the inn (its player quit or lost connection) is put to bed at `bed`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct InnDef {
    /// x, y, width, height in pixels.
    pub area: (f32, f32, f32, f32),
    pub bed: (f32, f32),
}

impl InnDef {
    pub fn contains(&self, point: (f32, f32)) -> bool {
        let (x, y, w, h) = self.area;
        point.0 >= x && point.0 < x + w && point.1 >= y && point.1 < y + h
    }
}

/// An enemy of a kind the project's `combat.ron` defines, guarding `position`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnemyPlacement {
    pub kind: String,
    pub position: (f32, f32),
    #[serde(default)]
    pub facing: Facing,
}

/// Height levels and walls, painted as rectangles of tiles in order (later ones win).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TerrainDef {
    /// Pixels per level; defaults to the project tile size.
    #[serde(default)]
    pub level_height: Option<f32>,
    #[serde(default)]
    pub fill: Vec<FillDef>,
    /// Ground-sheet frames for raised tiles' tops and cliff faces; default to the ground frame.
    #[serde(default)]
    pub top_frame: Option<u32>,
    #[serde(default)]
    pub face_frame: Option<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct FillDef {
    /// Column, row, width, height in tiles.
    pub tiles: (u32, u32, u32, u32),
    pub cell: Cell,
}

/// A prop's footprint, relative to its pivot on the ground plane.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct ColliderDef {
    pub shape: Shape,
    #[serde(default)]
    pub offset: (f32, f32),
    /// How tall it is; bodies whose feet are above its top pass over it.
    #[serde(default = "default_collider_height")]
    pub height: f32,
}

fn default_collider_height() -> f32 {
    1000.0
}

/// Walking into `area` moves the body to `spawn` in scene `to`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ExitDef {
    /// x, y, width, height in pixels.
    pub area: (f32, f32, f32, f32),
    pub to: String,
    pub spawn: (f32, f32),
}

impl ExitDef {
    pub fn contains(&self, point: (f32, f32)) -> bool {
        let (x, y, w, h) = self.area;
        point.0 >= x && point.0 < x + w && point.1 >= y && point.1 < y + h
    }
}

impl SceneDef {
    /// Builds the collision terrain for this scene at the project's tile size.
    /// Largest map, in tiles per side. Twenty thousand is 320 km at 16 px to the tile, or
    /// 20 km if a tile is a metre — past what §24.0 says one `f32` should hold a position in,
    /// and far past what anyone will draw by hand. The land itself costs nothing until it is
    /// shaped (§24.4), so the cap is about what the rest of the engine can carry, not about
    /// what a grid of tiles costs.
    pub const MAX_TILES_PER_SIDE: u32 = 20_000;

    pub fn build_terrain(&self, tile_size: u32) -> Result<Terrain, String> {
        let tile = tile_size as f32;
        let (w, h) = self.size;
        if !(w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()) {
            return Err(format!("scene size {w}x{h} must be positive"));
        }
        let cols = (w / tile).ceil() as u32;
        let rows = (h / tile).ceil() as u32;
        if cols > Self::MAX_TILES_PER_SIDE || rows > Self::MAX_TILES_PER_SIDE {
            return Err(format!(
                "scene is {cols}x{rows} tiles; at most {} per side",
                Self::MAX_TILES_PER_SIDE
            ));
        }
        let level_height = self.terrain.level_height.unwrap_or(tile);
        if !(level_height > 0.0 && level_height.is_finite()) {
            return Err(format!("level_height {level_height} must be positive"));
        }
        let mut terrain = Terrain::new(cols, rows, tile, level_height);
        for fill in &self.terrain.fill {
            let (c, r, w, h) = fill.tiles;
            terrain.fill(c, r, w, h, fill.cell);
        }
        Ok(terrain)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GroundDef {
    pub sheet: String,
    /// Seamless texture frame, repeated over the whole scene.
    pub frame: u32,
}

/// A character's look: its sheet (walk and idle, optionally run and jump clips), an optional
/// separate attack sheet, and how it appears in dialogue. Sheet paths are relative to the
/// project root.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LookDef {
    pub sheet: String,
    pub attack: Option<String>,
    pub face: Option<FaceDef>,
    /// String-table key of the name shown in dialogue.
    pub name: Option<String>,
    /// How it fights (a moveset in the project's `combat.ron`); unarmed if none.
    pub moveset: Option<String>,
}

/// A face for dialogue, from an RPG Maker faceset: a 4×2 grid of faces, `index` counting
/// across then down.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct FaceDef {
    pub image: String,
    #[serde(default)]
    pub index: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PlayerDef {
    #[serde(alias = "walk")]
    pub sheet: String,
    #[serde(default)]
    pub attack: Option<String>,
    #[serde(default)]
    pub face: Option<FaceDef>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub moveset: Option<String>,
    pub spawn: (f32, f32),
}

impl PlayerDef {
    pub fn look(&self) -> LookDef {
        LookDef {
            sheet: self.sheet.clone(),
            attack: self.attack.clone(),
            face: self.face.clone(),
            name: self.name.clone(),
            moveset: self.moveset.clone(),
        }
    }
}

/// One step of an NPC's conversation, as a string-table key (see [`Localization`]): written
/// `"key"` the NPC says it, `(reply: "key")` whoever is talking to the NPC says it back.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum LineDef {
    Says(String),
    Reply { reply: String },
}

impl LineDef {
    pub fn key(&self) -> &str {
        match self {
            LineDef::Says(key) | LineDef::Reply { reply: key } => key,
        }
    }
}

/// A character the world places in a scene. Talking to it starts its conversation; each press
/// moves it on one line, and the press after the last closes it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NpcDef {
    pub sheet: String,
    #[serde(default)]
    pub attack: Option<String>,
    #[serde(default)]
    pub face: Option<FaceDef>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub moveset: Option<String>,
    pub position: (f32, f32),
    #[serde(default)]
    pub facing: Facing,
    /// Keys, not text: every player reads them in their own language.
    pub lines: Vec<LineDef>,
    /// The world actor (`world.ron`) this NPC is: a person who can be asked to follow.
    #[serde(default)]
    pub actor: Option<String>,
    /// Where this person is through the day. Empty: they stand where they were placed.
    #[serde(default)]
    pub day: Vec<DayEntry>,
}

/// A person's day: from `hour` they make their way to `at`, and lie down there if `sleep`.
/// The entry in force is the latest one whose hour has come, so the last runs through midnight.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct DayEntry {
    /// Hour of the day, 0 to 24.
    pub from: f32,
    /// Where in this scene they go.
    pub at: (f32, f32),
    #[serde(default)]
    pub sleep: bool,
}

impl DayEntry {
    /// The entry in force at `hour`: the latest that has come, else the last of the day (which
    /// runs through midnight into the morning).
    pub fn at_hour(day: &[DayEntry], hour: f32) -> Option<&DayEntry> {
        day.iter()
            .filter(|entry| entry.from <= hour)
            .max_by(|a, b| a.from.total_cmp(&b.from))
            .or_else(|| day.iter().max_by(|a, b| a.from.total_cmp(&b.from)))
    }
}

impl NpcDef {
    pub fn look(&self) -> LookDef {
        LookDef {
            sheet: self.sheet.clone(),
            attack: self.attack.clone(),
            face: self.face.clone(),
            name: self.name.clone(),
            moveset: self.moveset.clone(),
        }
    }
}

impl Project {
    /// The definition in `*.sheet.ron` file `path`, as written (for the editor).
    pub fn load_sheet_def(&self, path: impl AsRef<Path>) -> Result<SheetDef, AssetError> {
        read_ron(&self.path(path))
    }

    /// Writes `def` to sheet file `path` (the editor's save). Comments in the file are not kept.
    pub fn save_sheet_def(&self, path: impl AsRef<Path>, def: &SheetDef) -> Result<(), AssetError> {
        let path = self.path(path);
        let config = ron::ser::PrettyConfig::default()
            .extensions(ron::extensions::Extensions::IMPLICIT_SOME)
            .struct_names(false);
        let body = ron::ser::to_string_pretty(def, config)
            .map_err(|e| invalid(&path, format!("cannot write the sheet: {e}")))?;
        let text = format!("// A Dark Engine sheet: made in the editor (dark-editor).\n{body}\n");
        std::fs::write(&path, text).map_err(|source| AssetError::Io { path, source })
    }

    /// Writes `def` to scene file `path` (the editor's save). Comments in the file are not kept.
    pub fn save_scene(&self, path: impl AsRef<Path>, def: &SceneDef) -> Result<(), AssetError> {
        let path = self.path(path);
        let config = ron::ser::PrettyConfig::default()
            .extensions(ron::extensions::Extensions::IMPLICIT_SOME)
            .struct_names(false);
        let body = ron::ser::to_string_pretty(def, config)
            .map_err(|e| invalid(&path, format!("cannot write the scene: {e}")))?;
        let text = format!(
            "// A Dark Engine scene: made in the editor (dark-editor). Positions are pixels.
{body}
"
        );
        std::fs::write(&path, text).map_err(|source| AssetError::Io { path, source })
    }

    /// The face `def` names, resized to `size`×`size` with a smooth filter: faces are painted
    /// art, not pixel art, so they are scaled like a picture.
    pub fn load_face(&self, def: &FaceDef, size: u32) -> Result<Image, AssetError> {
        let path = self.path(&def.image);
        let image = load_image(&path)?;
        let (cw, ch) = (image.width / 4, image.height / 2);
        if def.index >= 8 || cw == 0 || ch == 0 {
            return Err(invalid(
                &path,
                format!(
                    "face {} of a {}x{} faceset (4x2 faces)",
                    def.index, image.width, image.height
                ),
            ));
        }
        let (x, y) = ((def.index % 4) * cw, (def.index / 4) * ch);
        Ok(crop_resized(image, (x, y, cw, ch), size))
    }

    /// Cell `cell` (column, row) of `image`, a grid of `cell_size`-pixel cells, resized to
    /// `size`×`size` with a smooth filter: for painted icons.
    pub fn load_icon(
        &self,
        image: &str,
        cell_size: u32,
        cell: (u32, u32),
        size: u32,
    ) -> Result<Image, AssetError> {
        let path = self.path(image);
        let loaded = load_image(&path)?;
        let fits = |cell: u32, size: u32| {
            cell.checked_mul(cell_size)
                .filter(|start| start.checked_add(cell_size).is_some_and(|end| end <= size))
        };
        let (Some(x), Some(y)) = (fits(cell.0, loaded.width), fits(cell.1, loaded.height)) else {
            return Err(invalid(
                &path,
                format!(
                    "no {cell_size} px icon at {cell:?} in a {}x{} image",
                    loaded.width, loaded.height
                ),
            ));
        };
        if cell_size == 0 {
            return Err(invalid(&path, "icon size 0".into()));
        }
        Ok(crop_resized(loaded, (x, y, cell_size, cell_size), size))
    }
}

/// The `(x, y, w, h)` part of `image`, resized to `size`×`size` (Catmull-Rom).
fn crop_resized(image: Image, (x, y, w, h): (u32, u32, u32, u32), size: u32) -> Image {
    let full = image::RgbaImage::from_raw(image.width, image.height, image.rgba)
        .expect("decoded image matches its size");
    let cell = image::imageops::crop_imm(&full, x, y, w, h).to_image();
    let resized =
        image::imageops::resize(&cell, size, size, image::imageops::FilterType::CatmullRom);
    Image {
        width: size,
        height: size,
        rgba: resized.into_raw(),
    }
}

/// String tables, one per language in [`ProjectSettings::languages`], from
/// `locale/<code>.ron` (a map of key to text). Game data names strings by key; the text is
/// looked up at the last moment, in the reader's language.
#[derive(Clone, Debug, Default)]
pub struct Localization {
    /// `(code, table)` in project order; the first is the fallback.
    tables: Vec<(String, std::collections::HashMap<String, String>)>,
    current: usize,
}

impl Localization {
    pub fn load(project: &Project) -> Result<Self, AssetError> {
        let tables = project
            .settings
            .languages
            .iter()
            .map(|code| {
                let path = project.path(format!("locale/{code}.ron"));
                let mut table: std::collections::HashMap<String, String> = read_ron(&path)?;
                // Text written in the editor lives beside the hand-written table, and wins.
                let edited = project.path(format!("locale/{code}.editor.ron"));
                if edited.exists() {
                    let more: std::collections::HashMap<String, String> = read_ron(&edited)?;
                    table.extend(more);
                }
                Ok((code.clone(), table))
            })
            .collect::<Result<_, AssetError>>()?;
        Ok(Self { tables, current: 0 })
    }

    /// Built directly, for tests and tools.
    pub fn from_tables(tables: Vec<(String, std::collections::HashMap<String, String>)>) -> Self {
        Self { tables, current: 0 }
    }

    pub fn language(&self) -> &str {
        self.tables.get(self.current).map_or("", |(code, _)| code)
    }

    /// Switches language; false if the project has no such language.
    pub fn set_language(&mut self, code: &str) -> bool {
        match self.tables.iter().position(|(c, _)| c == code) {
            Some(i) => {
                self.current = i;
                true
            }
            None => false,
        }
    }

    /// The next language in project order, wrapping round.
    pub fn cycle(&mut self) {
        if !self.tables.is_empty() {
            self.current = (self.current + 1) % self.tables.len();
        }
    }

    /// Whether the current language itself has text for `key` (no fallback).
    pub fn has(&self, key: &str) -> bool {
        self.tables
            .get(self.current)
            .is_some_and(|(_, table)| table.contains_key(key))
    }

    /// The text for `key` in the current language, else the first language, else the key itself
    /// (so a missing string shows up on screen instead of disappearing).
    pub fn text<'a>(&'a self, key: &'a str) -> &'a str {
        [self.tables.get(self.current), self.tables.first()]
            .into_iter()
            .flatten()
            .find_map(|(_, table)| table.get(key))
            .map_or(key, String::as_str)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct PlacedProp {
    pub sheet: String,
    pub frame: u32,
    /// World position of the frame's pivot.
    pub position: (f32, f32),
    #[serde(default)]
    pub colliders: Vec<ColliderDef>,
}

/// Deterministic random placement, for filling a scene before hand-placed props exist.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ScatterDef {
    pub sheet: String,
    pub frames: Vec<u32>,
    pub count: u32,
    /// Minimum distance to every prop already placed, including earlier scatter groups.
    pub min_spacing: f32,
    pub seed: u64,
    /// Given to every prop this group places.
    #[serde(default)]
    pub collider: Option<ColliderDef>,
    /// Ground levels this group may be placed on; any level when absent.
    #[serde(default)]
    pub levels: Option<Vec<u8>>,
}

impl SceneDef {
    /// Hand-placed props, then each scatter group in order. Scattered props stay `keep_clear`
    /// away from the player spawn and every point in `arrivals` (where exits from other scenes
    /// land), out of exits and hand-placed footprints, and only where `accept(group, position)`
    /// agrees (flat ground on an allowed level, say). The same file always produces the same
    /// scene.
    pub fn placed_props(
        &self,
        keep_clear: f32,
        arrivals: &[(f32, f32)],
        accept: impl Fn(&ScatterDef, (f32, f32)) -> bool,
    ) -> Vec<PlacedProp> {
        const MARGIN: f32 = 24.0;
        /// Scattered props keep this far outside hand-placed props' footprints.
        const FOOTPRINT_CLEARING: f32 = 12.0;
        let mut placed = self.props.clone();
        let spawn = self.player.as_ref().map(|p| p.spawn);
        for group in &self.scatter {
            if group.frames.is_empty() {
                continue;
            }
            let mut rng = SplitMix64(group.seed);
            let mut added = 0;
            for _ in 0..group.count * 40 {
                if added == group.count {
                    break;
                }
                let x = MARGIN + rng.unit() * (self.size.0 - 2.0 * MARGIN);
                let y = MARGIN + rng.unit() * (self.size.1 - 2.0 * MARGIN);
                let too_close = |p: (f32, f32), d: f32| {
                    // Plain multiplies: powi precision may differ across platforms.
                    let (dx, dy) = (p.0 - x, p.1 - y);
                    dx * dx + dy * dy < d * d
                };
                let position = (x.round(), y.round());
                // Clear of the footprint of anything placed by hand, however big (a house).
                let on_placed = self.props.iter().any(|p| {
                    p.colliders.iter().any(|c| {
                        let (cx, cy) = (p.position.0 + c.offset.0, p.position.1 + c.offset.1);
                        let (dx, dy) = ((x - cx).abs(), (y - cy).abs());
                        match c.shape {
                            Shape::Circle { radius } => {
                                let r = radius + FOOTPRINT_CLEARING;
                                dx * dx + dy * dy < r * r
                            }
                            Shape::Rect { half } => {
                                dx < half.x + FOOTPRINT_CLEARING && dy < half.y + FOOTPRINT_CLEARING
                            }
                        }
                    })
                });
                if spawn.is_some_and(|s| too_close(s, keep_clear))
                    || arrivals.iter().any(|&a| too_close(a, keep_clear))
                    || self.exits.iter().any(|e| e.contains(position))
                    || on_placed
                    || placed
                        .iter()
                        .any(|p| too_close(p.position, group.min_spacing))
                    || !accept(group, position)
                {
                    continue;
                }
                let frame = group.frames[(rng.next() % group.frames.len() as u64) as usize];
                placed.push(PlacedProp {
                    sheet: group.sheet.clone(),
                    frame,
                    position,
                    colliders: group.collider.into_iter().collect(),
                });
                added += 1;
            }
        }
        placed
    }
}

/// Small, stable PRNG so scattered scenes are identical on every machine and version.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

impl Project {
    pub fn load_scene(&self, path: impl AsRef<Path>) -> Result<SceneDef, AssetError> {
        read_ron(&self.path(path))
    }
}

fn read_ron<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, AssetError> {
    let text = std::fs::read_to_string(path).map_err(|source| AssetError::Io {
        path: path.to_owned(),
        source,
    })?;
    // implicit_some: optional fields can be written `frame: 3` instead of `frame: Some(3)`.
    ron::Options::default()
        .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
        .from_str(&text)
        .map_err(|source| AssetError::Parse {
            path: path.to_owned(),
            source: Box::new(source),
        })
}

fn invalid(path: &Path, message: String) -> AssetError {
    AssetError::Invalid {
        path: path.to_owned(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32) -> Image {
        Image {
            width: w,
            height: h,
            rgba: vec![255; (w * h * 4) as usize],
        }
    }

    #[test]
    fn character_def_builds_frames_and_clips() {
        let def: SheetDef = ron::from_str(
            r#"(
                image: "Art/x1/Character01.png",
                slicing: Character(layout: Some((characters: (1, 1), frames: 4))),
                clips: [(name: "attack_down", frames: [0, 1, 2, 3], ticks_per_frame: 4)],
            )"#,
        )
        .unwrap();
        let (sheet, _) = def
            .build(&image(256, 256), Path::new("Character01.png"))
            .unwrap();
        assert_eq!(sheet.frames.len(), 16);
        assert_eq!(sheet.frames[5].rect, Rect::new(64, 64, 64, 64));
        let pivot = sheet.frames[5].pivot;
        assert_eq!((pivot.x, pivot.y), (32.0, 64.0));
        let walk_left = sheet.clip_id("walk_left").unwrap();
        assert_eq!(sheet.clips[walk_left.0 as usize].frames, vec![4, 5, 6, 7]);
        assert!(sheet.clip_id("attack_down").is_some());
    }

    #[test]
    fn character_layout_defaults_to_file_name() {
        let def: SheetDef = ron::from_str(r#"(image: "x", slicing: Character(index: 7))"#).unwrap();
        let (sheet, _) = def
            .build(&image(3072, 1536), Path::new("MonsterChara01+(4).png"))
            .unwrap();
        assert_eq!(sheet.frames.len(), 16);
        assert_eq!(sheet.frames[0].rect, Rect::new(2304, 768, 192, 192));
    }

    fn scene() -> SceneDef {
        ron::from_str(
            r#"(
                size: (800, 600),
                ground: (sheet: "g", frame: 0),
                player: Some((sheet: "w", attack: Some("a"), spawn: (400, 300))),
                terrain: (fill: [(tiles: (0, 0, 10, 2), cell: Level(1)), (tiles: (2, 0, 1, 1), cell: Wall)]),
                exits: [(area: (0, 500, 800, 100), to: "other.ron", spawn: (10, 10))],
                props: [(sheet: "p", frame: 1, position: (100, 100))],
                scatter: [(sheet: "p", frames: [2, 3], count: 30, min_spacing: 40, seed: 7)],
            )"#,
        )
        .unwrap()
    }

    #[test]
    fn scatter_is_deterministic_spaced_and_clear_of_spawn() {
        let scene = scene();
        let props = scene.placed_props(80.0, &[], |_, _| true);
        assert_eq!(props, scene.placed_props(80.0, &[], |_, _| true));
        assert_eq!(props.len(), 31);
        assert_eq!(props[0].position, (100.0, 100.0));
        for (i, a) in props.iter().enumerate() {
            let (x, y) = a.position;
            assert!((0.0..=800.0).contains(&x) && (0.0..=600.0).contains(&y));
            assert!(
                (x - 400.0).hypot(y - 300.0) >= 80.0 - 1.0,
                "prop {i} too close to spawn"
            );
            for b in &props[i + 1..] {
                let d = (x - b.position.0).hypot(y - b.position.1);
                assert!(
                    d >= 40.0 - 1.5,
                    "props {:?} and {:?} are {d} apart",
                    a.position,
                    b.position
                );
            }
            if i > 0 {
                assert!(y < 500.0, "prop {i} placed inside the exit");
            }
        }
    }

    #[test]
    fn scatter_respects_accept_filter() {
        let props = scene().placed_props(80.0, &[], |_, (x, _)| x < 300.0);
        assert!(props[1..].iter().all(|p| p.position.0 < 300.0));
        assert!(props.len() > 5);
    }

    #[test]
    fn scatter_keeps_out_of_a_big_placed_prop() {
        let mut scene = scene();
        // A house-sized footprint: 200 x 80 around (600, 200).
        scene.props[0].position = (600.0, 240.0);
        scene.props[0].colliders = vec![ColliderDef {
            shape: Shape::Rect {
                half: glam::Vec2::new(100.0, 40.0),
            },
            offset: (0.0, -40.0),
            height: 1000.0,
        }];
        let props = scene.placed_props(80.0, &[], |_, _| true);
        assert!(props.len() > 5);
        for p in &props[1..] {
            let (x, y) = p.position;
            assert!(
                !((490.0..710.0).contains(&x) && (150.0..250.0).contains(&y)),
                "{x},{y} is inside the house"
            );
        }
    }

    #[test]
    fn terrain_is_built_from_fills_in_order() {
        let terrain = scene().build_terrain(16).unwrap();
        assert_eq!((terrain.cols(), terrain.rows()), (50, 38));
        assert_eq!(terrain.cell(0, 0), Some(Cell::Level(1)));
        assert_eq!(terrain.cell(2, 0), Some(Cell::Wall), "later fill wins");
        assert_eq!(terrain.cell(0, 2), Some(Cell::Floor));
        assert_eq!(terrain.level_height(), 16.0);
    }

    #[test]
    fn auto_slicing_isolates_sprites_whose_boxes_overlap() {
        // An L whose box encloses a separate 4x4 square.
        let (w, h) = (20u32, 20u32);
        let mut src = Image {
            width: w,
            height: h,
            rgba: vec![0; (w * h * 4) as usize],
        };
        let mut set = |x: u32, y: u32, v: u8| {
            let i = ((y * w + x) * 4) as usize;
            src.rgba[i..i + 4].copy_from_slice(&[v, v, v, 255]);
        };
        for i in 0..20 {
            set(i, 0, 10);
            set(0, i, 10);
        }
        for y in 8..12 {
            for x in 8..12 {
                set(x, y, 200);
            }
        }
        let def: SheetDef = ron::from_str(r#"(image: "x", slicing: Auto(()))"#).unwrap();
        let (sheet, atlas) = def.build(&src, Path::new("x")).unwrap();
        let atlas = atlas.expect("auto slicing repacks");
        assert_eq!(sheet.frames.len(), 2);
        let l = sheet.frames[0].rect;
        assert_eq!((l.w, l.h), (20, 20));
        let bright_inside_l = (l.y..l.bottom()).any(|y| {
            (l.x..l.right()).any(|x| atlas.rgba[((y * atlas.width + x) * 4) as usize] == 200)
        });
        assert!(
            !bright_inside_l,
            "the square must not be drawn as part of the L"
        );
    }

    #[test]
    fn rejects_out_of_range_frames() {
        let def: SheetDef = ron::from_str(
            r#"(image: "x", slicing: Manual([(x: 0, y: 0, w: 8, h: 8)]),
                clips: [(name: "a", frames: [3], ticks_per_frame: 1)])"#,
        )
        .unwrap();
        assert!(
            def.build(&image(8, 8), Path::new("x"))
                .unwrap_err()
                .contains("frame 3")
        );
        let def: SheetDef =
            ron::from_str(r#"(image: "x", slicing: Manual([(x: 4, y: 0, w: 8, h: 8)]))"#).unwrap();
        assert!(def.build(&image(8, 8), Path::new("x")).is_err());
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    fn blank(w: u32, h: u32) -> Image {
        Image {
            width: w,
            height: h,
            rgba: vec![0; (w * h * 4) as usize],
        }
    }

    fn build(def: &str, image: &Image) -> Result<(SpriteSheet, Option<Image>), String> {
        ron::from_str::<SheetDef>(def)
            .unwrap()
            .build(image, Path::new("x"))
    }

    #[test]
    fn empty_auto_slice_is_an_error_not_a_zero_sized_atlas() {
        let err = build(r#"(image: "x", slicing: Auto(()))"#, &blank(8, 8)).unwrap_err();
        assert!(err.contains("no sprites"), "{err}");
    }

    #[test]
    fn degenerate_character_layouts_are_errors() {
        let zero_frames =
            r#"(image: "x", slicing: Character(layout: Some((characters: (1, 1), frames: 0))))"#;
        assert!(build(zero_frames, &blank(64, 64)).is_err());
        let too_small =
            r#"(image: "x", slicing: Character(layout: Some((characters: (4, 2), frames: 3))))"#;
        assert!(
            build(too_small, &blank(8, 8))
                .unwrap_err()
                .contains("too small")
        );
    }

    #[test]
    fn directional_sheets_make_a_clip_per_action_and_direction() {
        let def = r#"(image: "x", slicing: Directional(
            cell: (8, 8),
            directions: [Down, Left, DownRight],
            actions: [
                (name: "idle", frames: 2, ticks_per_frame: 10),
                (name: "jump", frames: 3, ticks_per_frame: 5, looping: false),
            ],
        ))"#;
        // 6 columns (one spare), 3 rows.
        let (sheet, _) = build(def, &blank(48, 24)).unwrap();
        assert_eq!(sheet.frames.len(), 18);
        let clip = |name: &str| &sheet.clips[sheet.clip_id(name).unwrap().0 as usize];
        assert_eq!(clip("idle_down").frames, vec![0, 1]);
        assert_eq!(clip("jump_left").frames, vec![8, 9, 10]);
        assert!(!clip("jump_left").looping);
        assert_eq!(clip("idle_down_right").frames, vec![12, 13]);
        assert_eq!(sheet.frames[13].rect, Rect::new(8, 16, 8, 8));
        assert!(
            build(def, &blank(32, 24)).is_err(),
            "5 frames need 5 columns"
        );
    }

    #[test]
    fn a_sheets_own_boxes_become_its_clip_timing() {
        let def = r#"(image: "x", slicing: Grid(cell: (8, 8)),
            clips: [(name: "swing", frames: [0, 1, 2], ticks_per_frame: 4),
                    (name: "rest", frames: [3], ticks_per_frame: 4)],
            boxes: {"swing": (hitboxes: [None, Some((0, 12, 6))], hurtboxes: [Some((0, 0, 5))])})"#;
        let (sheet, _) = build(def, &blank(32, 8)).unwrap();
        let swing = sheet.timing(sheet.clip_id("swing").unwrap()).unwrap();
        assert_eq!(swing.hitboxes.len(), 3, "one per frame, the last without");
        assert_eq!(
            swing.hitboxes[1].unwrap().offset,
            glam::Vec2::new(0.0, 12.0)
        );
        assert_eq!(swing.hitboxes[2], None);
        assert_eq!(swing.hurtboxes[0].unwrap().radius, 5.0);
        assert!(
            sheet
                .timing(sheet.clip_id("rest").unwrap())
                .is_none_or(|t| t.hitboxes.is_empty()),
            "a clip without boxes uses its moveset's"
        );
        // Written back, a sheet says the same (and no more than it did).
        let parsed: SheetDef = ron::from_str(def).unwrap();
        let text = ron::ser::to_string(&parsed).unwrap();
        let again: SheetDef = ron::from_str(&text).unwrap();
        assert_eq!(again.boxes, parsed.boxes);
        assert!(!text.contains("downscale"), "{text}");
        let too_many = def.replace("(0, 0, 5))]", "(0, 0, 5)), None, None, None]");
        assert!(
            build(&too_many, &blank(32, 8))
                .unwrap_err()
                .contains("boxes for more")
        );
        let unknown = def.replace("{\"swing\"", "{\"lunge\"");
        assert!(
            build(&unknown, &blank(32, 8))
                .unwrap_err()
                .contains("lunge")
        );
    }

    #[test]
    fn downscale_takes_each_block_centre() {
        let mut src = blank(6, 3);
        for x in 3..6 {
            for y in 0..3 {
                src.rgba[((y * 6 + x) * 4) as usize] = 200;
            }
        }
        let small = downscale(&src, 3);
        assert_eq!((small.width, small.height), (2, 1));
        assert_eq!((small.rgba[0], small.rgba[4]), (0, 200));
    }

    #[test]
    fn localization_falls_back_to_the_first_language_then_the_key() {
        let table = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let mut l = Localization::from_tables(vec![
            ("en".into(), table(&[("hi", "Hello"), ("only_en", "Only")])),
            ("ja".into(), table(&[("hi", "こんにちは")])),
        ]);
        assert_eq!(l.text("hi"), "Hello");
        assert!(l.set_language("ja"));
        assert_eq!(l.text("hi"), "こんにちは");
        assert_eq!(l.text("only_en"), "Only");
        assert!(l.has("hi") && !l.has("only_en"), "has() does not fall back");
        assert_eq!(l.text("missing"), "missing");
        assert!(!l.set_language("fr"));
        l.cycle();
        assert_eq!(l.language(), "en");
    }

    #[test]
    fn a_clip_can_anchor_its_frames_differently_from_the_sheet() {
        let def = r#"(image: "x", pivot: Pixel(4, 8),
            slicing: Manual([(x: 0, y: 0, w: 8, h: 8), (x: 0, y: 0, w: 8, h: 8)]),
            clips: [(name: "lunge", frames: [1], ticks_per_frame: 1, pivot: Some((2, 7)))])"#;
        let (sheet, _) = build(def, &blank(8, 8)).unwrap();
        assert_eq!(sheet.frames[0].pivot, glam::Vec2::new(4.0, 8.0));
        assert_eq!(sheet.frames[1].pivot, glam::Vec2::new(2.0, 7.0));
    }

    #[test]
    fn npc_lines_are_said_by_the_npc_or_replied_by_the_talker() {
        let npc: NpcDef =
            ron::from_str(r#"(sheet: "n", position: (0, 0), lines: ["a", (reply: "b"), "c"])"#)
                .unwrap();
        assert_eq!(
            npc.lines,
            vec![
                LineDef::Says("a".into()),
                LineDef::Reply { reply: "b".into() },
                LineDef::Says("c".into())
            ]
        );
        assert_eq!(npc.lines[1].key(), "b");
    }

    #[test]
    fn faces_are_cut_from_a_four_by_two_faceset_and_resized() {
        let dir = std::env::temp_dir().join(format!("dark_assets_face_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("project.ron"),
            r#"(name: "t", tile_size: 16, resolution: (320, 180))"#,
        )
        .unwrap();
        // 8x4: faces are 2x2; face 5 (second row, second column) is red, the rest clear.
        let mut faces = image::RgbaImage::new(8, 4);
        for (x, y) in [(2, 2), (3, 2), (2, 3), (3, 3)] {
            faces.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
        }
        faces.save(dir.join("faces.png")).unwrap();
        let project = Project::open(&dir).unwrap();
        let def = |index| FaceDef {
            image: "faces.png".into(),
            index,
        };
        let face = project.load_face(&def(5), 4).unwrap();
        assert_eq!((face.width, face.height), (4, 4));
        assert!(face.rgba.chunks(4).all(|p| p == [255, 0, 0, 255]));
        let empty = project.load_face(&def(0), 4).unwrap();
        assert!(empty.rgba.chunks(4).all(|p| p[3] == 0));
        assert!(project.load_face(&def(8), 4).is_err());
    }

    #[test]
    fn zero_sized_manual_frames_are_errors() {
        let def = r#"(image: "x", slicing: Manual([(x: 0, y: 0, w: 0, h: 4)]))"#;
        assert!(build(def, &blank(8, 8)).is_err());
    }
}

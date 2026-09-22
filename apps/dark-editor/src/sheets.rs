//! Sprite sheets: how a picture is cut into frames (`*.sheet.ron`), the clips made of them, and
//! where each frame of a clip hits and can be hit. Skeleton sheets (`*.spine.ron`) are baked
//! here from their Spine export instead.

use std::collections::BTreeMap;

use dark_assets::{ActionDef, ClipBoxes, ClipDef, Image, Project, SheetDef, Slicing};
use dark_sprite::{AutoSlice, CharacterLayout, Facing, Pivot, Rect, SpriteSheet};
use egui::{Color32, ComboBox, Pos2, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::catalog::sheet_name;
use crate::widgets::{Tracker, tidy_id, valid_id};

/// A plain sheet being edited.
struct Plain {
    def: SheetDef,
    /// The picture, shrunk as the sheet says.
    source: Image,
    /// What the sheet is now: frames and clips, and the picture they are cut from (automatic
    /// slicing packs its own), or why it cannot be cut.
    built: Result<(SpriteSheet, Option<Image>), String>,
    texture: Option<egui::TextureHandle>,
    /// The picture the texture shows changed.
    stale_texture: bool,
    undo: Vec<SheetDef>,
    redo: Vec<SheetDef>,
    editing: Option<egui::Id>,
    dirty: bool,
}

enum Kind {
    Plain(Box<Plain>),
    /// A skeleton: nothing to edit but its bake.
    Spine {
        report: Option<String>,
    },
    Broken(String),
}

struct OpenSheet {
    path: String,
    kind: Kind,
    /// Frames picked on the picture, in the order picked (a new clip is made of them).
    picked: Vec<u32>,
    clip: Option<String>,
    /// The frame of the clip shown for its boxes.
    step: usize,
    playing: bool,
    zoom: f32,
    marking: Mark,
    new_clip: String,
}

/// Which circle a click on a frame places.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Hit,
    Hurt,
}

pub struct SheetEditor {
    pub list: Vec<String>,
    /// Sheets of `list` the game cannot load (to be fixed here).
    pub broken: Vec<String>,
    open: Option<OpenSheet>,
    /// Sheets written since the editor last asked: maps must load them again.
    pub saved: Vec<String>,
    new_name: String,
    new_image: String,
}

impl SheetEditor {
    pub fn new(list: Vec<String>) -> Self {
        Self {
            list,
            broken: Vec::new(),
            open: None,
            saved: Vec::new(),
            new_name: String::new(),
            new_image: String::new(),
        }
    }

    pub fn dirty(&self) -> bool {
        self.plain().is_some_and(|p| p.dirty)
    }

    fn plain(&self) -> Option<&Plain> {
        match &self.open.as_ref()?.kind {
            Kind::Plain(p) => Some(p),
            _ => None,
        }
    }

    pub fn can_undo(&self) -> bool {
        self.plain().is_some_and(|p| !p.undo.is_empty())
    }

    pub fn can_redo(&self) -> bool {
        self.plain().is_some_and(|p| !p.redo.is_empty())
    }

    pub fn undo_step(&mut self, project: &Project) {
        if let Some(OpenSheet {
            kind: Kind::Plain(p),
            ..
        }) = &mut self.open
            && let Some(before) = p.undo.pop()
        {
            p.redo.push(std::mem::replace(&mut p.def, before));
            p.dirty = true;
            p.editing = None;
            reload_image(p, project);
            rebuild(p, project);
        }
    }

    pub fn redo_step(&mut self, project: &Project) {
        if let Some(OpenSheet {
            kind: Kind::Plain(p),
            ..
        }) = &mut self.open
            && let Some(after) = p.redo.pop()
        {
            p.undo.push(std::mem::replace(&mut p.def, after));
            p.dirty = true;
            p.editing = None;
            reload_image(p, project);
            rebuild(p, project);
        }
    }

    /// Writes the open sheet if it changed. Err if it could not be written.
    pub fn save(&mut self, project: &Project) -> Result<Option<String>, String> {
        let Some(open) = &mut self.open else {
            return Ok(None);
        };
        let Kind::Plain(p) = &mut open.kind else {
            return Ok(None);
        };
        if !p.dirty {
            return Ok(None);
        }
        // The game could not load it: it stays unsaved until it cuts again.
        if let Err(e) = &p.built {
            return Err(format!(
                "{} cannot be cut ({e}), so it is kept unsaved",
                open.path
            ));
        }
        // Circles of clips (or frames) that are gone go now, said so.
        let kept = pruned(&p.def, &p.source, &project.path(&p.def.image));
        let dropped: Vec<String> = p
            .def
            .boxes
            .iter()
            .filter(|(name, b)| kept.boxes.get(*name) != Some(b))
            .map(|(name, _)| name.clone())
            .collect();
        project
            .save_sheet_def(&open.path, &kept)
            .map_err(|e| e.to_string())?;
        p.def = kept;
        p.dirty = false;
        self.saved.push(open.path.clone());
        Ok((!dropped.is_empty()).then(|| {
            format!(
                "circles of frames {} no longer has were dropped ({})",
                sheet_name(&open.path),
                dropped.join(", ")
            )
        }))
    }

    fn open(&mut self, project: &Project, path: &str) {
        let kind = if path.ends_with(".spine.ron") {
            Kind::Spine { report: None }
        } else {
            match load_plain(project, path) {
                Ok(p) => Kind::Plain(Box::new(p)),
                Err(e) => Kind::Broken(e),
            }
        };
        self.open = Some(OpenSheet {
            path: path.to_owned(),
            kind,
            picked: Vec::new(),
            clip: None,
            step: 0,
            playing: false,
            zoom: 2.0,
            marking: Mark::Hit,
            new_clip: String::new(),
        });
    }

    // --- The interface ---------------------------------------------------------------------

    pub fn ui(&mut self, ui: &mut Ui, project: &Project) {
        egui::Panel::left("sheet list")
            .resizable(true)
            .default_size(230.0)
            .min_size(180.0)
            .show(ui, |ui| self.list_ui(ui, project));
        let Some(open) = &mut self.open else {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label("Pick a sheet on the left, or make a new one from a picture.");
            });
            return;
        };
        match &mut open.kind {
            Kind::Broken(e) => {
                let e = e.clone();
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.colored_label(Color32::from_rgb(255, 110, 100), e);
                });
            }
            Kind::Spine { report } => {
                let path = open.path.clone();
                egui::CentralPanel::default().show(ui, |ui| spine_ui(ui, project, &path, report));
            }
            Kind::Plain(p) => {
                let p = p.as_mut();
                let before = p.def.clone();
                let mut f = Tracker::default();
                egui::Panel::right("sheet settings")
                    .resizable(true)
                    .default_size(380.0)
                    .min_size(340.0)
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            settings(
                                ui,
                                &mut f,
                                p,
                                open_parts(
                                    &mut open.picked,
                                    &mut open.clip,
                                    &mut open.step,
                                    &mut open.playing,
                                    &mut open.marking,
                                    &mut open.new_clip,
                                ),
                                &open.path,
                            );
                        });
                    });
                egui::CentralPanel::default().show(ui, |ui| {
                    picture(ui, p, &mut open.picked, &mut open.zoom);
                });
                if let Some(changed) = f.changed {
                    let new_picture =
                        before.image != p.def.image || before.downscale != p.def.downscale;
                    if p.editing != Some(changed) || f.step {
                        p.undo.push(before);
                        p.redo.clear();
                    }
                    p.editing = (!f.step).then_some(changed);
                    p.dirty = true;
                    if new_picture {
                        reload_image(p, project);
                    }
                    rebuild(p, project);
                }
            }
        }
    }

    fn list_ui(&mut self, ui: &mut Ui, project: &Project) {
        ui.heading("Sheets");
        ui.label("New sheet from a picture:");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_image)
                    .hint_text("Art/…png")
                    .desired_width(140.0),
            );
            if ui.button("Browse…").clicked()
                && let Some(file) = rfd::FileDialog::new()
                    .add_filter("pictures", &["png"])
                    .set_directory(project.root())
                    .pick_file()
                && let Ok(inside) = file.strip_prefix(project.root())
            {
                self.new_image = inside.to_string_lossy().replace('\\', "/");
                if self.new_name.is_empty() {
                    self.new_name = inside
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    tidy_id(&mut self.new_name);
                }
            }
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_name)
                    .hint_text("name")
                    .desired_width(140.0),
            );
            tidy_id(&mut self.new_name);
            let path = format!("sheets/{}.sheet.ron", self.new_name);
            let ok = valid_id(&self.new_name)
                && !self.new_image.is_empty()
                && project.path(&self.new_image).exists()
                && !project.path(&path).exists();
            if ui.add_enabled(ok, egui::Button::new("Make")).clicked() {
                let def = SheetDef {
                    image: self.new_image.clone(),
                    downscale: 1,
                    slicing: Slicing::Grid {
                        cell: (project.settings.tile_size, project.settings.tile_size),
                        offset: (0, 0),
                        spacing: (0, 0),
                    },
                    pivot: Pivot::default(),
                    clips: Vec::new(),
                    boxes: BTreeMap::new(),
                };
                if project.save_sheet_def(&path, &def).is_ok() {
                    self.list.push(path.clone());
                    self.list.sort();
                    self.saved.push(path.clone());
                    self.open(project, &path);
                    self.new_name.clear();
                    self.new_image.clear();
                }
            }
        });
        ui.separator();
        let current = self.open.as_ref().map(|o| o.path.clone());
        let mut pick = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for path in &self.list {
                let skeleton = path.ends_with(".spine.ron");
                let broken = self.broken.contains(path);
                let mut label = if skeleton {
                    format!("{} (skeleton)", sheet_name(path))
                } else {
                    sheet_name(path).to_owned()
                };
                if broken {
                    label.push_str("  ⚠ does not load");
                }
                let chosen = current.as_deref() == Some(path.as_str());
                if ui.selectable_label(chosen, label).clicked() && !chosen {
                    pick = Some(path.clone());
                }
            }
        });
        if let Some(path) = pick {
            if self.dirty() {
                // Unsaved sheets are kept: save first (Ctrl+S) to switch.
                return;
            }
            self.open(project, &path);
        }
        if self.dirty() {
            ui.weak("Save this sheet to open another.");
        }
    }
}

/// The parts of the open sheet the settings panel changes besides the definition.
struct Parts<'a> {
    picked: &'a mut Vec<u32>,
    clip: &'a mut Option<String>,
    step: &'a mut usize,
    playing: &'a mut bool,
    marking: &'a mut Mark,
    new_clip: &'a mut String,
}

fn open_parts<'a>(
    picked: &'a mut Vec<u32>,
    clip: &'a mut Option<String>,
    step: &'a mut usize,
    playing: &'a mut bool,
    marking: &'a mut Mark,
    new_clip: &'a mut String,
) -> Parts<'a> {
    Parts {
        picked,
        clip,
        step,
        playing,
        marking,
        new_clip,
    }
}

fn load_plain(project: &Project, path: &str) -> Result<Plain, String> {
    let def = project.load_sheet_def(path).map_err(|e| e.to_string())?;
    let mut p = Plain {
        source: Image {
            width: 1,
            height: 1,
            rgba: vec![0; 4],
        },
        built: Err(String::new()),
        texture: None,
        stale_texture: true,
        undo: Vec::new(),
        redo: Vec::new(),
        editing: None,
        dirty: false,
        def,
    };
    reload_image(&mut p, project);
    rebuild(&mut p, project);
    Ok(p)
}

/// Reads the sheet's picture again (its path or shrinking changed).
fn reload_image(p: &mut Plain, project: &Project) {
    let loaded = dark_assets::load_image(&project.path(&p.def.image));
    p.source = match loaded {
        Ok(image) if (2..=16).contains(&p.def.downscale) => {
            dark_assets::downscale(&image, p.def.downscale)
        }
        Ok(image) => image,
        Err(_) => Image {
            width: 1,
            height: 1,
            rgba: vec![0; 4],
        },
    };
    p.stale_texture = true;
}

/// Cuts the sheet again as its definition says now. Circles of clips (or frames) the cut no
/// longer has are dropped first: they would stop the sheet loading.
fn rebuild(p: &mut Plain, project: &Project) {
    let path = project.path(&p.def.image);
    if !path.exists() {
        p.built = Err(format!("there is no picture {}", p.def.image));
        return;
    }
    let was_auto = matches!(&p.built, Ok((_, Some(_))));
    // Circles of clips (or frames) the cut no longer has are left out of the cut, not dropped:
    // typing a clip's name, or trying fewer frames, must not lose them.
    p.built = pruned(&p.def, &p.source, &path).build(&p.source, &path);
    let is_auto = matches!(&p.built, Ok((_, Some(_))));
    if was_auto || is_auto {
        p.stale_texture = true;
    }
}

/// The picture frames are cut from: the packed one of an automatic sheet, else the source.
fn shown(p: &Plain) -> &Image {
    match &p.built {
        Ok((_, Some(atlas))) => atlas,
        _ => &p.source,
    }
}

/// The picture with every frame outlined and numbered; clicking frames picks them.
fn picture(ui: &mut Ui, p: &mut Plain, picked: &mut Vec<u32>, zoom: &mut f32) {
    if p.stale_texture || p.texture.is_none() {
        let image = shown(p);
        p.texture = Some(ui.ctx().load_texture(
            "sheet picture",
            egui::ColorImage::from_rgba_unmultiplied(
                [image.width as usize, image.height as usize],
                &image.rgba,
            ),
            egui::TextureOptions::NEAREST,
        ));
        p.stale_texture = false;
    }
    ui.horizontal(|ui| {
        ui.label("Zoom");
        for z in [1.0, 2.0, 3.0, 4.0, 6.0] {
            ui.selectable_value(zoom, z, format!("{z}×"));
        }
        ui.separator();
        ui.weak("Click frames to pick them (in order) for a new clip; click again to drop one.");
    });
    let (w, h) = {
        let image = shown(p);
        (image.width as f32, image.height as f32)
    };
    let Some(texture) = p.texture.clone() else {
        return;
    };
    let count = p.built.as_ref().map_or(0, |(sheet, _)| sheet.frames.len());
    picked.retain(|f| (*f as usize) < count);
    let frames: Vec<Rect> = match &p.built {
        Ok((sheet, _)) => sheet.frames.iter().map(|f| f.rect).collect(),
        Err(e) => {
            ui.colored_label(Color32::from_rgb(255, 110, 100), e);
            Vec::new()
        }
    };
    egui::ScrollArea::both().show(ui, |ui| {
        let size = vec2(w, h) * *zoom;
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let painter = ui.painter_at(rect);
        // A checkerboard behind, so transparent pixels read as such.
        painter.rect_filled(rect, 0.0, Color32::from_gray(40));
        painter.image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        let at = |r: &Rect| {
            egui::Rect::from_min_size(
                rect.min + vec2(r.x as f32, r.y as f32) * *zoom,
                vec2(r.w as f32, r.h as f32) * *zoom,
            )
        };
        for (i, r) in frames.iter().enumerate() {
            let order = picked.iter().position(|f| *f == i as u32);
            let stroke = match order {
                Some(_) => Stroke::new(2.0, Color32::from_rgb(255, 220, 60)),
                None => Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 200, 255, 140)),
            };
            let box_ = at(r);
            painter.rect_stroke(box_, 0.0, stroke, StrokeKind::Inside);
            let label = match order {
                Some(n) => format!("{i} ({})", n + 1),
                None => i.to_string(),
            };
            painter.text(
                box_.left_top() + vec2(2.0, 1.0),
                egui::Align2::LEFT_TOP,
                label,
                egui::FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
        if response.clicked()
            && let Some(click) = response.interact_pointer_pos()
        {
            let texel = (click - rect.min) / *zoom;
            if let Some(i) = frames.iter().position(|r| {
                texel.x >= r.x as f32
                    && texel.y >= r.y as f32
                    && texel.x < (r.x + r.w) as f32
                    && texel.y < (r.y + r.h) as f32
            }) {
                let i = i as u32;
                match picked.iter().position(|f| *f == i) {
                    Some(n) => {
                        picked.remove(n);
                    }
                    None => picked.push(i),
                }
            }
        }
    });
}

fn settings(ui: &mut Ui, f: &mut Tracker, p: &mut Plain, parts: Parts, path: &str) {
    ui.heading(sheet_name(path));
    ui.horizontal(|ui| {
        ui.label("Picture");
        f.track(ui.text_edit_singleline(&mut p.def.image));
    });
    f.number(ui, "Drawn at", &mut p.def.downscale, 1..=16, "The art is drawn this many times its pixel size (RPG Maker MZ art is often 3): it is shrunk back first");
    ui.separator();
    slicing(ui, f, &mut p.def.slicing);
    pivot(ui, f, &mut p.def.pivot);
    ui.separator();
    match &p.built {
        Ok((sheet, _)) => {
            ui.label(format!(
                "{} frames, {} clips.",
                sheet.frames.len(),
                sheet.clips.len()
            ));
        }
        Err(e) => {
            ui.colored_label(Color32::from_rgb(255, 110, 100), e);
        }
    }
    clips(ui, f, p, parts);
}

fn slicing(ui: &mut Ui, f: &mut Tracker, s: &mut Slicing) {
    let kinds = [
        "A grid",
        "An RPG Maker character",
        "Find each sprite",
        "Rectangles by hand",
        "Directions × actions",
    ];
    let kind = match s {
        Slicing::Grid { .. } => 0,
        Slicing::Character { .. } => 1,
        Slicing::Auto(_) => 2,
        Slicing::Manual(_) => 3,
        Slicing::Directional { .. } => 4,
    };
    let mut chosen = kind;
    ui.horizontal(|ui| {
        ui.label("Cut as");
        ComboBox::from_id_salt("slicing")
            .selected_text(kinds[kind])
            .show_ui(ui, |ui| {
                for (i, k) in kinds.iter().enumerate() {
                    ui.selectable_value(&mut chosen, i, *k);
                }
            });
    });
    if chosen != kind {
        *s = match chosen {
            0 => Slicing::Grid {
                cell: (16, 16),
                offset: (0, 0),
                spacing: (0, 0),
            },
            1 => Slicing::Character {
                layout: None,
                index: 0,
                walk_ticks: 9,
            },
            2 => Slicing::Auto(AutoSlice::default()),
            3 => Slicing::Manual(Vec::new()),
            _ => Slicing::Directional {
                cell: (32, 32),
                directions: Facing::CARDINAL.to_vec(),
                actions: vec![ActionDef {
                    name: "idle".into(),
                    frames: 1,
                    ticks_per_frame: 10,
                    looping: true,
                }],
            },
        };
        f.step("slicing kind");
    }
    ui.indent("slicing", |ui| match s {
        Slicing::Grid {
            cell,
            offset,
            spacing,
        } => {
            pair(ui, f, "Cell", cell, 1);
            pair(ui, f, "Offset", offset, 0);
            pair(ui, f, "Gap", spacing, 0);
        }
        Slicing::Character {
            layout,
            index,
            walk_ticks,
        } => {
            let mut own = layout.is_some();
            if f.track(ui.checkbox(
                &mut own,
                "Say the layout (otherwise the file name tells it)",
            ))
            .changed()
            {
                *layout = own.then_some(CharacterLayout {
                    characters: (4, 2),
                    frames: 3,
                });
            }
            if let Some(l) = layout {
                pair(ui, f, "Characters across, down", &mut l.characters, 1);
                f.number(ui, "Frames each way", &mut l.frames, 1..=32, "");
            }
            f.number(
                ui,
                "Which character",
                index,
                0..=63,
                "Counting across, then down, from 0",
            );
            f.number(
                ui,
                "Walk speed",
                walk_ticks,
                1..=120,
                "Ticks (60 a second) per walking frame",
            );
        }
        Slicing::Auto(a) => {
            f.number(
                ui,
                "Solid above",
                &mut a.alpha_threshold,
                0..=254,
                "Pixels more opaque than this belong to a sprite",
            );
            f.number(
                ui,
                "Join within",
                &mut a.merge_distance,
                0..=64,
                "Pieces this close are one sprite",
            );
            f.number(
                ui,
                "Smallest",
                &mut a.min_size,
                0..=256,
                "Specks smaller than this are dropped",
            );
        }
        Slicing::Manual(rects) => {
            ui.weak("One rectangle per frame, in texels.");
            let mut remove = None;
            for (i, r) in rects.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("{i}"));
                    f.track(ui.add(egui::DragValue::new(&mut r.x).prefix("x ")));
                    f.track(ui.add(egui::DragValue::new(&mut r.y).prefix("y ")));
                    f.track(ui.add(egui::DragValue::new(&mut r.w).prefix("w ").range(1..=4096)));
                    f.track(ui.add(egui::DragValue::new(&mut r.h).prefix("h ").range(1..=4096)));
                    if ui.small_button("✖").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                rects.remove(i);
                f.step(("remove rect", i));
            }
            if ui.button("Add a rectangle").clicked() {
                let last = rects.last().copied().unwrap_or(Rect::new(0, 0, 16, 16));
                rects.push(Rect::new(last.x + last.w, last.y, last.w, last.h));
                f.step(("add rect", rects.len()));
            }
        }
        Slicing::Directional {
            cell,
            directions,
            actions,
        } => {
            pair(ui, f, "Cell", cell, 1);
            ui.label("Rows, top to bottom, face:");
            let mut remove = None;
            for (i, d) in directions.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    let before = *d;
                    ComboBox::from_id_salt(("row", i))
                        .selected_text(d.name())
                        .show_ui(ui, |ui| {
                            for facing in Facing::ALL {
                                ui.selectable_value(d, facing, facing.name());
                            }
                        });
                    if *d != before {
                        f.mark(("row", i));
                    }
                    if ui.small_button("✖").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                directions.remove(i);
                f.step(("remove row", i));
            }
            if ui.small_button("+ row").clicked() {
                directions.push(Facing::Down);
                f.step(("add row", directions.len()));
            }
            ui.label("Columns, left to right, show:");
            let mut remove = None;
            for (i, a) in actions.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    f.track(ui.add(egui::TextEdit::singleline(&mut a.name).desired_width(70.0)));
                    f.track(
                        ui.add(
                            egui::DragValue::new(&mut a.frames)
                                .range(1..=64)
                                .suffix(" frames"),
                        ),
                    );
                    f.track(
                        ui.add(
                            egui::DragValue::new(&mut a.ticks_per_frame)
                                .range(1..=120)
                                .suffix(" ticks"),
                        ),
                    );
                    f.track(ui.checkbox(&mut a.looping, "loops"));
                    if ui.small_button("✖").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                actions.remove(i);
                f.step(("remove action", i));
            }
            if ui.small_button("+ action").clicked() {
                actions.push(ActionDef {
                    name: "walk".into(),
                    frames: 4,
                    ticks_per_frame: 8,
                    looping: true,
                });
                f.step(("add action", actions.len()));
            }
        }
    });
}

fn pair(ui: &mut Ui, f: &mut Tracker, label: &str, value: &mut (u32, u32), least: u32) {
    ui.horizontal(|ui| {
        ui.label(label);
        f.track(ui.add(egui::DragValue::new(&mut value.0).range(least..=4096)));
        f.track(ui.add(egui::DragValue::new(&mut value.1).range(least..=4096)));
    });
}

fn pivot(ui: &mut Ui, f: &mut Tracker, pivot: &mut Pivot) {
    let kinds = [
        "Feet (the bottom middle)",
        "The middle",
        "Where its opaque pixels stand",
        "A point",
    ];
    let kind = match pivot {
        Pivot::BottomCenter => 0,
        Pivot::Center => 1,
        Pivot::Footprint => 2,
        Pivot::Pixel(..) => 3,
    };
    let mut chosen = kind;
    ui.horizontal(|ui| {
        ui.label("Stands on")
            .on_hover_text("The point of each frame that is put on the ground");
        ComboBox::from_id_salt("pivot")
            .selected_text(kinds[kind])
            .show_ui(ui, |ui| {
                for (i, k) in kinds.iter().enumerate() {
                    ui.selectable_value(&mut chosen, i, *k);
                }
            });
    });
    if chosen != kind {
        *pivot = match chosen {
            0 => Pivot::BottomCenter,
            1 => Pivot::Center,
            2 => Pivot::Footprint,
            _ => Pivot::Pixel(8.0, 16.0),
        };
        f.step("pivot kind");
    }
    if let Pivot::Pixel(x, y) = pivot {
        ui.horizontal(|ui| {
            ui.label("Point");
            f.track(ui.add(egui::DragValue::new(x).prefix("x ")));
            f.track(ui.add(egui::DragValue::new(y).prefix("y ")));
        });
    }
}

/// The clips: those the slicing makes, and the sheet's own (which can be changed); and the
/// chosen clip played, with its boxes.
fn clips(ui: &mut Ui, f: &mut Tracker, p: &mut Plain, parts: Parts) {
    let Ok((sheet, _)) = &p.built else {
        return;
    };
    ui.heading("Clips");
    let own: Vec<String> = p.def.clips.iter().map(|c| c.name.clone()).collect();
    ui.horizontal_wrapped(|ui| {
        for clip in &sheet.clips {
            let mut label = clip.name.clone();
            if p.def.boxes.contains_key(&clip.name) {
                label.push_str(" ★");
            }
            let chosen = parts.clip.as_deref() == Some(clip.name.as_str());
            if ui.selectable_label(chosen, label).clicked() {
                *parts.clip = Some(clip.name.clone());
                *parts.step = 0;
            }
        }
    });
    ui.weak("★ has its own hit and hurt circles.");
    // A new clip of the picked frames.
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(parts.new_clip)
                .hint_text("new clip, e.g. attack_down")
                .desired_width(170.0),
        );
        tidy_id(parts.new_clip);
        let ok = valid_id(parts.new_clip)
            && !parts.picked.is_empty()
            && !sheet.clips.iter().any(|c| c.name == *parts.new_clip);
        if ui
            .add_enabled(
                ok,
                egui::Button::new(format!("Make of {} picked", parts.picked.len())),
            )
            .clicked()
        {
            p.def.clips.push(ClipDef {
                name: std::mem::take(parts.new_clip),
                frames: std::mem::take(parts.picked),
                ticks_per_frame: 6,
                looping: false,
                flip_x: false,
                pivot: None,
            });
            f.step(("new clip", p.def.clips.len()));
        }
    });
    let Some(name) = parts.clip.clone() else {
        return;
    };
    let Some(clip) = sheet.clips.iter().find(|c| c.name == name).cloned() else {
        *parts.clip = None;
        return;
    };
    ui.separator();
    // The sheet's own clip can be changed; a slicing's is as the slicing makes it.
    if let Some(def) = p.def.clips.iter_mut().find(|c| c.name == name) {
        ui.label(format!("Frames: {:?}", def.frames));
        f.number(
            ui,
            "Ticks a frame",
            &mut def.ticks_per_frame,
            1..=120,
            "60 ticks a second",
        );
        f.check(ui, "Loops", &mut def.looping, "");
        f.check(
            ui,
            "Mirrored",
            &mut def.flip_x,
            "A right-facing clip from left-facing art",
        );
        let mut remove = false;
        ui.horizontal(|ui| {
            if ui.button("Use the picked frames").clicked() && !parts.picked.is_empty() {
                def.frames = parts.picked.clone();
                f.step(("clip frames", &name));
            }
            if ui.button("Delete this clip").clicked() {
                remove = true;
            }
        });
        if remove {
            p.def.clips.retain(|c| c.name != name);
            p.def.boxes.remove(&name);
            *parts.clip = None;
            f.step(("delete clip", &name));
            return;
        }
    } else if own.is_empty() {
        ui.weak("Made by the slicing; its frames follow the cut.");
    }
    let frames = clip.frames.len().max(1);
    *parts.step = (*parts.step).min(frames - 1);
    ui.horizontal(|ui| {
        if ui.button("◀").clicked() {
            *parts.step = (*parts.step + frames - 1) % frames;
        }
        ui.label(format!("frame {} of {}", *parts.step + 1, frames));
        if ui.button("▶").clicked() {
            *parts.step = (*parts.step + 1) % frames;
        }
        ui.checkbox(parts.playing, "Play");
    });
    if *parts.playing {
        // One frame per `ticks_per_frame` sixtieths of a second.
        let t = ui.input(|i| i.time);
        let per = clip.ticks_per_frame.max(1) as f64 / 60.0;
        *parts.step = ((t / per) as usize) % frames;
        ui.ctx().request_repaint();
    }
    // The frame, large, standing on its feet, with its circles.
    let Some(&index) = clip.frames.get(*parts.step) else {
        return;
    };
    let Some(frame) = sheet.frames.get(index as usize).copied() else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label("Clicking the frame places:");
        ui.selectable_value(parts.marking, Mark::Hit, "where it hits");
        ui.selectable_value(parts.marking, Mark::Hurt, "where it can be hit");
    });
    // Large, but within the panel.
    let longest = frame.rect.w.max(frame.rect.h).max(1) as f32;
    let zoom = (300.0 / longest).clamp(1.0, 6.0);
    let pad = 24.0;
    let size = vec2(frame.rect.w as f32, frame.rect.h as f32) * zoom + vec2(pad, pad) * 2.0;
    let (area, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 4.0, Color32::from_gray(45));
    let image = shown(p);
    let tex_size = vec2(image.width as f32, image.height as f32);
    let picture = egui::Rect::from_min_size(
        area.min + vec2(pad, pad),
        vec2(frame.rect.w as f32, frame.rect.h as f32) * zoom,
    );
    let uv = egui::Rect::from_min_size(
        pos2(
            frame.rect.x as f32 / tex_size.x,
            frame.rect.y as f32 / tex_size.y,
        ),
        vec2(
            frame.rect.w as f32 / tex_size.x,
            frame.rect.h as f32 / tex_size.y,
        ),
    );
    let uv = if clip.flip_x {
        egui::Rect::from_min_max(pos2(uv.max.x, uv.min.y), pos2(uv.min.x, uv.max.y))
    } else {
        uv
    };
    if let Some(texture) = &p.texture {
        painter.image(texture.id(), picture, uv, Color32::WHITE);
    }
    let pivot = if clip.flip_x {
        vec2(frame.rect.w as f32 - frame.pivot.x, frame.pivot.y)
    } else {
        vec2(frame.pivot.x, frame.pivot.y)
    };
    let feet: Pos2 = picture.min + pivot * zoom;
    painter.circle_stroke(
        feet,
        3.0,
        Stroke::new(1.5, Color32::from_rgb(120, 230, 120)),
    );
    let boxes = p.def.boxes.get(&name).cloned().unwrap_or_default();
    let draw = |list: &[Option<(f32, f32, f32)>], color: Color32| {
        if let Some(Some((x, y, r))) = list.get(*parts.step) {
            painter.circle_stroke(
                feet + vec2(*x, *y) * zoom,
                r * zoom,
                Stroke::new(2.0, color),
            );
        }
    };
    let hit_color = Color32::from_rgb(255, 90, 80);
    let hurt_color = Color32::from_rgb(90, 180, 255);
    draw(&boxes.hitboxes, hit_color);
    draw(&boxes.hurtboxes, hurt_color);
    // Press where the circle goes, drag out its size: one undo step per press.
    let press = ui
        .input(|i| i.pointer.press_start_time())
        .unwrap_or_default()
        .to_bits();
    if let Some(at) = response.interact_pointer_pos()
        && (response.drag_started() || response.clicked())
    {
        let centre = (at - feet) / zoom;
        let entry = p.def.boxes.entry(name.clone()).or_default();
        let list = match parts.marking {
            Mark::Hit => &mut entry.hitboxes,
            Mark::Hurt => &mut entry.hurtboxes,
        };
        if list.len() <= *parts.step {
            list.resize(*parts.step + 1, None);
        }
        let radius = list[*parts.step].map_or(6.0, |(_, _, r)| r);
        list[*parts.step] = Some((centre.x.round(), centre.y.round(), radius));
        f.mark(("box", &name, *parts.step, press));
    }
    if response.dragged()
        && let (Some(at), Some(entry)) =
            (response.interact_pointer_pos(), p.def.boxes.get_mut(&name))
    {
        let list = match parts.marking {
            Mark::Hit => &mut entry.hitboxes,
            Mark::Hurt => &mut entry.hurtboxes,
        };
        if let Some(Some((x, y, r))) = list.get_mut(*parts.step) {
            let centre = feet + vec2(*x, *y) * zoom;
            *r = ((at - centre).length() / zoom).round().max(1.0);
            f.mark(("box", &name, *parts.step, press));
        }
    }
    ui.horizontal(|ui| {
        let step = *parts.step;
        if ui.button("None on this frame").clicked()
            && let Some(entry) = p.def.boxes.get_mut(&name)
        {
            let list = match parts.marking {
                Mark::Hit => &mut entry.hitboxes,
                Mark::Hurt => &mut entry.hurtboxes,
            };
            if let Some(slot) = list.get_mut(step) {
                *slot = None;
            }
            trim(entry);
            if entry.hitboxes.is_empty() && entry.hurtboxes.is_empty() {
                p.def.boxes.remove(&name);
            }
            f.step(("clear box", &name, step));
        }
        if step > 0
            && ui.button("As the frame before").clicked()
            && let Some(entry) = p.def.boxes.get_mut(&name)
        {
            let list = match parts.marking {
                Mark::Hit => &mut entry.hitboxes,
                Mark::Hurt => &mut entry.hurtboxes,
            };
            if list.len() <= step {
                list.resize(step + 1, None);
            }
            list[step] = list[step - 1];
            f.step(("copy box", &name, step));
        }
        if p.def.boxes.contains_key(&name) && ui.button("Remove all its circles").clicked() {
            p.def.boxes.remove(&name);
            f.step(("remove boxes", &name));
        }
    });
    ui.weak("Green: the feet. Red: where it hits (only frames with one hit, and a clip with any uses them instead of its moveset's). Blue: where it can be hit.");
}

/// `def` without the circles its cut has no clip or frame for.
fn pruned(def: &SheetDef, source: &Image, path: &std::path::Path) -> SheetDef {
    let mut def = def.clone();
    if def.boxes.is_empty() {
        return def;
    }
    let bare = SheetDef {
        boxes: BTreeMap::new(),
        ..def.clone()
    };
    if let Ok((sheet, _)) = bare.build(source, path) {
        prune_boxes(&mut def.boxes, &sheet);
    }
    def
}

/// Keeps only the circles of clips `sheet` has, and of frames those clips have.
fn prune_boxes(boxes: &mut BTreeMap<String, ClipBoxes>, sheet: &SpriteSheet) {
    boxes.retain(|name, b| {
        let Some(id) = sheet.clip_id(name) else {
            return false;
        };
        let frames = sheet.clips[usize::from(id.0)].frames.len();
        b.hitboxes.truncate(frames);
        b.hurtboxes.truncate(frames);
        trim(b);
        !(b.hitboxes.is_empty() && b.hurtboxes.is_empty())
    });
}

/// Drops trailing empty frames, so the file lists only what is there.
fn trim(boxes: &mut ClipBoxes) {
    for list in [&mut boxes.hitboxes, &mut boxes.hurtboxes] {
        while list.last() == Some(&None) {
            list.pop();
        }
    }
}

fn spine_ui(ui: &mut Ui, project: &Project, path: &str, report: &mut Option<String>) {
    ui.heading(format!("{} (a skeleton)", sheet_name(path)));
    ui.label("Skeletons come from Spine (4.1 JSON exports). The game measures them ahead of time: bake again after every export.");
    let state = match (project.load_spine_def(path), project.load_sheet(path)) {
        (Ok(def), Ok(loaded)) => match dark_spine::Rig::load(project, &def) {
            Ok(rig) => {
                let fresh = loaded
                    .spine
                    .as_ref()
                    .is_some_and(|s| s.bake.skeleton_hash == rig.hash());
                if fresh {
                    "The bake is up to date.".to_owned()
                } else {
                    "The skeleton changed since it was baked: bake it again.".to_owned()
                }
            }
            Err(e) => format!("The skeleton does not load: {e}"),
        },
        (Err(e), _) | (_, Err(e)) => format!("{e}"),
    };
    ui.label(state);
    if ui.button("Bake").clicked() {
        *report = Some(match bake(project, path) {
            Ok(done) => done,
            Err(e) => format!("Not baked: {e}"),
        });
    }
    if let Some(report) = report {
        ui.label(report.as_str());
    }
}

/// Measures the skeleton at `path` for the host and writes its baked file.
pub fn bake(project: &Project, path: &str) -> Result<String, String> {
    let def = project.load_spine_def(path).map_err(|e| e.to_string())?;
    let bake = dark_spine::bake(project, &def).map_err(|e| e.to_string())?;
    bake.write(&project.path(&def.baked))
        .map_err(|e| e.to_string())?;
    let boxed = bake
        .clips
        .values()
        .filter(|c| !c.hitboxes.is_empty() || !c.hurtboxes.is_empty())
        .count();
    Ok(format!(
        "Baked {} clips ({boxed} with boxes).",
        bake.clips.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circles_the_cut_no_longer_has_are_dropped() {
        let mut sheet = SpriteSheet::default();
        sheet.add_clip(dark_sprite::Clip {
            name: "swing".into(),
            frames: vec![0, 1],
            ticks_per_frame: 4,
            looping: false,
            flip_x: false,
        });
        let circle = Some((0.0, 8.0, 4.0));
        let mut boxes = BTreeMap::from([
            (
                "swing".to_owned(),
                ClipBoxes {
                    hitboxes: vec![None, circle, circle, circle],
                    hurtboxes: Vec::new(),
                },
            ),
            (
                "gone".to_owned(),
                ClipBoxes {
                    hitboxes: vec![circle],
                    hurtboxes: Vec::new(),
                },
            ),
        ]);
        prune_boxes(&mut boxes, &sheet);
        assert_eq!(boxes.len(), 1, "the clip that went took its circles");
        assert_eq!(
            boxes["swing"].hitboxes,
            vec![None, circle],
            "cut to its two frames"
        );
    }

    #[test]
    fn trailing_empty_frames_are_dropped() {
        let mut boxes = ClipBoxes {
            hitboxes: vec![None, Some((0.0, 1.0, 2.0)), None, None],
            hurtboxes: vec![None],
        };
        trim(&mut boxes);
        assert_eq!(boxes.hitboxes.len(), 2);
        assert!(boxes.hurtboxes.is_empty());
    }
}

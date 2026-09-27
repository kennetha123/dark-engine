//! The Effects workspace (`journals/engine/05` phase 3): a form for a `*.fx.ron` and the effect
//! playing beside it.
//!
//! **This is the tool.** Not a node graph — a panel of numbers that shows the result. The result
//! is drawn by the game's own renderer through the game's own [`dark_fx::Effects`], not by egui
//! shapes approximating one, because a tool whose whole promise is *this is what you will see*
//! cannot afford a second implementation that drifts from the first.

use dark_assets::Project;
use dark_fx::{Art, EffectDef, EffectId, Effects, Handle, Range};
use dark_render::Mesh;
use egui::{Color32, Ui};
use glam::Vec2;

use crate::catalog::fx_name;
use crate::viewport::Viewport;
use crate::widgets::{Tracker, tidy_id};

/// Backgrounds to judge a particle against, **as they should look**. A pale particle on a pale
/// ground cannot be seen at all, and an artist should not have to save and run the game to find
/// that out.
const GROUNDS: [(&str, [f64; 3]); 4] = [
    ("Night", [0.05, 0.06, 0.09]),
    ("Dusk", [0.22, 0.20, 0.26]),
    ("Grass", [0.32, 0.46, 0.26]),
    ("Snow", [0.80, 0.83, 0.88]),
];

/// A colour as it should look, turned into the linear one the renderer clears to. The surface is
/// sRGB, so handing it 0.22 directly would paint a dusk that is half way to daylight.
fn linear(srgb: [f64; 3]) -> [f64; 3] {
    srgb.map(|c| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    })
}

/// An effect being edited.
struct Open {
    path: String,
    def: EffectDef,
    /// The sheet its frames come from, once it has been found; `Err` says why it has not.
    art: Result<Art, String>,
    /// The runtime the preview plays, and this effect in it.
    fx: Effects,
    kind: Option<EffectId>,
    playing: Option<Handle>,
    undo: Vec<EffectDef>,
    redo: Vec<EffectDef>,
    editing: Option<egui::Id>,
    dirty: bool,
    /// The definition the runtime was last given; the preview is retuned when they differ.
    tuned: Option<EffectDef>,
}

pub struct EffectEditor {
    pub list: Vec<String>,
    open: Option<Box<Open>>,
    new_name: String,
    new_sheet: String,
    ground: usize,
    running: bool,
    zoom: u32,
    /// How large the preview's picture should be, in its own pixels; `render` draws that.
    wanted: (u32, u32),
    /// The viewport's rendered target, as egui knows it.
    image: egui::TextureId,
    /// Seconds the last frame took.
    dt: f32,
    /// Effects written since the editor last asked.
    pub saved: Vec<String>,
}

impl EffectEditor {
    pub fn new(list: Vec<String>, image: egui::TextureId) -> Self {
        Self {
            list,
            open: None,
            new_name: String::new(),
            new_sheet: String::new(),
            ground: 1,
            running: true,
            zoom: 2,
            wanted: (1, 1),
            image,
            dt: 0.0,
            saved: Vec::new(),
        }
    }

    pub fn dirty(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.dirty)
    }

    pub fn can_undo(&self) -> bool {
        self.open.as_ref().is_some_and(|o| !o.undo.is_empty())
    }

    pub fn can_redo(&self) -> bool {
        self.open.as_ref().is_some_and(|o| !o.redo.is_empty())
    }

    pub fn undo_step(&mut self) {
        if let Some(open) = &mut self.open
            && let Some(before) = open.undo.pop()
        {
            open.redo.push(std::mem::replace(&mut open.def, before));
            open.dirty = true;
            open.editing = None;
        }
    }

    pub fn redo_step(&mut self) {
        if let Some(open) = &mut self.open
            && let Some(after) = open.redo.pop()
        {
            open.undo.push(std::mem::replace(&mut open.def, after));
            open.dirty = true;
            open.editing = None;
        }
    }

    /// Writes the open effect if it changed.
    pub fn save(&mut self, project: &Project) -> Result<Option<String>, String> {
        let Some(open) = &mut self.open else {
            return Ok(None);
        };
        if !open.dirty {
            return Ok(None);
        }
        open.def
            .save(project, &open.path)
            .map_err(|e| e.to_string())?;
        open.dirty = false;
        self.saved.push(open.path.clone());
        Ok(Some(format!("Saved {}.", fx_name(&open.path))))
    }

    fn open(&mut self, project: &Project, path: &str) {
        let def = match EffectDef::load(project, path) {
            Ok(def) => def,
            Err(err) => {
                // Shown as an unopenable file rather than silently doing nothing.
                self.open = None;
                self.new_name = format!("{err}");
                return;
            }
        };
        self.open = Some(Box::new(Open {
            path: path.to_owned(),
            def,
            art: Err(String::new()),
            fx: Effects::new(),
            kind: None,
            playing: None,
            undo: Vec::new(),
            redo: Vec::new(),
            editing: None,
            dirty: false,
            tuned: None,
        }));
    }

    // --- The interface ---------------------------------------------------------------------

    pub fn ui(&mut self, ui: &mut Ui, project: &Project) {
        self.tick(ui);
        egui::Panel::left("effect list")
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| self.list_panel(ui, project));
        egui::Panel::right("effect form")
            .resizable(true)
            .default_size(320.0)
            .min_size(300.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.form(ui, project));
            });
        egui::CentralPanel::default().show(ui, |ui| self.preview_panel(ui));
    }

    /// How long the last frame took, remembered while there is a context to ask. `render` runs
    /// after the interface is laid out and has none of its own.
    fn tick(&mut self, ui: &Ui) {
        // Clamped: a window that was not drawn for a while must not advance the effect by a
        // minute the moment it comes back.
        self.dt = ui.ctx().input(|i| i.stable_dt).clamp(0.0, 1.0 / 15.0);
    }

    fn list_panel(&mut self, ui: &mut Ui, project: &Project) {
        ui.add_space(4.0);
        ui.heading("Effects");
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            let list = self.list.clone();
            for path in &list {
                let chosen = self.open.as_ref().is_some_and(|o| &o.path == path);
                if ui.selectable_label(chosen, fx_name(path)).clicked() && !chosen {
                    self.open(project, path);
                }
            }
        });
        ui.separator();
        ui.label("New effect");
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut self.new_name);
        });
        ui.horizontal(|ui| {
            ui.label("Sheet");
            ui.text_edit_singleline(&mut self.new_sheet);
        })
        .response
        .on_hover_text("The `*.sheet.ron` its pictures come from");
        let mut name = self.new_name.clone();
        tidy_id(&mut name);
        let ready = !name.is_empty() && self.new_sheet.ends_with(".sheet.ron");
        if ui
            .add_enabled(ready, egui::Button::new("Make it"))
            .on_hover_text("Writes fx/<name>.fx.ron and opens it")
            .clicked()
        {
            let path = format!("fx/{name}.fx.ron");
            let def = EffectDef::plain(std::mem::take(&mut self.new_sheet));
            match def.save(project, &path) {
                Ok(()) => {
                    self.list.push(path.clone());
                    self.list.sort();
                    self.saved.push(path.clone());
                    self.new_name.clear();
                    self.open(project, &path);
                }
                Err(err) => self.new_name = err.to_string(),
            }
        }
    }

    fn form(&mut self, ui: &mut Ui, project: &Project) {
        let Some(open) = &mut self.open else {
            ui.add_space(8.0);
            ui.label("Pick an effect, or make one.");
            return;
        };
        let before = open.def.clone();
        let mut tracker = Tracker::default();
        ui.add_space(4.0);
        ui.heading(fx_name(&open.path));
        if let Err(why) = &open.art
            && !why.is_empty()
        {
            ui.colored_label(Color32::from_rgb(220, 120, 110), why);
        }
        ui.separator();

        ui.label("Pictures");
        tracker.text(
            ui,
            "Sheet",
            &mut open.def.sheet,
            "The `*.sheet.ron` its pictures come from",
        );
        frames(ui, &mut tracker, &mut open.def.frames);

        ui.separator();
        ui.label("How many, and for how long");
        tracker.number(
            ui,
            "Rate",
            &mut open.def.rate,
            0.0..=4000.0,
            "Born a second",
        );
        tracker.number(
            ui,
            "Burst",
            &mut open.def.burst,
            0..=4000,
            "Born at once when it starts. With no rate, that makes it a one-shot",
        );
        range(
            ui,
            &mut tracker,
            "Life",
            &mut open.def.life,
            0.0..=60.0,
            "Seconds one lives",
        );
        tracker.number(
            ui,
            "Cap",
            &mut open.def.cap,
            1..=4000,
            "At most this many alive at once; a rate asking for more drops the newcomers",
        );

        ui.separator();
        ui.label("Where they start");
        tracker.number(
            ui,
            "Area across",
            &mut open.def.area.0,
            0.0..=2000.0,
            "Half-width",
        );
        tracker.number(
            ui,
            "Area down",
            &mut open.def.area.1,
            0.0..=2000.0,
            "Half-height",
        );
        range(
            ui,
            &mut tracker,
            "Lift",
            &mut open.def.lift,
            0.0..=1000.0,
            "How high off the ground",
        );

        ui.separator();
        ui.label("Where they go, in pixels a second");
        range(
            ui,
            &mut tracker,
            "Across",
            &mut open.def.drift.x,
            -1000.0..=1000.0,
            "",
        );
        range(
            ui,
            &mut tracker,
            "Up",
            &mut open.def.drift.lift,
            -1000.0..=1000.0,
            "",
        );
        range(
            ui,
            &mut tracker,
            "Down the map",
            &mut open.def.drift.y,
            -1000.0..=1000.0,
            "",
        );
        tracker.number(
            ui,
            "Gravity",
            &mut open.def.gravity,
            -2000.0..=2000.0,
            "Pulls lift down, pixels a second squared. Negative falls",
        );
        tracker.number(
            ui,
            "Drag",
            &mut open.def.drag,
            0.0..=1.0,
            "How much of its speed a particle loses a second",
        );
        tracker.check(
            ui,
            "Lands",
            &mut open.def.lands,
            "Gone when it reaches the ground. Rain lands; smoke does not",
        );

        ui.separator();
        ui.label("How they look");
        tracker.number(ui, "Size at birth", &mut open.def.size.from, 0.0..=16.0, "");
        tracker.number(ui, "Size at death", &mut open.def.size.to, 0.0..=16.0, "");
        range(
            ui,
            &mut tracker,
            "Spin",
            &mut open.def.spin,
            -8.0..=8.0,
            "Turns a second",
        );
        colour(
            ui,
            &mut tracker,
            "Colour at birth",
            &mut open.def.colour.from,
        );
        colour(ui, &mut tracker, "Colour at death", &mut open.def.colour.to);
        tracker.number(
            ui,
            "Layer",
            &mut open.def.layer,
            -1000..=1000,
            "Where it sits in the world's order: 0 among what stands, 100 above it",
        );

        if tracker.changed.is_some() && open.def != before {
            // One undo step for a whole drag, as everywhere else in the editor.
            if open.editing != tracker.changed || tracker.step {
                open.undo.push(before);
                open.redo.clear();
                open.editing = tracker.changed;
            }
            open.dirty = true;
        }
        let _ = project;
    }

    fn preview_panel(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.toggle_value(&mut self.running, "Playing")
                .on_hover_text("Stops the clock, to look at one moment");
            if ui
                .button("Burst")
                .on_hover_text("Plays it once from the start, as a one-shot")
                .clicked()
                && let Some(open) = &mut self.open
                && let Some(kind) = open.kind
            {
                if let Some(handle) = open.playing.take() {
                    open.fx.stop(handle);
                }
                open.fx.burst(kind, Vec2::ZERO);
            }
            ui.separator();
            for (n, (name, _)) in GROUNDS.iter().enumerate() {
                ui.selectable_value(&mut self.ground, n, *name);
            }
        });
        ui.horizontal(|ui| {
            if let Some(open) = &self.open {
                let (alive, cap) = (open.fx.count(), open.def.cap);
                let wanted = open.def.wanted();
                if wanted > cap as f32 {
                    ui.colored_label(
                        Color32::from_rgb(230, 190, 110),
                        format!(
                            "{alive} alive — it asks for about {wanted:.0} and its cap is {cap}"
                        ),
                    )
                    .on_hover_text("Raise the cap, or lower the rate or the lifetime");
                } else {
                    ui.label(format!("{alive} alive, cap {cap}"));
                }
            }
            ui.separator();
            ui.label("Zoom");
            for n in 1..=4u32 {
                ui.selectable_value(&mut self.zoom, n, format!("{n}×"));
            }
        });
        ui.separator();

        // The picture itself, drawn into by `render` after the interface is laid out — the same
        // way the map view is, and through the same renderer.
        let rect = ui.available_rect_before_wrap();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(12, 12, 14));
        if self.open.is_none() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Pick an effect on the left, or make one.",
                egui::FontId::proportional(18.0),
                Color32::GRAY,
            );
            self.wanted = (1, 1);
            return;
        }
        let ppp = ui.ctx().pixels_per_point();
        let zoom = self.zoom.max(1) as f32;
        let size = (
            ((rect.width() * ppp) / zoom).ceil().max(1.0) as u32,
            ((rect.height() * ppp) / zoom).ceil().max(1.0) as u32,
        );
        self.wanted = size;
        let shown = egui::Rect::from_min_size(
            rect.min,
            egui::vec2(size.0 as f32, size.1 as f32) * (zoom / ppp),
        );
        painter.image(
            self.image,
            shown,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        // Redrawn every frame while it plays, or the effect would only move when the mouse does.
        if self.running {
            ui.ctx().request_repaint();
        }
    }

    // --- Drawing ---------------------------------------------------------------------------

    /// Advances the effect and hands its meshes to the viewport's renderer. Called once a frame,
    /// after the interface has been laid out, as the map view is.
    pub fn render(
        &mut self,
        project: &Project,
        viewport: &mut Viewport,
        egui: &mut egui_wgpu::Renderer,
    ) {
        let (size, dt) = (self.wanted, self.dt);
        let Some(open) = &mut self.open else {
            return;
        };
        open.art = art_for(project, viewport, &open.def);
        let Ok(art) = &open.art else {
            // Nothing to draw: clear to the chosen ground so the panel is not a stale picture.
            viewport.render_meshes(egui, size, Vec2::ZERO, linear(GROUNDS[self.ground].1), &[]);
            return;
        };
        // The runtime is taught the effect once, then retuned whenever a number changes, so
        // dragging one alters the fall being watched instead of starting it over.
        let changed = open.tuned.as_ref() != Some(&open.def);
        match (open.kind, changed) {
            (None, _) => {
                if let Ok(kind) = open.fx.add(open.def.clone(), art.clone()) {
                    open.kind = Some(kind);
                    open.tuned = Some(open.def.clone());
                    open.playing = Some(open.fx.standing(kind, Vec2::ZERO));
                }
            }
            (Some(kind), true) => {
                if open.fx.retune(kind, open.def.clone(), art.clone()).is_ok() {
                    open.tuned = Some(open.def.clone());
                }
            }
            (Some(_), false) => {}
        }
        if self.running {
            open.fx.update(dt);
        }
        let mut meshes: Vec<Mesh> = Vec::new();
        open.fx.meshes(&mut meshes);
        // The emitter stands at the origin; the camera looks a little above it, so what rises
        // has somewhere to rise into and what falls has somewhere to fall from.
        let camera = Vec2::new(0.0, -(size.1 as f32) * 0.28);
        viewport.render_meshes(egui, size, camera, linear(GROUNDS[self.ground].1), &meshes);
    }
}

/// The effect's sheet, uploaded, with one rectangle for each frame it names.
fn art_for(project: &Project, viewport: &mut Viewport, def: &EffectDef) -> Result<Art, String> {
    viewport.load_sheet(project, &def.sheet)?;
    let loaded = &viewport.sheets[&def.sheet];
    let texture = viewport
        .texture(&def.sheet)
        .ok_or_else(|| format!("{} is not on the screen", def.sheet))?;
    let mut frames = Vec::with_capacity(def.frames.len());
    for &frame in &def.frames {
        let picture = loaded.sheet.frames.get(frame as usize).ok_or_else(|| {
            format!(
                "frame {frame} is not in {} — it has {}",
                def.sheet,
                loaded.sheet.frames.len()
            )
        })?;
        frames.push(picture.rect);
    }
    Ok(Art {
        texture,
        page: Vec2::new(loaded.image.width as f32, loaded.image.height as f32),
        frames,
    })
}

/// A span: one number when it does not vary, two when it does, as the file writes it.
fn range(
    ui: &mut Ui,
    tracker: &mut Tracker,
    label: &str,
    value: &mut Range,
    span: std::ops::RangeInclusive<f32>,
    help: &str,
) {
    let mut varies = value.low != value.high;
    ui.horizontal(|ui| {
        ui.label(label).on_hover_text(help);
        let low = egui::DragValue::new(&mut value.low)
            .range(span.clone())
            .clamp_existing_to_range(false)
            .speed(0.5);
        tracker.track(ui.add(low));
        if varies {
            let high = egui::DragValue::new(&mut value.high)
                .range(span)
                .clamp_existing_to_range(false)
                .speed(0.5);
            tracker.track(ui.add(high));
        }
        if ui
            .toggle_value(&mut varies, "…")
            .on_hover_text("Pick from a span rather than always the same")
            .changed()
        {
            value.high = if varies { value.low + 1.0 } else { value.low };
            tracker.step(("range", label));
        }
    });
}

/// The frames of the sheet a particle may be, edited as the list of numbers the file holds.
fn frames(ui: &mut Ui, tracker: &mut Tracker, frames: &mut Vec<u32>) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Frames")
            .on_hover_text("One is picked at birth and kept");
        // The last one cannot go: an effect with no frames draws nothing, and the loader
        // refuses it. Shown greyed rather than hidden, so the row does not jump about.
        let removable = frames.len() > 1;
        let mut drop = None;
        for (n, frame) in frames.iter_mut().enumerate() {
            let field = egui::DragValue::new(frame).range(0..=4000);
            tracker.track(ui.add(field));
            if ui
                .add_enabled(removable, egui::Button::new("×").small())
                .clicked()
            {
                drop = Some(n);
            }
        }
        if let Some(n) = drop {
            frames.remove(n);
            tracker.step(("frames", n));
        }
        if ui.small_button("+").clicked() {
            frames.push(frames.last().copied().unwrap_or(0));
            tracker.step(("frames", "add"));
        }
    });
}

/// A linear RGBA, shown as the colour it is.
fn colour(ui: &mut Ui, tracker: &mut Tracker, label: &str, value: &mut [f32; 4]) {
    ui.horizontal(|ui| {
        ui.label(label);
        tracker.track(ui.color_edit_button_rgba_unmultiplied(value));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new effect must be one the game will load. Writing a file the loader then refuses would
    /// make the "Make it" button a way of producing broken files.
    #[test]
    fn a_new_effect_is_one_that_loads() {
        let def = EffectDef::plain("sheets/fx.sheet.ron");
        def.check("fx/new.fx.ron")
            .expect("what the button writes must be loadable");
        assert!(def.cap > 0 && def.life.high > 0.0);
    }

    /// The span toggle has to leave a span that means something: turning it on when both ends are
    /// equal would otherwise show two fields that still never vary.
    #[test]
    fn turning_a_span_on_gives_it_two_ends() {
        let mut value = Range::at(3.0);
        // What the toggle does, without an interface to click.
        value.high = value.low + 1.0;
        assert_ne!(value.low, value.high);
        assert_eq!(value.pick(0.0), 3.0);
        assert_eq!(value.pick(1.0), 4.0);
        // And off again.
        value.high = value.low;
        assert_eq!(value, Range::at(3.0));
    }

    /// The renderer clears to a linear colour and the surface is sRGB, so a background handed
    /// over as it looks would be painted far too bright — dusk half way to daylight.
    #[test]
    fn a_background_is_the_colour_it_looks() {
        assert_eq!(linear([0.0, 0.0, 0.0]), [0.0; 3]);
        assert_eq!(linear([1.0, 1.0, 1.0]), [1.0; 3]);
        let dusk = linear(GROUNDS[1].1);
        assert!(
            dusk.iter().all(|c| *c < 0.07),
            "dusk should be dark, and is {dusk:?}"
        );
        // Middle grey is the one everybody knows: about 0.5 to look at, about 0.21 in light.
        let mid = linear([0.5, 0.5, 0.5])[0];
        assert!((mid - 0.214).abs() < 0.005, "middle grey came out {mid}");
    }

    /// The backgrounds are there to judge a particle against, so they have to differ enough to be
    /// worth offering — a row of four near-identical greys would be a row of nothing.
    #[test]
    fn the_backgrounds_are_actually_different() {
        for (i, (name, a)) in GROUNDS.iter().enumerate() {
            for (other, b) in GROUNDS.iter().skip(i + 1) {
                let apart: f64 = a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum();
                assert!(apart > 0.2, "{name} and {other} are the same background");
            }
        }
    }
}

//! Conversations with the people of the world (`story.ron`), drawn as a flowchart: each box is
//! a line the person says, each arrow a choice the player can answer with. Lines are typed as
//! text; conditions and what choosing does are picked from lists. The year's endings, in the
//! order they are tried, are edited here too.

use std::collections::{BTreeMap, HashMap, HashSet};

use dark_assets::Project;
use dark_story::{Choice, Condition, Effect, EndingDef, Node, Repeat, StoryDef, Storylet};
use egui::{
    Align2, Color32, ComboBox, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

use crate::database::{Lists, to_ron};
use crate::strings::Strings;
use crate::widgets::{Tracker, tidy_id, valid_id};

/// What the middle shows.
#[derive(Clone, Debug, PartialEq)]
enum Open {
    Nothing,
    Storylet(String),
    Endings,
}

pub struct StoryEditor {
    def: StoryDef,
    /// Why the file cannot be edited (saving would lose it).
    pub broken: Option<String>,
    dirty: bool,
    undo: Vec<StoryDef>,
    redo: Vec<StoryDef>,
    editing: Option<egui::Id>,
    open: Open,
    /// The box chosen in the flowchart.
    node: Option<String>,
    new_id: String,
    new_with: String,
    /// Reading the conversation through: the box it has reached (empty once it has ended).
    reading: Option<String>,
}

/// Steps kept to undo.
const HISTORY: usize = 200;

impl StoryEditor {
    pub fn load(project: &Project) -> Self {
        let (def, broken) = match StoryDef::load_or_default(&project.path("story.ron")) {
            Ok(def) => (def, None),
            Err(e) => (StoryDef::default(), Some(e.to_string())),
        };
        Self {
            def,
            broken,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            editing: None,
            open: Open::Nothing,
            node: None,
            new_id: String::new(),
            new_with: String::new(),
            reading: None,
        }
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// The story as edited (saved or not).
    pub fn def(&self) -> &StoryDef {
        &self.def
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn record(&mut self, before: StoryDef) {
        self.undo.push(before);
        if self.undo.len() > HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
    }

    pub fn undo_step(&mut self) {
        if let Some(before) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.def, before));
            self.dirty = true;
            self.editing = None;
        }
    }

    pub fn redo_step(&mut self) {
        if let Some(after) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.def, after));
            self.dirty = true;
            self.editing = None;
        }
    }

    /// Writes `story.ron`, then checks it as the game will. Err if nothing could be written.
    pub fn save(&mut self, project: &Project, lists: &Lists) -> Result<Option<String>, String> {
        if let Some(broken) = &self.broken {
            return Err(format!(
                "fix {broken} first: saving now would lose what it holds"
            ));
        }
        let text = to_ron(&self.def)?;
        std::fs::write(
            project.path("story.ron"),
            format!("// Made in the editor (dark-editor).\n{text}\n"),
        )
        .map_err(|e| format!("story.ron: {e}"))?;
        self.dirty = false;
        Ok(self.check(lists).err())
    }

    /// The game's own checks, and that everything named exists.
    pub fn check(&self, lists: &Lists) -> Result<(), String> {
        self.def.validate().map_err(|e| e.to_string())?;
        let has = |list: &[(String, String)], id: &str| list.iter().any(|(i, _)| i == id);
        for s in &self.def.storylets {
            if !has(&lists.people, &s.with) {
                return Err(format!(
                    "{} is with {}, who is not in the world",
                    s.id, s.with
                ));
            }
            let conditions = s.when.iter().chain(
                s.nodes
                    .values()
                    .flat_map(|n| n.choices.iter().flat_map(|c| &c.when)),
            );
            for c in conditions {
                named_in_condition(c, lists).map_err(|e| format!("{}: {e}", s.id))?;
            }
            let effects = s
                .nodes
                .values()
                .flat_map(|n| n.then.iter().chain(n.choices.iter().flat_map(|c| &c.then)));
            for e in effects {
                let missing = match e {
                    Effect::Standing { faction, .. } | Effect::Defect(faction)
                        if !has(&lists.factions, faction) =>
                    {
                        Some(format!("no faction {faction}"))
                    }
                    Effect::Give { item, .. } if !has(&lists.items, item) => {
                        Some(format!("no item {item}"))
                    }
                    _ => None,
                };
                if let Some(missing) = missing {
                    return Err(format!("{}: {missing}", s.id));
                }
            }
        }
        for ending in &self.def.endings {
            for c in &ending.when {
                named_in_condition(c, lists).map_err(|e| format!("ending {}: {e}", ending.id))?;
            }
        }
        Ok(())
    }

    /// Every string key the story names (text or not yet).
    fn keys(&self) -> HashSet<String> {
        self.def.keys().into_iter().collect()
    }

    // --- The interface ---------------------------------------------------------------------

    pub fn ui(&mut self, ui: &mut Ui, lists: &Lists, strings: &mut Strings, language: &str) {
        if let Some(broken) = &self.broken {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.colored_label(Color32::from_rgb(255, 110, 100), broken);
                ui.label("The story cannot be edited until story.ron reads again.");
            });
            return;
        }
        let before = self.def.clone();
        let mut f = Tracker::default();
        let mut text = Text {
            strings,
            language,
            changed: false,
        };
        egui::Panel::left("story list")
            .resizable(true)
            .default_size(250.0)
            .min_size(200.0)
            .show(ui, |ui| self.list(ui, lists, &mut text));
        if let Open::Storylet(id) = self.open.clone() {
            egui::Panel::right("story node")
                .resizable(true)
                .default_size(380.0)
                .min_size(340.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        self.node_form(ui, &mut f, &mut text, lists, &id);
                    });
                });
        }
        egui::CentralPanel::default().show(ui, |ui| match self.open.clone() {
            Open::Nothing => {
                ui.label("Pick a conversation on the left, or make a new one.");
            }
            Open::Endings => {
                egui::ScrollArea::vertical()
                    .show(ui, |ui| self.endings(ui, &mut f, &mut text, lists));
            }
            Open::Storylet(id) => self.storylet(ui, &mut f, &mut text, lists, &id),
        });
        if let Some(changed) = f.changed {
            if self.editing != Some(changed) || f.step {
                self.record(before);
            }
            self.editing = (!f.step).then_some(changed);
            self.dirty = true;
        }
        if text.changed {
            self.editing = None;
        }
    }

    fn list(&mut self, ui: &mut Ui, lists: &Lists, text: &mut Text) {
        ui.heading("Story");
        ui.label("New conversation with");
        ui.horizontal(|ui| {
            if self.new_with.is_empty()
                && let Some((first, _)) = lists.people.first()
            {
                self.new_with = first.clone();
            }
            ComboBox::from_id_salt("new with")
                .selected_text(Lists::label(&lists.people, &self.new_with))
                .width(200.0)
                .show_ui(ui, |ui| {
                    for (id, label) in &lists.people {
                        ui.selectable_value(&mut self.new_with, id.clone(), label);
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_id)
                    .hint_text("id, e.g. borin_market")
                    .desired_width(150.0),
            );
            tidy_id(&mut self.new_id);
            let taken = self.def.storylets.iter().any(|s| s.id == self.new_id);
            let ok = valid_id(&self.new_id) && !taken && !self.new_with.is_empty();
            if ui.add_enabled(ok, egui::Button::new("Add")).clicked() {
                let before = self.def.clone();
                let id = std::mem::take(&mut self.new_id);
                let key = self.fresh(text, &format!("story.{id}"));
                text.strings.set(&key, text.language, "");
                self.def.storylets.push(Storylet {
                    id: id.clone(),
                    with: self.new_with.clone(),
                    priority: 0,
                    repeat: Repeat::Always,
                    when: Vec::new(),
                    start: "start".into(),
                    nodes: BTreeMap::from([(
                        "start".to_owned(),
                        Node {
                            line: key,
                            then: Vec::new(),
                            choices: Vec::new(),
                        },
                    )]),
                });
                self.record(before);
                self.open = Open::Storylet(id);
                self.node = Some("start".into());
                self.reading = None;
            }
            if taken {
                ui.weak("taken");
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            let chosen = self.open == Open::Endings;
            if ui
                .selectable_label(chosen, egui::RichText::new("The year's endings").strong())
                .clicked()
            {
                self.open = Open::Endings;
            }
            ui.separator();
            // By person, in the order the world lists them.
            let mut by_person: Vec<(&str, Vec<&Storylet>)> = Vec::new();
            for s in &self.def.storylets {
                match by_person.iter_mut().find(|(p, _)| *p == s.with) {
                    Some((_, list)) => list.push(s),
                    None => by_person.push((&s.with, vec![s])),
                }
            }
            let mut open = None;
            for (person, storylets) in by_person {
                ui.label(egui::RichText::new(Lists::label(&lists.people, person)).weak());
                for s in storylets {
                    let chosen = self.open == Open::Storylet(s.id.clone());
                    let first = s
                        .nodes
                        .get(&s.start)
                        .map(|n| text.strings.text(&n.line, text.language))
                        .unwrap_or_default();
                    let label = format!("{}  {}", s.id, short(&first, 28));
                    if ui.selectable_label(chosen, label).clicked() {
                        open = Some(s.id.clone());
                    }
                }
            }
            if let Some(id) = open {
                self.open = Open::Storylet(id);
                self.node = None;
                self.reading = None;
                self.editing = None;
            }
        });
    }

    /// A string key the story does not use yet.
    fn fresh(&self, text: &mut Text, prefix: &str) -> String {
        let used = self.keys();
        text.strings.fresh_key(prefix, |k| used.contains(k))
    }

    fn storylet(&mut self, ui: &mut Ui, f: &mut Tracker, text: &mut Text, lists: &Lists, id: &str) {
        let Some(index) = self.def.storylets.iter().position(|s| s.id == id) else {
            self.open = Open::Nothing;
            return;
        };
        let mut delete = false;
        ui.horizontal(|ui| {
            ui.heading(format!("Conversation {id}"));
            let asking = ui.id().with(("delete", id));
            let asked = ui.data(|d| d.get_temp::<bool>(asking).unwrap_or(false));
            if !asked && ui.button("Delete…").clicked() {
                ui.data_mut(|d| d.insert_temp(asking, true));
            }
            if asked {
                ui.label("Delete this conversation?");
                if ui.button("Delete it").clicked() {
                    delete = true;
                    ui.data_mut(|d| d.insert_temp(asking, false));
                }
                if ui.button("Keep it").clicked() {
                    ui.data_mut(|d| d.insert_temp(asking, false));
                }
            }
        });
        if delete {
            let before = self.def.clone();
            self.def.storylets.remove(index);
            self.record(before);
            self.open = Open::Nothing;
            return;
        }
        let s = &mut self.def.storylets[index];
        egui::CollapsingHeader::new("When it is offered")
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("With");
                    let before = s.with.clone();
                    ComboBox::from_id_salt(("with", id))
                        .selected_text(Lists::label(&lists.people, &s.with))
                        .show_ui(ui, |ui| {
                            for (p, label) in &lists.people {
                                ui.selectable_value(&mut s.with, p.clone(), label);
                            }
                        });
                    if s.with != before {
                        f.mark(("with", id));
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Told");
                    let before = s.repeat;
                    for (r, label) in [
                        (Repeat::Always, "every time"),
                        (Repeat::Daily, "once a day"),
                        (Repeat::Once, "once"),
                    ] {
                        ui.selectable_value(&mut s.repeat, r, label);
                    }
                    if s.repeat != before {
                        f.mark(("repeat", id));
                    }
                });
                f.number(
                    ui,
                    "Priority",
                    &mut s.priority,
                    -1000..=1000,
                    "When several are open, the highest is told",
                );
                conditions(ui, f, ("when", id), "Only while", &mut s.when, lists);
            });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Click a box to change what is said there.");
            let reading = self.reading.is_some();
            if ui
                .selectable_label(reading, "Read it through")
                .on_hover_text("Follow the conversation as a player would, ignoring conditions")
                .clicked()
            {
                self.reading = if reading { None } else { Some(s.start.clone()) };
            }
        });
        if let Some(at) = self.reading.clone() {
            self.read_through(ui, text, index, &at);
            ui.separator();
        }
        let s = &self.def.storylets[index];
        let picked = flowchart(ui, s, text, self.node.as_deref(), self.reading.as_deref());
        if let Some(node) = picked {
            self.node = Some(node);
            self.editing = None;
        }
    }

    /// `at` is the box reached, or empty once the conversation has ended.
    fn read_through(&mut self, ui: &mut Ui, text: &mut Text, index: usize, at: &str) {
        let s = &self.def.storylets[index];
        if at.is_empty() {
            ui.horizontal(|ui| {
                ui.weak("(the conversation ends)");
                if ui.button("From the top").clicked() {
                    self.reading = Some(s.start.clone());
                }
            });
            return;
        }
        let Some(node) = s.nodes.get(at) else {
            self.reading = None;
            return;
        };
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(egui::RichText::new(text.strings.text(&node.line, text.language)).size(15.0));
            if node.choices.is_empty() {
                ui.horizontal(|ui| {
                    ui.weak("(the conversation ends)");
                    if ui.button("From the top").clicked() {
                        self.reading = Some(s.start.clone());
                    }
                });
            }
            let mut next = None;
            for (i, c) in node.choices.iter().enumerate() {
                let says = text.strings.text(&c.says, text.language);
                let shown = if c.when.is_empty() {
                    format!("{}  {}", i + 1, says)
                } else {
                    format!("{}  {}  (only sometimes)", i + 1, says)
                };
                if ui.button(shown).clicked() {
                    next = Some(c.next.clone());
                }
            }
            if let Some(next) = next {
                // An answer leading nowhere ends it.
                self.reading = Some(next.filter(|n| s.nodes.contains_key(n)).unwrap_or_default());
            }
        });
    }

    fn node_form(
        &mut self,
        ui: &mut Ui,
        f: &mut Tracker,
        text: &mut Text,
        lists: &Lists,
        id: &str,
    ) {
        let Some(index) = self.def.storylets.iter().position(|s| s.id == id) else {
            return;
        };
        let Some(name) = self.node.clone() else {
            ui.label("Click a box in the flowchart to change it.");
            return;
        };
        let used = self.keys();
        let s = &mut self.def.storylets[index];
        let node_ids: Vec<String> = s.nodes.keys().cloned().collect();
        let previews: HashMap<String, String> = s
            .nodes
            .iter()
            .map(|(n, node)| {
                (
                    n.clone(),
                    short(&text.strings.text(&node.line, text.language), 30),
                )
            })
            .collect();
        let start = s.start.clone();
        let person = Lists::label(&lists.people, &s.with).to_owned();
        let Some(node) = s.nodes.get_mut(&name) else {
            self.node = None;
            return;
        };
        ui.horizontal(|ui| {
            ui.heading(if name == start {
                "The first line"
            } else {
                "A line"
            });
        });
        ui.label(format!("What {person} says ({}):", text.language));
        text.field(ui, &node.line, true);
        effects(
            ui,
            f,
            ("node then", id, &name),
            "Then",
            &mut node.then,
            lists,
        );
        ui.separator();
        ui.label("What the player can answer (none: the conversation ends here):");
        let mut remove = None;
        let mut new_line = None;
        let count = node.choices.len();
        let mut swap = None;
        for (i, c) in node.choices.iter_mut().enumerate() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(format!("{}", i + 1));
                    if ui.add_enabled(i > 0, egui::Button::new("▲")).clicked() {
                        swap = Some((i - 1, i));
                    }
                    if ui
                        .add_enabled(i + 1 < count, egui::Button::new("▼"))
                        .clicked()
                    {
                        swap = Some((i, i + 1));
                    }
                    if ui.button("✖").on_hover_text("Remove this answer").clicked() {
                        remove = Some(i);
                    }
                });
                text.field(ui, &c.says, false);
                ui.horizontal(|ui| {
                    ui.label("Then");
                    let shown = match &c.next {
                        None => "the conversation ends".to_owned(),
                        Some(n) => format!("→ {}", previews.get(n).cloned().unwrap_or_default()),
                    };
                    let before = c.next.clone();
                    ComboBox::from_id_salt(("next", id, &name, i))
                        .selected_text(shown)
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut c.next, None, "the conversation ends");
                            for n in &node_ids {
                                let label =
                                    format!("→ {}", previews.get(n).cloned().unwrap_or_default());
                                ui.selectable_value(&mut c.next, Some(n.clone()), label);
                            }
                            if ui.selectable_label(false, "→ a new line…").clicked() {
                                new_line = Some(i);
                            }
                        });
                    if c.next != before {
                        f.mark(("next", id, &name, i));
                    }
                });
                conditions(
                    ui,
                    f,
                    ("choice when", id, &name, i),
                    "Shown only while",
                    &mut c.when,
                    lists,
                );
                effects(
                    ui,
                    f,
                    ("choice then", id, &name, i),
                    "Choosing it",
                    &mut c.then,
                    lists,
                );
            });
        }
        if let Some((a, b)) = swap {
            node.choices.swap(a, b);
            f.step(("swap", id, &name, a));
        }
        if let Some(i) = remove {
            node.choices.remove(i);
            f.step(("remove choice", id, &name, i));
        }
        if ui.button("Add an answer").clicked() {
            let key = text
                .strings
                .fresh_key(&format!("story.{id}.me"), |k| used.contains(k));
            text.strings.set(&key, text.language, "");
            node.choices.push(Choice {
                says: key,
                when: Vec::new(),
                then: Vec::new(),
                next: None,
            });
            f.step(("add choice", id, &name, count));
        }
        // A new box, linked from the answer that asked for it.
        if let Some(i) = new_line {
            let fresh = (1..)
                .map(|n| format!("line{n}"))
                .find(|n| !node_ids.contains(n))
                .expect("some number is free");
            let key = text
                .strings
                .fresh_key(&format!("story.{id}"), |k| used.contains(k));
            text.strings.set(&key, text.language, "");
            node.choices[i].next = Some(fresh.clone());
            s.nodes.insert(
                fresh.clone(),
                Node {
                    line: key,
                    then: Vec::new(),
                    choices: Vec::new(),
                },
            );
            f.step(("new line", id, &fresh));
            self.node = Some(fresh);
            return;
        }
        ui.separator();
        ui.horizontal(|ui| {
            if name != start
                && ui
                    .button("Start here")
                    .on_hover_text("Make this the first line")
                    .clicked()
            {
                s.start = name.clone();
                f.step(("start", id, &name));
            }
            if name != start && ui.button("Delete this line").clicked() {
                s.nodes.remove(&name);
                for n in s.nodes.values_mut() {
                    for c in &mut n.choices {
                        if c.next.as_deref() == Some(&name) {
                            c.next = None;
                        }
                    }
                }
                f.step(("delete line", id, &name));
                self.node = None;
            }
        });
    }

    fn endings(&mut self, ui: &mut Ui, f: &mut Tracker, text: &mut Text, lists: &Lists) {
        ui.heading("The year's endings");
        ui.label(
            "When the year is over they are tried in this order; the first that holds is shown.",
        );
        let used = self.keys();
        let endings = &mut self.def.endings;
        let count = endings.len();
        let mut swap = None;
        let mut remove = None;
        for (i, e) in endings.iter_mut().enumerate() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(format!("{}. {}", i + 1, e.id));
                    if ui.add_enabled(i > 0, egui::Button::new("▲")).clicked() {
                        swap = Some((i - 1, i));
                    }
                    if ui
                        .add_enabled(i + 1 < count, egui::Button::new("▼"))
                        .clicked()
                    {
                        swap = Some((i, i + 1));
                    }
                    if ui.button("✖").clicked() {
                        remove = Some(i);
                    }
                });
                text.field(ui, &e.text, true);
                conditions(ui, f, ("ending", i), "When", &mut e.when, lists);
            });
        }
        if let Some((a, b)) = swap {
            endings.swap(a, b);
            f.step(("swap ending", a));
        }
        if let Some(i) = remove {
            endings.remove(i);
            f.step(("remove ending", i));
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_id)
                    .hint_text("id, e.g. exile")
                    .desired_width(150.0),
            );
            tidy_id(&mut self.new_id);
            let ok = valid_id(&self.new_id) && !endings.iter().any(|e| e.id == self.new_id);
            if ui
                .add_enabled(ok, egui::Button::new("Add an ending"))
                .clicked()
            {
                let id = std::mem::take(&mut self.new_id);
                let key = text
                    .strings
                    .fresh_key(&format!("ending.{id}"), |k| used.contains(k));
                text.strings.set(&key, text.language, "");
                endings.push(EndingDef {
                    id: id.clone(),
                    text: key,
                    when: Vec::new(),
                });
                f.step(("add ending", id));
            }
        });
    }
}

/// Draws the conversation as boxes and arrows; returns the box clicked, if any. A box shows
/// the line said there, then the answers, each with its arrow to the line that follows (or
/// "end").
fn flowchart(
    ui: &mut Ui,
    s: &Storylet,
    text: &mut Text,
    chosen: Option<&str>,
    reading: Option<&str>,
) -> Option<String> {
    const W: f32 = 230.0;
    /// The line's part of a box, and each answer's row below it.
    const LINE: f32 = 58.0;
    const ROW: f32 = 18.0;
    const GAP: f32 = 80.0;
    let places = layout(s);
    let most = s.nodes.values().map(|n| n.choices.len()).max().unwrap_or(0);
    let h = LINE + ROW * most as f32 + 8.0;
    let (cols, rows) = places
        .values()
        .fold((0, 0), |(c, r), &(x, y)| (c.max(x + 1), r.max(y + 1)));
    let mut picked = None;
    egui::ScrollArea::both().show(ui, |ui| {
        let size = vec2(
            cols as f32 * (W + GAP) + 60.0,
            rows as f32 * (h + 30.0) + 40.0,
        );
        let (area, _) = ui.allocate_exact_size(size, Sense::hover());
        let painter = ui.painter_at(area);
        let at = |name: &str| {
            let (c, r) = places.get(name).copied().unwrap_or((0, 0));
            let height = LINE + ROW * s.nodes.get(name).map_or(0, |n| n.choices.len()) as f32 + 8.0;
            Rect::from_min_size(
                area.min + vec2(20.0 + c as f32 * (W + GAP), 24.0 + r as f32 * (h + 30.0)),
                vec2(W, height),
            )
        };
        let small = FontId::proportional(12.0);
        // Arrows first, under the boxes.
        for (name, node) in &s.nodes {
            let from = at(name);
            for (i, c) in node.choices.iter().enumerate() {
                let y = from.top() + LINE + ROW * i as f32 + ROW / 2.0;
                let start = pos2(from.right(), y);
                match &c.next {
                    Some(next) if s.nodes.contains_key(next) => {
                        let to = at(next);
                        let end = pos2(to.left(), to.top() + 20.0);
                        arrow(&painter, start, end, Color32::from_rgb(150, 170, 210));
                    }
                    _ => {
                        painter.text(
                            start + vec2(6.0, 0.0),
                            Align2::LEFT_CENTER,
                            "end",
                            small.clone(),
                            Color32::GRAY,
                        );
                    }
                }
            }
        }
        for (name, node) in &s.nodes {
            let rect = at(name);
            let response = ui.interact(rect, ui.id().with(("box", name)), Sense::click());
            let fill = if reading == Some(name.as_str()) {
                Color32::from_rgb(52, 70, 52)
            } else if response.hovered() {
                Color32::from_rgb(48, 52, 64)
            } else {
                Color32::from_rgb(36, 38, 46)
            };
            painter.rect_filled(rect, 6.0, fill);
            let stroke = if chosen == Some(name.as_str()) {
                Stroke::new(2.0, Color32::WHITE)
            } else {
                Stroke::new(1.0, Color32::from_gray(90))
            };
            painter.rect_stroke(rect, 6.0, stroke, StrokeKind::Inside);
            if *name == s.start {
                painter.text(
                    rect.left_top() + vec2(8.0, -2.0),
                    Align2::LEFT_BOTTOM,
                    "▶ start",
                    small.clone(),
                    Color32::from_rgb(120, 230, 120),
                );
            }
            let line = text.strings.text(&node.line, text.language);
            let shown = if line.is_empty() {
                "(nothing written yet)".to_owned()
            } else {
                line
            };
            let galley = painter.layout(
                shown,
                FontId::proportional(13.0),
                Color32::from_gray(230),
                W - 16.0,
            );
            let top = Rect::from_min_size(rect.min, vec2(W, LINE)).shrink(4.0);
            painter.with_clip_rect(top).galley(
                rect.left_top() + vec2(8.0, 6.0),
                galley,
                Color32::WHITE,
            );
            if !node.then.is_empty() {
                painter.text(
                    rect.right_top() + vec2(-6.0, 4.0),
                    Align2::RIGHT_TOP,
                    "★",
                    small.clone(),
                    Color32::from_rgb(255, 210, 120),
                );
            }
            // The answers, one row each.
            for (i, c) in node.choices.iter().enumerate() {
                let y = rect.top() + LINE + ROW * i as f32;
                painter.line_segment(
                    [pos2(rect.left() + 6.0, y), pos2(rect.right() - 6.0, y)],
                    Stroke::new(1.0, Color32::from_gray(60)),
                );
                let says = text.strings.text(&c.says, text.language);
                let mark = if c.when.is_empty() { "" } else { " ?" };
                let row = format!("{}  {}{mark}", i + 1, short(&says, 30));
                painter.with_clip_rect(rect.shrink(2.0)).text(
                    pos2(rect.left() + 8.0, y + ROW / 2.0),
                    Align2::LEFT_CENTER,
                    row,
                    small.clone(),
                    Color32::from_rgb(190, 205, 235),
                );
            }
            if response.clicked() {
                picked = Some(name.clone());
            }
        }
    });
    picked
}

fn arrow(painter: &egui::Painter, from: Pos2, to: Pos2, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    // Out to the right, then across, then in from the left: readable even going back.
    let out = pos2(from.x + 18.0, from.y);
    let into = pos2(to.x - 14.0, to.y);
    painter.line_segment([from, out], stroke);
    painter.line_segment([out, into], stroke);
    painter.line_segment([into, to], stroke);
    painter.line_segment([to, to + vec2(-7.0, -4.0)], stroke);
    painter.line_segment([to, to + vec2(-7.0, 4.0)], stroke);
}

/// Where each box goes: a column per step from the start, then the order met. Boxes the start
/// never reaches go in a column of their own at the end.
fn layout(s: &Storylet) -> HashMap<String, (usize, usize)> {
    let mut places = HashMap::new();
    let mut rows: Vec<usize> = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    if s.nodes.contains_key(&s.start) {
        queue.push_back((s.start.clone(), 0));
    }
    while let Some((name, depth)) = queue.pop_front() {
        if places.contains_key(&name) {
            continue;
        }
        if rows.len() <= depth {
            rows.resize(depth + 1, 0);
        }
        places.insert(name.clone(), (depth, rows[depth]));
        rows[depth] += 1;
        for c in &s.nodes[&name].choices {
            if let Some(next) = &c.next
                && s.nodes.contains_key(next)
                && !places.contains_key(next)
            {
                queue.push_back((next.clone(), depth + 1));
            }
        }
    }
    let loose = rows.len();
    let mut row = 0;
    for name in s.nodes.keys() {
        if !places.contains_key(name) {
            places.insert(name.clone(), (loose, row));
            row += 1;
        }
    }
    places
}

/// `text` cut to `n` characters, with an ellipsis.
fn short(text: &str, n: usize) -> String {
    let first_line = text.lines().next().unwrap_or("");
    if first_line.chars().count() > n || text.lines().count() > 1 {
        format!("{}…", first_line.chars().take(n).collect::<String>())
    } else {
        first_line.to_owned()
    }
}

/// Typed text, in the language being written.
struct Text<'a> {
    strings: &'a mut Strings,
    language: &'a str,
    changed: bool,
}

impl Text<'_> {
    fn field(&mut self, ui: &mut Ui, key: &str, multiline: bool) {
        let mut value = self.strings.text(key, self.language);
        let id = egui::Id::new(("story text", key));
        let edit = if multiline {
            egui::TextEdit::multiline(&mut value).desired_rows(2)
        } else {
            egui::TextEdit::singleline(&mut value)
        };
        if ui.add(edit.id(id).desired_width(f32::INFINITY)).changed() {
            self.strings.set(key, self.language, &value);
            self.changed = true;
        }
    }
}

/// Everything a condition names must exist.
fn named_in_condition(c: &Condition, lists: &Lists) -> Result<(), String> {
    let has = |list: &[(String, String)], id: &str| list.iter().any(|(i, _)| i == id);
    match c {
        Condition::Standing { faction, .. }
        | Condition::StandingBelow { faction, .. }
        | Condition::Faction(faction)
            if !has(&lists.factions, faction) =>
        {
            Err(format!("no faction {faction}"))
        }
        Condition::Title(t) if !has(&lists.titles, t) => Err(format!("no title {t}")),
        Condition::Season(id) if !has(&lists.seasons, id) => Err(format!("no season {id}")),
        Condition::During(id) if !has(&lists.events, id) => Err(format!("no event {id}")),
        Condition::Alive(p) | Condition::Dead(p) if !has(&lists.people, p) => {
            Err(format!("no person {p}"))
        }
        Condition::Not(inner) => named_in_condition(inner, lists),
        _ => Ok(()),
    }
}

/// The kinds of condition, as a person picks them.
const CONDITIONS: [&str; 24] = [
    "Standing at least",
    "Standing below",
    "Holds a title",
    "From day",
    "Someone lives",
    "Someone is dead",
    "A flag is set",
    "A flag is not set",
    "They like the player at least",
    "The player is married",
    "The player is unmarried",
    "They are unmarried",
    "They would follow the player",
    "They are the player's spouse",
    "They follow the player",
    "They do not follow the player",
    "The player is in the hero party",
    "The player is not in the hero party",
    "The player serves a faction",
    "The Demon Lord fell this year",
    "The Demon Lord still stands",
    "It is a season",
    "During a calendar event",
    "(unknown)",
];

fn condition_kind(c: &Condition) -> usize {
    match c {
        Condition::Standing { .. } => 0,
        Condition::StandingBelow { .. } => 1,
        Condition::Title(_) => 2,
        Condition::FromDay(_) => 3,
        Condition::Alive(_) => 4,
        Condition::Dead(_) => 5,
        Condition::Flag(_) => 6,
        Condition::NotFlag(_) => 7,
        Condition::Affinity(_) => 8,
        Condition::Married => 9,
        Condition::Unmarried => 10,
        Condition::PersonUnmarried => 11,
        Condition::WillFollow => 12,
        Condition::SpouseHere => 13,
        Condition::Follows => 14,
        Condition::NotFollowing => 15,
        Condition::InHeroParty => 16,
        Condition::NotInHeroParty => 17,
        Condition::Faction(_) => 18,
        Condition::GoalDefeated => 19,
        Condition::GoalSurvived => 20,
        Condition::Season(_) => 21,
        Condition::During(_) => 22,
        Condition::Not(_) => 23,
    }
}

fn new_condition(kind: usize, lists: &Lists) -> Condition {
    let first =
        |list: &[(String, String)]| list.first().map(|(i, _)| i.clone()).unwrap_or_default();
    match kind {
        0 => Condition::Standing {
            faction: first(&lists.factions),
            at_least: 100,
        },
        1 => Condition::StandingBelow {
            faction: first(&lists.factions),
            below: 0,
        },
        2 => Condition::Title(first(&lists.titles)),
        3 => Condition::FromDay(1),
        4 => Condition::Alive(first(&lists.people)),
        5 => Condition::Dead(first(&lists.people)),
        6 => Condition::Flag("flag".into()),
        7 => Condition::NotFlag("flag".into()),
        8 => Condition::Affinity(100),
        9 => Condition::Married,
        10 => Condition::Unmarried,
        11 => Condition::PersonUnmarried,
        12 => Condition::WillFollow,
        13 => Condition::SpouseHere,
        14 => Condition::Follows,
        15 => Condition::NotFollowing,
        16 => Condition::InHeroParty,
        17 => Condition::NotInHeroParty,
        18 => Condition::Faction(first(&lists.factions)),
        19 => Condition::GoalDefeated,
        20 => Condition::GoalSurvived,
        21 => Condition::Season(first(&lists.seasons)),
        _ => Condition::During(first(&lists.events)),
    }
}

/// A list of conditions, all of which must hold; each can be turned round ("not").
fn conditions(
    ui: &mut Ui,
    f: &mut Tracker,
    salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    label: &str,
    list: &mut Vec<Condition>,
    lists: &Lists,
) {
    ui.label(format!("{label}:"));
    ui.indent((salt, "conditions"), |ui| {
        let mut remove = None;
        for (i, c) in list.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let mut negated = matches!(c, Condition::Not(_));
                if ui.checkbox(&mut negated, "not").changed() {
                    let inner = std::mem::replace(c, Condition::Married);
                    *c = match inner {
                        Condition::Not(inner) => *inner,
                        other => Condition::Not(Box::new(other)),
                    };
                    f.step((salt, "not", i));
                }
                let inner = match c {
                    Condition::Not(inner) => inner.as_mut(),
                    other => other,
                };
                let kind = condition_kind(inner);
                let mut chosen = kind;
                ComboBox::from_id_salt((salt, "kind", i))
                    .selected_text(CONDITIONS[kind])
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for (k, name) in CONDITIONS.iter().enumerate().take(CONDITIONS.len() - 1) {
                            ui.selectable_value(&mut chosen, k, *name);
                        }
                    });
                if chosen != kind {
                    *inner = new_condition(chosen, lists);
                    f.mark((salt, "kind", i));
                }
                condition_fields(ui, f, (salt, i), inner, lists);
                if ui.small_button("✖").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            list.remove(i);
            f.step((salt, "remove", i));
        }
        if ui.small_button("+ condition").clicked() {
            list.push(new_condition(8, lists));
            f.step((salt, "add", list.len()));
        }
    });
}

fn condition_fields(
    ui: &mut Ui,
    f: &mut Tracker,
    salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    c: &mut Condition,
    lists: &Lists,
) {
    match c {
        Condition::Standing { faction, at_least } => {
            pick(ui, f, (salt, "faction"), faction, &lists.factions);
            f.track(ui.add(egui::DragValue::new(at_least).range(-1000..=1000)));
        }
        Condition::StandingBelow { faction, below } => {
            pick(ui, f, (salt, "faction"), faction, &lists.factions);
            f.track(ui.add(egui::DragValue::new(below).range(-1000..=1000)));
        }
        Condition::Title(t) => pick(ui, f, (salt, "title"), t, &lists.titles),
        Condition::Season(id) => pick(ui, f, (salt, "season"), id, &lists.seasons),
        Condition::During(id) => pick(ui, f, (salt, "event"), id, &lists.events),
        Condition::FromDay(d) => {
            f.track(ui.add(egui::DragValue::new(d).range(1..=365)));
        }
        Condition::Alive(p) | Condition::Dead(p) => pick(ui, f, (salt, "person"), p, &lists.people),
        Condition::Flag(flag) | Condition::NotFlag(flag) => {
            f.track(ui.add(egui::TextEdit::singleline(flag).desired_width(120.0)));
        }
        Condition::Affinity(a) => {
            f.track(ui.add(egui::DragValue::new(a).range(-1000..=1000)));
        }
        Condition::Faction(faction) => pick(ui, f, (salt, "faction"), faction, &lists.factions),
        _ => {}
    }
}

/// The kinds of effect, as a person picks them.
const EFFECTS: [&str; 10] = [
    "Set a flag",
    "Clear a flag",
    "Standing changes",
    "Liking changes",
    "They follow the player",
    "The player joins the hero party",
    "The player goes over to a faction",
    "They marry the player",
    "Fade to black",
    "Give items",
];

fn effect_kind(e: &Effect) -> usize {
    match e {
        Effect::SetFlag(_) => 0,
        Effect::ClearFlag(_) => 1,
        Effect::Standing { .. } => 2,
        Effect::Affinity(_) => 3,
        Effect::Follow => 4,
        Effect::JoinHeroParty => 5,
        Effect::Defect(_) => 6,
        Effect::Marry => 7,
        Effect::FadeToBlack => 8,
        Effect::Give { .. } => 9,
    }
}

fn new_effect(kind: usize, lists: &Lists) -> Effect {
    let first =
        |list: &[(String, String)]| list.first().map(|(i, _)| i.clone()).unwrap_or_default();
    match kind {
        0 => Effect::SetFlag("flag".into()),
        1 => Effect::ClearFlag("flag".into()),
        2 => Effect::Standing {
            faction: first(&lists.factions),
            by: 10,
        },
        3 => Effect::Affinity(10),
        4 => Effect::Follow,
        5 => Effect::JoinHeroParty,
        6 => Effect::Defect(first(&lists.factions)),
        7 => Effect::Marry,
        8 => Effect::FadeToBlack,
        _ => Effect::Give {
            item: first(&lists.items),
            count: 1,
        },
    }
}

fn effects(
    ui: &mut Ui,
    f: &mut Tracker,
    salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    label: &str,
    list: &mut Vec<Effect>,
    lists: &Lists,
) {
    ui.label(format!("{label}:"));
    ui.indent((salt, "effects"), |ui| {
        let mut remove = None;
        for (i, e) in list.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let kind = effect_kind(e);
                let mut chosen = kind;
                ComboBox::from_id_salt((salt, "effect", i))
                    .selected_text(EFFECTS[kind])
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for (k, name) in EFFECTS.iter().enumerate() {
                            ui.selectable_value(&mut chosen, k, *name);
                        }
                    });
                if chosen != kind {
                    *e = new_effect(chosen, lists);
                    f.mark((salt, "effect", i));
                }
                match e {
                    Effect::SetFlag(flag) | Effect::ClearFlag(flag) => {
                        f.track(ui.add(egui::TextEdit::singleline(flag).desired_width(120.0)));
                    }
                    Effect::Standing { faction, by } => {
                        pick(ui, f, (salt, "faction", i), faction, &lists.factions);
                        f.track(ui.add(egui::DragValue::new(by).range(-1000..=1000)));
                    }
                    Effect::Affinity(by) => {
                        f.track(ui.add(egui::DragValue::new(by).range(-1000..=1000)));
                    }
                    Effect::Defect(faction) => {
                        pick(ui, f, (salt, "defect", i), faction, &lists.factions)
                    }
                    Effect::Give { item, count } => {
                        pick(ui, f, (salt, "item", i), item, &lists.items);
                        f.track(ui.add(egui::DragValue::new(count).range(1..=999).prefix("× ")));
                    }
                    _ => {}
                }
                if ui.small_button("✖").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            list.remove(i);
            f.step((salt, "remove effect", i));
        }
        if ui.small_button("+ effect").clicked() {
            list.push(new_effect(3, lists));
            f.step((salt, "add effect", list.len()));
        }
    });
}

/// A picker over `list` (id, label).
fn pick(
    ui: &mut Ui,
    f: &mut Tracker,
    salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    value: &mut String,
    list: &[(String, String)],
) {
    let before = value.clone();
    ComboBox::from_id_salt(salt)
        .selected_text(Lists::label(list, value).to_owned())
        .show_ui(ui, |ui| {
            for (id, label) in list {
                ui.selectable_value(value, id.clone(), label);
            }
        });
    if *value != before {
        f.mark(salt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storylet() -> Storylet {
        StoryDef::parse(
            r#"(storylets: [(id: "s", with: "p", start: "a", nodes: {
                "a": (line: "l.a", choices: [(says: "c.1", next: "b"), (says: "c.2", next: "c"),
                     (says: "c.3")]),
                "b": (line: "l.b", choices: [(says: "c.4", next: "a")]),
                "c": (line: "l.c"),
                "loose": (line: "l.loose"),
            })])"#,
        )
        .unwrap()
        .storylets
        .remove(0)
    }

    #[test]
    fn the_flowchart_goes_step_by_step_from_the_start() {
        let places = layout(&storylet());
        assert_eq!(places["a"], (0, 0));
        assert_eq!(places["b"], (1, 0));
        assert_eq!(places["c"], (1, 1));
        assert_eq!(
            places["loose"],
            (2, 0),
            "a line nothing leads to stands apart"
        );
    }

    #[test]
    fn the_story_round_trips_and_what_it_names_must_exist() {
        let def = StoryDef::parse(
            r#"(storylets: [(id: "s", with: "p", start: "a", when: [Not(Faction("f"))],
                  nodes: {"a": (line: "l", then: [Give(item: "i", count: 1)])})],
                endings: [(id: "e", text: "t", when: [GoalDefeated])])"#,
        )
        .unwrap();
        let text = to_ron(&def).unwrap();
        assert_eq!(StoryDef::parse(&text).unwrap(), def);
        let editor = StoryEditor {
            def,
            broken: None,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            editing: None,
            open: Open::Nothing,
            node: None,
            new_id: String::new(),
            new_with: String::new(),
            reading: None,
        };
        let one = |id: &str| vec![(id.to_owned(), id.to_owned())];
        let mut lists = Lists {
            people: one("p"),
            factions: one("f"),
            titles: Vec::new(),
            items: one("i"),
            ..Lists::default()
        };
        editor.check(&lists).unwrap();
        lists.items.clear();
        assert!(editor.check(&lists).unwrap_err().contains("no item i"));
        lists.items = one("i");
        lists.factions.clear();
        assert!(
            editor.check(&lists).unwrap_err().contains("no faction f"),
            "inside Not too"
        );
    }

    #[test]
    fn every_kind_of_condition_and_effect_can_be_made_and_named() {
        let lists = Lists::default();
        for k in 0..CONDITIONS.len() - 1 {
            assert_eq!(condition_kind(&new_condition(k, &lists)), k);
        }
        for k in 0..EFFECTS.len() {
            assert_eq!(effect_kind(&new_effect(k, &lists)), k);
        }
        assert_eq!(short("a long line of text", 6), "a long…");
    }
}

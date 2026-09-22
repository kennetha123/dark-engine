//! Text as the editor writes it. People type what an NPC says, in each language; the game
//! refers to text by key, so the editor makes the keys and keeps the text in
//! `locale/<code>.editor.ron` (loaded over the hand-written `locale/<code>.ron`, which is left
//! alone, comments and all).

use std::collections::BTreeMap;

use dark_assets::{Localization, Project};

pub struct Strings {
    /// Every language's text as the game will read it (hand-written, then the editor's).
    pub shown: Localization,
    /// What the editor wrote, by language.
    written: BTreeMap<String, BTreeMap<String, String>>,
    pub languages: Vec<String>,
    dirty: bool,
    /// Why the text cannot be saved: a table that did not read (saving would lose it).
    pub broken: Option<String>,
}

impl Strings {
    pub fn load(project: &Project) -> Self {
        let languages = project.settings.languages.clone();
        let mut written = BTreeMap::new();
        let mut broken = None;
        for code in &languages {
            let path = project.path(format!("locale/{code}.editor.ron"));
            let table = match std::fs::read_to_string(&path) {
                Ok(text) => ron::from_str::<BTreeMap<String, String>>(&text).unwrap_or_else(|e| {
                    broken = Some(format!("{}: {e}", path.display()));
                    BTreeMap::new()
                }),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
                Err(e) => {
                    broken = Some(format!("{}: {e}", path.display()));
                    BTreeMap::new()
                }
            };
            written.insert(code.clone(), table);
        }
        let shown = Localization::load(project).unwrap_or_else(|e| {
            broken.get_or_insert(e.to_string());
            Localization::default()
        });
        Self {
            shown,
            written,
            languages,
            dirty: false,
            broken,
        }
    }

    /// The text for `key` in `language` (falling back as the game does), or "" if there is none.
    pub fn text(&mut self, key: &str, language: &str) -> String {
        let current = self.shown.language().to_owned();
        self.shown.set_language(language);
        let text = if self.shown.has(key) || self.has_written(key, language) {
            self.written
                .get(language)
                .and_then(|t| t.get(key))
                .cloned()
                .unwrap_or_else(|| self.shown.text(key).to_owned())
        } else {
            String::new()
        };
        self.shown.set_language(&current);
        text
    }

    fn has_written(&self, key: &str, language: &str) -> bool {
        self.written
            .get(language)
            .is_some_and(|t| t.contains_key(key))
    }

    pub fn set(&mut self, key: &str, language: &str, text: &str) {
        let table = self.written.entry(language.to_owned()).or_default();
        if table.get(key).map(String::as_str) != Some(text) {
            table.insert(key.to_owned(), text.to_owned());
            self.dirty = true;
        }
    }

    /// A key, `<prefix>.<n>`, with no text in any language and not `in_use` (keys the map
    /// already names, text or not).
    pub fn fresh_key(&mut self, prefix: &str, in_use: impl Fn(&str) -> bool) -> String {
        let current = self.shown.language().to_owned();
        let languages = self.languages.clone();
        let key = (1..)
            .map(|n| format!("{prefix}.{n}"))
            .find(|k| {
                !in_use(k)
                    && !self.written.values().any(|t| t.contains_key(k))
                    && !languages.iter().any(|code| {
                        self.shown.set_language(code);
                        self.shown.has(k)
                    })
            })
            .expect("some number is free");
        self.shown.set_language(&current);
        key
    }

    /// Writes what changed.
    pub fn save(&mut self, project: &Project) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(broken) = &self.broken {
            return Err(std::io::Error::other(format!(
                "fix {broken} first: saving now would lose what it holds"
            )));
        }
        for (code, table) in &self.written {
            let text = ron::ser::to_string_pretty(table, ron::ser::PrettyConfig::default())
                .map_err(std::io::Error::other)?;
            let header =
                "// Text written in the editor (dark-editor); it wins over locale/<code>.ron.\n";
            std::fs::write(
                project.path(format!("locale/{code}.editor.ron")),
                format!("{header}{text}\n"),
            )?;
        }
        self.dirty = false;
        self.shown = Localization::load(project).unwrap_or_default();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_written_in_the_editor_is_saved_beside_the_hand_written_table_and_wins() {
        let root = std::env::temp_dir().join(format!("dark-editor-strings-{}", std::process::id()));
        std::fs::create_dir_all(root.join("locale")).unwrap();
        std::fs::write(
            root.join("project.ron"),
            r#"(name: "T", tile_size: 16, resolution: (320, 180), languages: ["en", "ja"])"#,
        )
        .unwrap();
        let hand = "// kept\n{\"npc.a.1\": \"Hello\"}";
        std::fs::write(root.join("locale/en.ron"), hand).unwrap();
        std::fs::write(root.join("locale/ja.ron"), "{\"npc.a.3\": \"三\"}").unwrap();
        let project = Project::open(&root).unwrap();

        let mut s = Strings::load(&project);
        assert_eq!(s.text("npc.a.1", "en"), "Hello");
        assert_eq!(
            s.text("npc.a.1", "ja"),
            "",
            "no Japanese yet: nothing to show"
        );
        let key = s.fresh_key("npc.a", |_| false);
        assert_eq!(key, "npc.a.2", "npc.a.1 is taken by the hand-written table");
        s.set(&key, "ja", "こんにちは");
        assert_eq!(
            s.fresh_key("npc.a", |_| false),
            "npc.a.4",
            "taken in Japanese only is taken"
        );
        assert_eq!(
            s.fresh_key("npc.a", |k| k == "npc.a.4"),
            "npc.a.5",
            "a key the map names, with no text yet, is taken"
        );
        s.set("npc.a.1", "en", "Hi there");
        s.save(&project).unwrap();

        let mut again = Strings::load(&project);
        assert_eq!(again.text("npc.a.2", "ja"), "こんにちは");
        assert_eq!(
            again.text("npc.a.1", "en"),
            "Hi there",
            "the editor's text wins"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("locale/en.ron")).unwrap(),
            hand,
            "the hand-written table is left alone"
        );
        let mut game = Localization::load(&project).unwrap();
        game.set_language("ja");
        assert_eq!(game.text("npc.a.2"), "こんにちは", "the game reads it too");

        // A table that does not read is never written over.
        std::fs::write(root.join("locale/en.editor.ron"), "{ broken").unwrap();
        let mut hurt = Strings::load(&project);
        assert!(hurt.broken.is_some());
        hurt.set("npc.a.9", "en", "lost?");
        assert!(hurt.save(&project).is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("locale/en.editor.ron")).unwrap(),
            "{ broken"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}

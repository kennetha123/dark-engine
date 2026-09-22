//! Text in the pixel-art view. cosmic-text shapes and wraps it (Latin by words, Japanese between
//! characters, per Unicode line breaking); each glyph is rasterised once into an atlas texture and
//! drawn as a sprite. Coverage is cut to hard pixels, so a pixel font at its design size stays as
//! crisp as the art around it.

use std::collections::HashMap;

use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, SwashCache, SwashContent,
};
use dark_sprite::Rect;
use glam::Vec2;

use crate::{RenderError, Renderer, Sprite, TextureId};

/// Glyph atlas side in texels: room for about 4000 glyphs at 16 px.
const ATLAS_SIZE: u32 = 1024;
/// Coverage at or above this is a pixel of the glyph; below, empty.
const COVERAGE: u8 = 128;

#[derive(Debug, thiserror::Error)]
pub enum FontError {
    #[error("the font data holds no usable face")]
    NoFace,
    #[error("font size {0} must be positive")]
    Size(f32),
}

/// One glyph of laid-out text: where it is in the atlas, and where it goes relative to the text's
/// top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphQuad {
    pub src: Rect,
    pub offset: Vec2,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextLayout {
    pub glyphs: Vec<GlyphQuad>,
    /// Width of the widest line and height of all lines, in pixels.
    pub size: Vec2,
}

/// A glyph in the atlas.
#[derive(Clone, Copy, Debug)]
struct AtlasGlyph {
    src: Rect,
    /// From the pen position to the bitmap's left edge, and up to its top edge.
    left: i32,
    top: i32,
}

pub struct TextSystem {
    fonts: FontSystem,
    swash: SwashCache,
    family: String,
    metrics: Metrics,
    atlas: Vec<u8>,
    /// Shelf packing: next free x, top of the current shelf, its height.
    cursor: (u32, u32, u32),
    /// `None` for glyphs with no pixels (spaces) or that did not fit.
    glyphs: HashMap<CacheKey, Option<AtlasGlyph>>,
    texture: Option<TextureId>,
    dirty: bool,
    warned_full: bool,
}

impl TextSystem {
    /// Text in the font in `font` (TTF or OTF bytes) at `size` pixels, lines `line_height` apart.
    /// Only this font is used: no system fonts, so text looks the same on every machine.
    pub fn new(font: Vec<u8>, size: f32, line_height: f32) -> Result<Self, FontError> {
        if !(size > 0.0 && size.is_finite()) {
            return Err(FontError::Size(size));
        }
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_font_data(font);
        let family = db
            .faces()
            .find_map(|face| face.families.first().map(|(name, _)| name.clone()))
            .ok_or(FontError::NoFace)?;
        Ok(Self {
            fonts: FontSystem::new_with_locale_and_db("en-US".into(), db),
            swash: SwashCache::new(),
            family,
            metrics: Metrics::new(size, line_height.max(size)),
            atlas: vec![0; (ATLAS_SIZE * ATLAS_SIZE * 4) as usize],
            cursor: (0, 0, 0),
            glyphs: HashMap::new(),
            texture: None,
            dirty: true,
            warned_full: false,
        })
    }

    pub fn line_height(&self) -> f32 {
        self.metrics.line_height
    }

    /// Lays out `text` wrapped to `max_width` pixels, rasterising any glyphs not seen before.
    pub fn layout(&mut self, text: &str, max_width: f32) -> TextLayout {
        let mut buffer = Buffer::new(&mut self.fonts, self.metrics);
        buffer.set_size(Some(max_width.max(1.0)), None);
        buffer.set_text(
            text,
            &Attrs::new().family(Family::Name(&self.family)),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.fonts, false);

        let mut layout = TextLayout::default();
        let mut placed = Vec::new();
        for run in buffer.layout_runs() {
            layout.size.x = layout.size.x.max(run.line_w.ceil());
            layout.size.y = layout.size.y.max(run.line_top + run.line_height);
            for glyph in run.glyphs {
                let physical = glyph.physical((0.0, run.line_y), 1.0);
                placed.push((physical.cache_key, physical.x, physical.y));
            }
        }
        for (key, x, y) in placed {
            if let Some(glyph) = self.glyph(key) {
                layout.glyphs.push(GlyphQuad {
                    src: glyph.src,
                    offset: Vec2::new((x + glyph.left) as f32, (y - glyph.top) as f32),
                });
            }
        }
        layout
    }

    /// The atlas texture, with every glyph laid out so far. Call after this frame's layouts.
    pub fn texture(&mut self, renderer: &mut Renderer) -> Result<TextureId, RenderError> {
        let id = match self.texture {
            Some(id) => {
                if self.dirty {
                    renderer.update_texture(id, &self.atlas)?;
                }
                id
            }
            None => {
                let id = renderer.create_texture("glyphs", ATLAS_SIZE, ATLAS_SIZE, &self.atlas)?;
                self.texture = Some(id);
                id
            }
        };
        self.dirty = false;
        Ok(id)
    }

    /// Sprites drawing `layout` with its top-left at `at`, tinted `color`.
    pub fn sprites(
        layout: &TextLayout,
        texture: TextureId,
        at: Vec2,
        color: [f32; 4],
        layer: i32,
        sort_y: f32,
    ) -> impl Iterator<Item = Sprite> + '_ {
        layout.glyphs.iter().map(move |g| {
            let mut s = Sprite::new(texture, g.src, at + g.offset, Vec2::ZERO);
            s.color = color;
            s.layer = layer;
            s.sort_y = sort_y;
            s
        })
    }

    /// Where `key`'s pixels are in the atlas and its offset from the pen position.
    fn glyph(&mut self, key: CacheKey) -> Option<AtlasGlyph> {
        if let Some(known) = self.glyphs.get(&key) {
            return *known;
        }
        let placed = self
            .swash
            .get_image_uncached(&mut self.fonts, key)
            .and_then(|image| self.pack(&image));
        self.glyphs.insert(key, placed);
        placed
    }

    fn pack(&mut self, image: &cosmic_text::SwashImage) -> Option<AtlasGlyph> {
        let (w, h) = (image.placement.width, image.placement.height);
        if w == 0 || h == 0 || w >= ATLAS_SIZE || h >= ATLAS_SIZE {
            return None;
        }
        // One texel of padding keeps neighbours from bleeding.
        let (mut x, mut y, mut shelf) = self.cursor;
        if x + w + 1 > ATLAS_SIZE {
            (x, y, shelf) = (0, y + shelf + 1, 0);
        }
        if y + h + 1 > ATLAS_SIZE {
            if !self.warned_full {
                tracing::warn!("glyph atlas is full; new characters will not show");
                self.warned_full = true;
            }
            return None;
        }
        for row in 0..h {
            for col in 0..w {
                let i = (row * w + col) as usize;
                let (alpha, rgb) = match image.content {
                    SwashContent::Mask => (image.data[i], [255; 3]),
                    SwashContent::Color => {
                        let p = &image.data[i * 4..i * 4 + 4];
                        (p[3], [p[0], p[1], p[2]])
                    }
                    SwashContent::SubpixelMask => (image.data[i * 4 + 1], [255; 3]),
                };
                if alpha >= COVERAGE {
                    let d = (((y + row) * ATLAS_SIZE + x + col) * 4) as usize;
                    self.atlas[d..d + 3].copy_from_slice(&rgb);
                    self.atlas[d + 3] = 255;
                }
            }
        }
        self.cursor = (x + w + 1, y, shelf.max(h));
        self.dirty = true;
        Some(AtlasGlyph {
            src: Rect::new(x, y, w, h),
            left: image.placement.left,
            top: image.placement.top,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game project's font, when `DARK_TEST_PROJECT` points at it (as for the project smoke
    /// test); these tests are skipped otherwise.
    fn text_system() -> Option<TextSystem> {
        let project = std::env::var_os("DARK_TEST_PROJECT")?;
        let path = std::path::Path::new(&project).join("fonts/DotGothic16-Regular.ttf");
        let font = std::fs::read(path).ok()?;
        Some(TextSystem::new(font, 16.0, 18.0).unwrap())
    }

    #[test]
    fn wraps_english_by_words_and_japanese_between_characters() {
        let Some(mut text) = text_system() else {
            return;
        };
        let one_line = text.layout("Hello", 200.0);
        assert_eq!(one_line.size.y, 18.0);
        assert!(one_line.size.x > 20.0 && one_line.size.x < 60.0);
        assert_eq!(one_line.glyphs.len(), 5);

        let english = text.layout("Mind the ledges east of here, traveller.", 120.0);
        assert!(english.size.y >= 36.0, "wrapped: {:?}", english.size);
        assert!(english.size.x <= 120.0);

        // No spaces to break at: Japanese still wraps, and every character has a glyph.
        let japanese = "おはよう、旅の人。東の崖には気をつけな。";
        let ja = text.layout(japanese, 120.0);
        assert!(ja.size.y >= 36.0, "wrapped: {:?}", ja.size);
        assert!(ja.size.x <= 120.0);
        assert_eq!(ja.glyphs.len(), japanese.chars().count());
    }

    #[test]
    fn glyph_pixels_are_hard_edged() {
        let Some(mut text) = text_system() else {
            return;
        };
        text.layout("Aa 話す", 200.0);
        assert!(text.atlas.chunks(4).all(|p| p[3] == 0 || p[3] == 255));
        assert!(text.atlas.chunks(4).any(|p| p[3] == 255));
        // Laying out the same text again adds nothing to the atlas.
        let cursor = text.cursor;
        text.layout("Aa 話す", 200.0);
        assert_eq!(text.cursor, cursor);
    }
}

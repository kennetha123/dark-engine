//! Turning a sheet image into frame rectangles.

use serde::{Deserialize, Serialize};

use crate::Rect;

/// Row-major grid cells. Cells that do not fit the image completely are skipped.
pub fn slice_grid(
    image: (u32, u32),
    cell: (u32, u32),
    offset: (u32, u32),
    spacing: (u32, u32),
) -> Vec<Rect> {
    assert!(cell.0 > 0 && cell.1 > 0, "grid cell must be non-empty");
    let mut rects = Vec::new();
    let mut y = offset.1;
    while y + cell.1 <= image.1 {
        let mut x = offset.0;
        while x + cell.0 <= image.0 {
            rects.push(Rect::new(x, y, cell.0, cell.1));
            x += cell.0 + spacing.0;
        }
        y += cell.1 + spacing.1;
    }
    rects
}

/// Layout of an RPG Maker-style character sheet: blocks of characters, each block
/// `frames` columns by 4 rows (down, left, right, up).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterLayout {
    /// Characters across and down the sheet.
    pub characters: (u32, u32),
    /// Animation frames per direction.
    pub frames: u32,
}

impl CharacterLayout {
    /// RPG Maker naming: a `$` prefix (after any `!`) means one character per sheet, otherwise
    /// 4×2; a `(N)` anywhere in the name gives N frames, otherwise 3.
    pub fn from_file_stem(stem: &str) -> Self {
        let single = stem.trim_start_matches('!').starts_with('$');
        let frames = stem
            .rfind('(')
            .and_then(|open| {
                let rest = &stem[open + 1..];
                rest[..rest.find(')')?].parse().ok()
            })
            .filter(|&n| n > 0)
            .unwrap_or(3);
        Self {
            characters: if single { (1, 1) } else { (4, 2) },
            frames,
        }
    }

    pub fn count(&self) -> u32 {
        self.characters.0 * self.characters.1
    }

    pub fn cell(&self, image: (u32, u32)) -> (u32, u32) {
        (
            image.0 / (self.characters.0 * self.frames),
            image.1 / (self.characters.1 * 4),
        )
    }
}

/// Frames of one character, indexed `[direction][frame]` in [`crate::Facing`] row order.
pub fn slice_character(image: (u32, u32), layout: CharacterLayout, index: u32) -> [Vec<Rect>; 4] {
    assert!(
        index < layout.count(),
        "character {index} is not on this sheet"
    );
    let (cw, ch) = layout.cell(image);
    let block_x = (index % layout.characters.0) * layout.frames * cw;
    let block_y = (index / layout.characters.0) * 4 * ch;
    std::array::from_fn(|row| {
        (0..layout.frames)
            .map(|f| Rect::new(block_x + f * cw, block_y + row as u32 * ch, cw, ch))
            .collect()
    })
}

/// Options for [`slice_auto`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoSlice {
    /// Pixels with alpha above this are part of a sprite. Soft shadows are usually above 0.
    pub alpha_threshold: u8,
    /// Islands closer than this many texels are one sprite (e.g. a flower's loose petals).
    pub merge_distance: u32,
    /// Islands smaller than this in both dimensions are dropped as specks.
    pub min_size: u32,
}

impl Default for AutoSlice {
    fn default() -> Self {
        Self {
            alpha_threshold: 0,
            merge_distance: 2,
            min_size: 4,
        }
    }
}

/// Sprites found on a sheet, plus which pixels belong to which.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Islands {
    /// Bounding boxes in reading order (top to bottom, then left to right).
    pub rects: Vec<Rect>,
    /// Per pixel: `0` for none, `i + 1` for `rects[i]`. A box can enclose pixels of another
    /// island (a prop standing inside a tree's shadow), so copy by label, not by box.
    pub labels: Vec<u32>,
}

/// Finds sprites on an irregularly packed sheet by their transparent gaps.
/// Returns bounding boxes in reading order (top to bottom, then left to right).
pub fn slice_auto(rgba: &[u8], width: u32, height: u32, options: AutoSlice) -> Vec<Rect> {
    find_islands(rgba, width, height, options).rects
}

/// Like [`slice_auto`], also labelling each pixel with its island.
pub fn find_islands(rgba: &[u8], width: u32, height: u32, options: AutoSlice) -> Islands {
    let (w, h) = (width as usize, height as usize);
    assert_eq!(
        rgba.len(),
        w * h * 4,
        "rgba buffer does not match {width}x{height}"
    );
    let solid = |i: usize| rgba[i * 4 + 3] > options.alpha_threshold;
    // Pixels at most `merge_distance` empty texels apart are connected. Merging by pixel distance,
    // not bounding box: a ring-shaped cliff's box encloses unrelated props.
    let reach = options.merge_distance as usize + 1;

    // Component ids start at 1 so 0 can mean "background".
    let mut component = vec![0u32; w * h];
    let mut stack = Vec::new();
    let mut boxes = Vec::new();
    for start in 0..w * h {
        if component[start] != 0 || !solid(start) {
            continue;
        }
        let id = boxes.len() as u32 + 1;
        component[start] = id;
        stack.push(start);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            for ny in y.saturating_sub(reach)..=(y + reach).min(h - 1) {
                for nx in x.saturating_sub(reach)..=(x + reach).min(w - 1) {
                    let n = ny * w + nx;
                    if component[n] == 0 && solid(n) {
                        component[n] = id;
                        stack.push(n);
                    }
                }
            }
        }
        boxes.push(Rect::new(
            x0 as u32,
            y0 as u32,
            (x1 - x0 + 1) as u32,
            (y1 - y0 + 1) as u32,
        ));
    }

    // Drop specks, order for reading, and renumber labels to match.
    let count = boxes.len();
    let mut kept: Vec<(usize, Rect)> = boxes
        .into_iter()
        .enumerate()
        .filter(|(_, b)| b.w >= options.min_size || b.h >= options.min_size)
        .collect();
    kept.sort_by_key(|(_, b)| (b.y, b.x));
    let mut renumber = vec![0u32; count + 1];
    for (new, (old, _)) in kept.iter().enumerate() {
        renumber[*old + 1] = new as u32 + 1;
    }
    let labels = component
        .into_iter()
        .map(|c| renumber[c as usize])
        .collect();
    Islands {
        rects: kept.into_iter().map(|(_, b)| b).collect(),
        labels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(w: u32, h: u32) -> Vec<u8> {
        vec![0; (w * h * 4) as usize]
    }

    fn fill(rgba: &mut [u8], width: u32, rect: Rect, alpha: u8) {
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                rgba[((y * width + x) * 4 + 3) as usize] = alpha;
            }
        }
    }

    #[test]
    fn grid_skips_partial_cells() {
        let rects = slice_grid((100, 64), (32, 32), (0, 0), (0, 0));
        assert_eq!(rects.len(), 6);
        assert_eq!(rects[4], Rect::new(32, 32, 32, 32));
        let spaced = slice_grid((70, 34), (32, 32), (1, 1), (2, 0));
        assert_eq!(
            spaced,
            vec![Rect::new(1, 1, 32, 32), Rect::new(35, 1, 32, 32)]
        );
    }

    #[test]
    fn rpg_maker_names() {
        assert_eq!(
            CharacterLayout::from_file_stem("$character_walk"),
            CharacterLayout {
                characters: (1, 1),
                frames: 3
            }
        );
        assert_eq!(
            CharacterLayout::from_file_stem("MonsterChara01+(4)"),
            CharacterLayout {
                characters: (4, 2),
                frames: 4
            }
        );
        assert_eq!(
            CharacterLayout::from_file_stem("!$Door(2)"),
            CharacterLayout {
                characters: (1, 1),
                frames: 2
            }
        );
    }

    #[test]
    fn character_frames_for_eight_character_sheet() {
        // MonsterChara01+(4) at 3x: 3072x1536, 192 px cells.
        let layout = CharacterLayout::from_file_stem("MonsterChara01+(4)");
        assert_eq!(layout.cell((3072, 1536)), (192, 192));
        let frames = slice_character((3072, 1536), layout, 5);
        // Character 5 is second row, second block.
        assert_eq!(frames[0][0], Rect::new(768, 768, 192, 192));
        assert_eq!(
            frames[3][3],
            Rect::new(768 + 3 * 192, 768 + 3 * 192, 192, 192)
        );
    }

    #[test]
    fn auto_slice_finds_islands_and_merges_close_ones() {
        let (w, h) = (40, 20);
        let mut px = canvas(w, h);
        fill(&mut px, w, Rect::new(1, 1, 8, 8), 255);
        // Soft shadow touching the first sprite joins it.
        fill(&mut px, w, Rect::new(9, 5, 3, 4), 60);
        // Two petals 1 px apart become one flower.
        fill(&mut px, w, Rect::new(20, 2, 3, 3), 255);
        fill(&mut px, w, Rect::new(24, 2, 3, 3), 255);
        // A lone speck is dropped.
        fill(&mut px, w, Rect::new(35, 15, 1, 1), 255);

        let rects = slice_auto(&px, w, h, AutoSlice::default());
        assert_eq!(rects, vec![Rect::new(1, 1, 11, 8), Rect::new(20, 2, 7, 3)]);
    }

    #[test]
    fn auto_slice_keeps_sprites_inside_a_ring_separate() {
        let (w, h) = (30, 30);
        let mut px = canvas(w, h);
        // A hollow ring whose bounding box encloses a separate sprite.
        fill(&mut px, w, Rect::new(0, 0, 30, 2), 255);
        fill(&mut px, w, Rect::new(0, 28, 30, 2), 255);
        fill(&mut px, w, Rect::new(0, 0, 2, 30), 255);
        fill(&mut px, w, Rect::new(28, 0, 2, 30), 255);
        fill(&mut px, w, Rect::new(12, 12, 6, 6), 255);
        let rects = slice_auto(&px, w, h, AutoSlice::default());
        assert_eq!(
            rects,
            vec![Rect::new(0, 0, 30, 30), Rect::new(12, 12, 6, 6)]
        );
    }

    #[test]
    fn labels_separate_a_sprite_enclosed_by_another_box() {
        let (w, h) = (30, 30);
        let mut px = canvas(w, h);
        fill(&mut px, w, Rect::new(0, 0, 30, 2), 255);
        fill(&mut px, w, Rect::new(0, 0, 2, 30), 255);
        fill(&mut px, w, Rect::new(12, 12, 6, 6), 255);
        // A speck inside the L's box is dropped and must not keep a label.
        fill(&mut px, w, Rect::new(25, 25, 1, 1), 255);
        let islands = find_islands(&px, w, h, AutoSlice::default());
        assert_eq!(islands.rects.len(), 2);
        assert_eq!(islands.labels[0], 1, "corner belongs to the L");
        assert_eq!(
            islands.labels[(14 * w + 14) as usize],
            2,
            "inner square is its own island"
        );
        assert_eq!(
            islands.labels[(25 * w + 25) as usize],
            0,
            "dropped speck is background"
        );
        assert_eq!(islands.labels[(20 * w + 20) as usize], 0);
    }

    #[test]
    fn auto_slice_threshold_can_exclude_shadows() {
        let (w, h) = (20, 10);
        let mut px = canvas(w, h);
        fill(&mut px, w, Rect::new(1, 1, 5, 5), 255);
        fill(&mut px, w, Rect::new(6, 1, 5, 5), 60);
        let options = AutoSlice {
            alpha_threshold: 128,
            ..AutoSlice::default()
        };
        assert_eq!(slice_auto(&px, w, h, options), vec![Rect::new(1, 1, 5, 5)]);
    }
}

/// Alpha at or above which a pixel is the object itself rather than its soft shadow.
const OPAQUE: u8 = 200;

/// Where an object stands inside `rect`: horizontally the middle of its lowest opaque band,
/// vertically just below its lowest opaque pixel. `None` if nothing in `rect` is opaque.
/// Relative to `rect`'s top-left, like a pivot.
pub fn footprint(rgba: &[u8], width: u32, rect: Rect) -> Option<(f32, f32)> {
    let opaque = |x: u32, y: u32| rgba[((y * width + x) * 4 + 3) as usize] >= OPAQUE;
    let rows: Vec<u32> = (rect.y..rect.bottom())
        .filter(|&y| (rect.x..rect.right()).any(|x| opaque(x, y)))
        .collect();
    let (&top, &bottom) = (rows.first()?, rows.last()?);
    // The base: the lowest tenth of the object (at least 2 rows), where it meets the ground.
    let band = ((bottom - top + 1) / 10).max(2);
    let (mut x0, mut x1) = (u32::MAX, 0);
    for y in bottom.saturating_sub(band - 1).max(top)..=bottom {
        for x in rect.x..rect.right() {
            if opaque(x, y) {
                x0 = x0.min(x);
                x1 = x1.max(x);
            }
        }
    }
    Some((
        (x0 + x1 + 1) as f32 / 2.0 - rect.x as f32,
        (bottom + 1 - rect.y) as f32,
    ))
}

#[cfg(test)]
mod footprint_tests {
    use super::*;

    #[test]
    fn footprint_ignores_the_soft_shadow() {
        let (w, h) = (40u32, 20u32);
        let mut px = vec![0u8; (w * h * 4) as usize];
        let mut fill = |r: Rect, a: u8| {
            for y in r.y..r.bottom() {
                for x in r.x..r.right() {
                    px[((y * w + x) * 4 + 3) as usize] = a;
                }
            }
        };
        // Shadow cast to the left, lower than the object; the object on the right.
        fill(Rect::new(0, 12, 30, 6), 90);
        fill(Rect::new(24, 2, 10, 14), 255);
        let frame = Rect::new(0, 0, 40, 20);
        assert_eq!(footprint(&px, w, frame), Some((29.0, 16.0)));
        assert_eq!(
            footprint(&px, w, Rect::new(0, 17, 10, 3)),
            None,
            "only shadow there"
        );
    }
}

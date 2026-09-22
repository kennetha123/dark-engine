//! Sprite sheets: slicing, frames, animation clips and facing.
//!
//! No GPU and no file IO, so this is a sim crate: the host can know which frame a character is
//! on (hitboxes are authored per frame in M4). Pixel data is passed in as raw RGBA8.

mod animation;
mod pack;
mod slice;

use glam::Vec2;
use serde::{Deserialize, Serialize};

pub use animation::{AnimationPlayer, Clip, ClipId};
pub use pack::shelf_pack;
pub use slice::{
    AutoSlice, CharacterLayout, Islands, find_islands, footprint, slice_auto, slice_character,
    slice_grid,
};

/// A rectangle in texels, origin top-left.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> u32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> u32 {
        self.y + self.h
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }
}

/// Where a frame is anchored in the world. Characters and props are Y-sorted by this point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Pivot {
    Center,
    /// Feet: bottom edge, horizontally centered. The default for anything standing on the ground.
    #[default]
    BottomCenter,
    /// Texels from the frame's top-left corner.
    Pixel(f32, f32),
    /// Bottom-centre of the frame's opaque pixels, ignoring soft shadows. For props on packed
    /// sheets, whose boxes include a shadow cast to one side. Needs pixels: see [`footprint`].
    Footprint,
}

impl Pivot {
    pub fn resolve(&self, rect: &Rect) -> Vec2 {
        match *self {
            Pivot::Center => Vec2::new(rect.w as f32 / 2.0, rect.h as f32 / 2.0),
            // Without pixels a footprint falls back to the box's bottom-centre.
            Pivot::BottomCenter | Pivot::Footprint => Vec2::new(rect.w as f32 / 2.0, rect.h as f32),
            Pivot::Pixel(x, y) => Vec2::new(x, y),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub rect: Rect,
    /// Anchor in texels relative to `rect`'s top-left.
    pub pivot: Vec2,
}

/// Frames and clips of one sheet image. Holds no pixels, so the host can use it headless.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpriteSheet {
    pub frames: Vec<Frame>,
    pub clips: Vec<Clip>,
    /// What clips mean for the game beyond pictures, by clip id: baked from skeletal animation
    /// (docs/PLAN.md §16). Empty for plain sheets; may be shorter than `clips`.
    #[serde(default)]
    pub timing: Vec<ClipTiming>,
}

/// A clip's events and boxes, one entry per frame of the clip (each frame is one tick in baked
/// clips).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClipTiming {
    /// Named moments, by frame of the clip. `strike` is when an attack lands.
    #[serde(default)]
    pub events: Vec<(String, u32)>,
    /// Where a blow lands this frame (ground plane, relative to the feet); empty if the clip
    /// has none, `None` on frames without one.
    #[serde(default)]
    pub hitboxes: Vec<Option<Circle>>,
    /// Where the body can be hit this frame; empty for the default footprint.
    #[serde(default)]
    pub hurtboxes: Vec<Option<Circle>>,
}

impl ClipTiming {
    /// The first frame an event of this name happens on.
    pub fn event(&self, name: &str) -> Option<u32> {
        self.events.iter().find(|(n, _)| n == name).map(|(_, f)| *f)
    }
}

/// A circle on the ground plane, relative to a character's feet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Circle {
    pub offset: Vec2,
    pub radius: f32,
}

impl SpriteSheet {
    /// Baked timing of clip `id`, if any.
    pub fn timing(&self, id: ClipId) -> Option<&ClipTiming> {
        self.timing.get(usize::from(id.0))
    }

    pub fn clip_id(&self, name: &str) -> Option<ClipId> {
        self.clips
            .iter()
            .position(|c| c.name == name)
            .map(|i| ClipId(i as u16))
    }

    /// Adds a clip, replacing any clip with the same name.
    pub fn add_clip(&mut self, clip: Clip) -> ClipId {
        match self.clip_id(&clip.name) {
            Some(id) => {
                self.clips[id.0 as usize] = clip;
                id
            }
            None => {
                self.clips.push(clip);
                ClipId(self.clips.len() as u16 - 1)
            }
        }
    }
}

/// Eight-way facing. The first four are the rows of an RPG Maker-style sheet, in its order;
/// sheets without diagonal art show a diagonal as its [`Facing::cardinal`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Facing {
    #[default]
    Down,
    Left,
    Right,
    Up,
    DownLeft,
    DownRight,
    UpLeft,
    UpRight,
}

impl Facing {
    /// The four directions every character sheet has, in row order.
    pub const CARDINAL: [Facing; 4] = [Facing::Down, Facing::Left, Facing::Right, Facing::Up];
    pub const ALL: [Facing; 8] = [
        Facing::Down,
        Facing::Left,
        Facing::Right,
        Facing::Up,
        Facing::DownLeft,
        Facing::DownRight,
        Facing::UpLeft,
        Facing::UpRight,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Facing::Down => "down",
            Facing::Left => "left",
            Facing::Right => "right",
            Facing::Up => "up",
            Facing::DownLeft => "down_left",
            Facing::DownRight => "down_right",
            Facing::UpLeft => "up_left",
            Facing::UpRight => "up_right",
        }
    }

    /// The nearest of the four [`Facing::CARDINAL`] directions. Diagonals read as sideways,
    /// which looks better than up or down on four-way sheets.
    pub fn cardinal(self) -> Facing {
        match self {
            Facing::DownLeft | Facing::UpLeft => Facing::Left,
            Facing::DownRight | Facing::UpRight => Facing::Right,
            other => other,
        }
    }

    /// Unit vector in screen space (+y is down).
    pub fn vector(self) -> Vec2 {
        let d = std::f32::consts::FRAC_1_SQRT_2;
        match self {
            Facing::Down => Vec2::Y,
            Facing::Left => Vec2::NEG_X,
            Facing::Right => Vec2::X,
            Facing::Up => Vec2::NEG_Y,
            Facing::DownLeft => Vec2::new(-d, d),
            Facing::DownRight => Vec2::new(d, d),
            Facing::UpLeft => Vec2::new(-d, -d),
            Facing::UpRight => Vec2::new(d, -d),
        }
    }

    /// The nearest of the eight directions to a vector in screen space (+y is down). `None`
    /// when it is zero.
    pub fn from_vector(v: Vec2) -> Option<Facing> {
        if v.length_squared() < 1e-6 || !v.is_finite() {
            return None;
        }
        // Octant 0 is east, counting clockwise on screen (towards +y).
        let octant = (v.y.atan2(v.x) / std::f32::consts::FRAC_PI_4).round() as i32;
        Some(match octant.rem_euclid(8) {
            0 => Facing::Right,
            1 => Facing::DownRight,
            2 => Facing::Down,
            3 => Facing::DownLeft,
            4 => Facing::Left,
            5 => Facing::UpLeft,
            6 => Facing::Up,
            _ => Facing::UpRight,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facing_from_vector() {
        assert_eq!(Facing::from_vector(Vec2::ZERO), None);
        assert_eq!(Facing::from_vector(Vec2::new(0.0, 1.0)), Some(Facing::Down));
        assert_eq!(Facing::from_vector(Vec2::new(0.0, -1.0)), Some(Facing::Up));
        assert_eq!(
            Facing::from_vector(Vec2::new(-1.0, 0.2)),
            Some(Facing::Left)
        );
        assert_eq!(
            Facing::from_vector(Vec2::new(1.0, 1.0)),
            Some(Facing::DownRight)
        );
        assert_eq!(
            Facing::from_vector(Vec2::new(-1.0, -1.0)),
            Some(Facing::UpLeft)
        );
        assert_eq!(Facing::from_vector(Vec2::NAN), None);
        for facing in Facing::ALL {
            assert_eq!(Facing::from_vector(facing.vector()), Some(facing));
        }
        assert_eq!(Facing::UpRight.cardinal(), Facing::Right);
        assert_eq!(Facing::Up.cardinal(), Facing::Up);
    }

    #[test]
    fn rect_union() {
        let a = Rect::new(0, 0, 4, 4);
        let b = Rect::new(6, 0, 4, 4);
        assert_eq!(a.union(&b), Rect::new(0, 0, 10, 4));
    }

    #[test]
    fn pivots() {
        let r = Rect::new(10, 10, 64, 32);
        assert_eq!(Pivot::BottomCenter.resolve(&r), Vec2::new(32.0, 32.0));
        assert_eq!(Pivot::Center.resolve(&r), Vec2::new(32.0, 16.0));
    }
}

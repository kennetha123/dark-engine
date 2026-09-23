//! The whole map at a glance (docs/PLAN.md §24): where the view is looking, and a click to look
//! somewhere else.
//!
//! The map view shows a screenful at a time and never zooms out past 1:1, so on anything larger
//! than a few screens a designer loses track of where they are. This fits the whole map into a
//! small box, whatever its size, and reads a point in that box back as a place in the world.
//!
//! The fitting is here, apart from the interface, so it can be tested without a window.

use egui::{Pos2, Rect, vec2};
use glam::Vec2;

/// A map fitted into a box: where its corners are, and how to read a point either way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// Where the map itself is drawn, inside the box it was given.
    pub shown: Rect,
    /// The map's size in world pixels.
    pub size: Vec2,
    /// Points per world pixel. Kept rather than worked out again, so it is never zero.
    scale: f32,
}

impl Fit {
    /// Fits a map of `size` world pixels inside `into`, keeping its shape and centring what is
    /// left over. A map with no size at all is given the whole box, so nothing divides by zero.
    pub fn new(into: Rect, size: Vec2) -> Self {
        let size = size.max(Vec2::ONE);
        let scale = (into.width() / size.x)
            .min(into.height() / size.y)
            .max(f32::MIN_POSITIVE);
        let shown = Rect::from_center_size(into.center(), vec2(size.x * scale, size.y * scale));
        Self { shown, size, scale }
    }

    /// Where a place in the world is drawn.
    pub fn to_screen(self, at: Vec2) -> Pos2 {
        self.shown.min + vec2(at.x * self.scale, at.y * self.scale)
    }

    /// What place a point in the box is. A point outside the map is the nearest place inside it,
    /// so a click near the edge looks at the edge rather than off the map.
    pub fn to_world(self, at: Pos2) -> Vec2 {
        let at = Vec2::new(at.x - self.shown.min.x, at.y - self.shown.min.y) / self.scale;
        at.clamp(Vec2::ZERO, self.size)
    }

    /// A rectangle of the world, as drawn. Anything smaller than a point is still drawn a point
    /// wide, so a door in a large map does not disappear.
    pub fn rect(self, (x, y, w, h): (f32, f32, f32, f32)) -> Rect {
        let min = self.to_screen(Vec2::new(x, y));
        let max = self.to_screen(Vec2::new(x + w, y + h));
        Rect::from_min_max(
            min,
            Pos2::new(max.x.max(min.x + 1.0), max.y.max(min.y + 1.0)),
        )
    }
}

/// Where the view may look: anywhere on the map itself. The middle of the view stays on the
/// map, so the very edge of a map can be worked on with the edge down the middle of the screen,
/// and the map can never be lost off the side of the view.
pub fn keep_inside(center: Vec2, size: Vec2) -> Vec2 {
    center.clamp(Vec2::ZERO, size.max(Vec2::ZERO))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_of(w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(10.0, 20.0), vec2(w, h))
    }

    #[test]
    fn a_map_keeps_its_shape_and_sits_in_the_middle() {
        // A wide map fills the width and leaves room above and below.
        let fit = Fit::new(box_of(100.0, 100.0), Vec2::new(1600.0, 800.0));
        assert_eq!(fit.shown.width(), 100.0);
        assert_eq!(fit.shown.height(), 50.0);
        assert_eq!(fit.shown.center(), box_of(100.0, 100.0).center());
        // A tall one fills the height instead.
        let fit = Fit::new(box_of(100.0, 100.0), Vec2::new(400.0, 1600.0));
        assert_eq!(fit.shown.height(), 100.0);
        assert_eq!(fit.shown.width(), 25.0);
    }

    #[test]
    fn a_point_read_back_is_the_place_it_was_drawn_at() {
        let fit = Fit::new(box_of(200.0, 120.0), Vec2::new(1600.0, 1200.0));
        for at in [
            Vec2::ZERO,
            Vec2::new(800.0, 600.0),
            Vec2::new(1600.0, 1200.0),
            Vec2::new(37.0, 913.0),
        ] {
            let back = fit.to_world(fit.to_screen(at));
            assert!(back.abs_diff_eq(at, 0.5), "{at} came back as {back}");
        }
    }

    #[test]
    fn a_click_outside_the_map_looks_at_its_edge() {
        let fit = Fit::new(box_of(200.0, 200.0), Vec2::new(1600.0, 1200.0));
        let far = Pos2::new(fit.shown.max.x + 500.0, fit.shown.min.y - 500.0);
        assert_eq!(fit.to_world(far), Vec2::new(1600.0, 0.0));
    }

    /// The whole point of it: a world too big for the view still fits in the box.
    #[test]
    fn a_hundred_kilometres_still_fits() {
        // 100 km at 16 px to the metre.
        let world = Vec2::splat(100_000.0 * 16.0);
        let fit = Fit::new(box_of(200.0, 200.0), world);
        assert_eq!(fit.shown.size(), vec2(200.0, 200.0));
        let middle = fit.to_world(fit.shown.center());
        assert!(middle.abs_diff_eq(world / 2.0, world.x * 0.01), "{middle}");
        // A door is still drawn, however small it is against the world.
        let door = fit.rect((800_000.0, 800_000.0, 44.0, 16.0));
        assert!(door.width() >= 1.0 && door.height() >= 1.0, "{door:?}");
    }

    #[test]
    fn the_view_stays_on_the_map() {
        let size = Vec2::new(1600.0, 1200.0);
        assert_eq!(keep_inside(Vec2::new(-9000.0, -9000.0), size), Vec2::ZERO);
        assert_eq!(keep_inside(Vec2::new(9000.0, 9000.0), size), size);
        let inside = Vec2::new(500.0, 500.0);
        assert_eq!(keep_inside(inside, size), inside);
    }

    /// A map smaller than one screenful is still only looked at where it is.
    #[test]
    fn a_tiny_map_is_not_wandered_away_from() {
        let size = Vec2::new(128.0, 128.0);
        assert_eq!(
            keep_inside(Vec2::new(-5000.0, 5000.0), size),
            Vec2::new(0.0, 128.0)
        );
    }

    /// What the whole thing is for: a click in the box leaves the map view looking at the place
    /// that was clicked. This is the click, the clamp, the rounding and the view's own mapping,
    /// end to end — each is harmless alone and they meet here.
    #[test]
    fn a_click_is_where_the_view_then_looks() {
        use crate::viewport::{Mapping, View};

        let size = Vec2::new(4096.0 * 16.0, 4096.0 * 16.0);
        let fit = Fit::new(box_of(200.0, 200.0), size);
        let panel = Rect::from_min_size(Pos2::new(240.0, 40.0), vec2(1000.0, 800.0));
        for at in [
            fit.shown.center(),
            fit.shown.min + vec2(20.0, 30.0),
            fit.shown.max - vec2(5.0, 5.0),
        ] {
            let asked = keep_inside(fit.to_world(at), size);
            let view = View {
                center: asked.round(),
                zoom: 2,
            };
            let (mapping, seen) = Mapping::new(panel, 1.0, view);
            let middle = mapping.to_world(panel.center());
            // Half a world pixel from the rounding, and the mapping's own rounded origin.
            assert!(
                middle.abs_diff_eq(asked, 1.5),
                "clicked {asked}, looking at {middle}"
            );
            assert!(seen.0 > 0 && seen.1 > 0);
        }
    }
}

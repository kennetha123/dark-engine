//! Rectangle packing for building atlases.

/// Shelf packing: tallest first, rows no wider than `max_width`, `padding` texels between items.
/// Returns each item's top-left in input order and the atlas size.
pub fn shelf_pack(
    sizes: &[(u32, u32)],
    max_width: u32,
    padding: u32,
) -> (Vec<(u32, u32)>, (u32, u32)) {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(sizes[i].1));
    let mut positions = vec![(0, 0); sizes.len()];
    let (mut x, mut y, mut row_height, mut width) = (padding, padding, 0, 0);
    for i in order {
        let (w, h) = sizes[i];
        if x > padding && x + w + padding > max_width {
            x = padding;
            y += row_height + padding;
            row_height = 0;
        }
        positions[i] = (x, y);
        x += w + padding;
        row_height = row_height.max(h);
        width = width.max(x);
    }
    let height = if sizes.is_empty() {
        0
    } else {
        y + row_height + padding
    };
    (positions, (width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: ((u32, u32), (u32, u32)), b: ((u32, u32), (u32, u32))) -> bool {
        let ((ax, ay), (aw, ah)) = a;
        let ((bx, by), (bw, bh)) = b;
        ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah
    }

    #[test]
    fn packs_without_overlap_inside_bounds() {
        let sizes = [(40, 30), (10, 50), (60, 10), (25, 25), (70, 20), (5, 5)];
        let (positions, (w, h)) = shelf_pack(&sizes, 100, 1);
        for (i, (&p, &s)) in positions.iter().zip(&sizes).enumerate() {
            assert!(
                p.0 + s.0 < w + 1 && p.1 + s.1 < h + 1,
                "item {i} outside atlas"
            );
            for (&q, &t) in positions.iter().zip(&sizes).skip(i + 1) {
                assert!(!overlaps((p, s), (q, t)));
            }
        }
        assert!(w <= 100);
    }

    #[test]
    fn oversized_item_gets_its_own_row() {
        let (positions, (w, _)) = shelf_pack(&[(150, 10), (20, 10)], 100, 0);
        assert_eq!(positions[0], (0, 0));
        assert_eq!(positions[1], (0, 10));
        assert_eq!(w, 150);
    }
}

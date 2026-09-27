//! `dark-cli preview-fx` — draws an effect at several moments into one picture
//! (`journals/engine/05`, phase 1).
//!
//! The same idea as `preview-model`: fill the triangles the game will draw, without a window or a
//! GPU, so that an effect can be checked by looking. An effect is a thing that happens over time,
//! so one moment tells you nothing — this runs one emitter and takes a panel out of it at evenly
//! spaced moments, left to right.

use dark_assets::{Image, Project};
use dark_fx::{Art, EffectDef, Effects};
use dark_render::{Mesh, TextureId};
use glam::Vec2;

/// One panel. Wide enough for weather, tall enough for smoke to rise out of.
const PANEL: (u32, u32) = (200, 220);

/// How many moments are drawn.
const PANELS: u32 = 5;

/// Where the emitter stands in a panel: centred, and low enough to leave room above it.
const FEET: (f32, f32) = (0.5, 0.78);

/// Sixty a second, as the game runs.
const STEP: f32 = 1.0 / 60.0;

pub fn preview_fx(
    project: &str,
    effect_path: &str,
    seconds: &str,
    out: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let project = Project::open(project)?;
    let def = EffectDef::load(&project, effect_path)?;
    let seconds: f32 = seconds.parse()?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(format!("{seconds} seconds is no time at all").into());
    }

    let loaded = project.load_sheet(&def.sheet)?;
    let mut frames = Vec::with_capacity(def.frames.len());
    for &frame in &def.frames {
        let picture = loaded.sheet.frames.get(frame as usize).ok_or_else(|| {
            format!(
                "{effect_path}: frame {frame} is not in {} (it has {})",
                def.sheet,
                loaded.sheet.frames.len()
            )
        })?;
        frames.push(picture.rect);
    }
    let art = Art {
        texture: TextureId::FIRST,
        page: Vec2::new(loaded.image.width as f32, loaded.image.height as f32),
        frames,
    };

    let mut fx = Effects::new();
    let kind = fx.add(def.clone(), art)?;
    // A standing emitter, because that is what shows an effect developing. Its opening burst is
    // born at once, so the first panel is not empty.
    fx.standing(kind, Vec2::ZERO);

    let (pw, ph) = PANEL;
    let width = pw * PANELS + (PANELS - 1);
    let mut canvas = image::RgbaImage::from_pixel(width, ph, image::Rgba([56, 60, 66, 255]));
    let origin = Vec2::new(pw as f32 * FEET.0, ph as f32 * FEET.1);

    let mut counts = Vec::new();
    let mut elapsed = 0.0f32;
    for panel in 0..PANELS {
        // The moment this panel shows. The first is a fifth of the way in rather than at zero:
        // a standing effect has nothing at all at zero, and a blank panel says nothing.
        let moment = seconds * (panel + 1) as f32 / PANELS as f32;
        while elapsed + STEP * 0.5 < moment {
            fx.update(STEP);
            elapsed += STEP;
        }
        let left = panel * (pw + 1);
        ground(&mut canvas, left, pw, ph, origin.y);

        let mut meshes = Vec::new();
        fx.meshes(&mut meshes);
        for mesh in &meshes {
            draw(
                &mut canvas,
                mesh,
                &loaded.image,
                (left, 0),
                (pw, ph),
                origin,
            );
        }
        counts.push((moment, fx.count()));
    }
    // The separators, drawn last so nothing bleeds across a panel's edge.
    for panel in 1..PANELS {
        let x = panel * (pw + 1) - 1;
        for y in 0..ph {
            canvas.put_pixel(x, y, image::Rgba([20, 22, 26, 255]));
        }
    }

    canvas.save(out)?;
    let moments: Vec<String> = counts
        .iter()
        .map(|(at, alive)| format!("{at:.2}s: {alive}"))
        .collect();
    println!(
        "{out}: {effect_path} over {seconds}s ({width}x{ph}), alive at {}",
        moments.join(", ")
    );
    println!(
        "  the file asks for {:.0} at once, capped at {}",
        def.wanted(),
        def.cap
    );
    Ok(())
}

/// The emitter's own row of ground, so lift can be read off the picture. Only the emitter's: a
/// particle born further down the map draws below this line at no height at all, which is right.
fn ground(canvas: &mut image::RgbaImage, left: u32, pw: u32, ph: u32, y: f32) {
    let y = y.round() as i64;
    if y < 0 || y >= ph as i64 {
        return;
    }
    for x in left..(left + pw).min(canvas.width()) {
        canvas.put_pixel(x, y as u32, image::Rgba([78, 84, 76, 255]));
    }
}

/// Fills one mesh's triangles, alpha blended, exactly as the sprite pass does: no depth, the
/// later triangle over the earlier one.
fn draw(
    canvas: &mut image::RgbaImage,
    mesh: &Mesh,
    page: &Image,
    (left, top): (u32, u32),
    (pw, ph): (u32, u32),
    origin: Vec2,
) {
    for tri in mesh.vertices.chunks_exact(3) {
        let screen: Vec<Vec2> = tri.iter().map(|v| v.position + origin).collect();
        let area = (screen[1] - screen[0]).perp_dot(screen[2] - screen[0]);
        if area.abs() < 1e-6 || !screen.iter().all(|p| p.is_finite()) {
            continue;
        }
        let (min, max) = (
            screen[0].min(screen[1]).min(screen[2]),
            screen[0].max(screen[1]).max(screen[2]),
        );
        let x0 = min.x.floor().max(0.0) as u32;
        let y0 = min.y.floor().max(0.0) as u32;
        let x1 = (max.x.ceil() as i64).clamp(0, pw as i64) as u32;
        let y1 = (max.y.ceil() as i64).clamp(0, ph as i64) as u32;
        for y in y0..y1 {
            for x in x0..x1 {
                let q = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let b1 = (screen[2] - screen[1]).perp_dot(q - screen[1]) / area;
                let b2 = (screen[0] - screen[2]).perp_dot(q - screen[2]) / area;
                let b3 = 1.0 - b1 - b2;
                // Both the area and the edges flip sign with the winding, so this is inside
                // either way round — which matters, because a spinning quad turns inside out.
                if b1.min(b2).min(b3) < 0.0 {
                    continue;
                }
                let uv = tri[0].uv * b1 + tri[1].uv * b2 + tri[2].uv * b3;
                let tint = [0, 1, 2, 3]
                    .map(|c| tri[0].color[c] * b1 + tri[1].color[c] * b2 + tri[2].color[c] * b3);
                let [r, g, b, a] = sample(page, uv);
                let alpha = f32::from(a) / 255.0 * tint[3];
                if alpha <= 0.002 {
                    continue;
                }
                let pixel = canvas.get_pixel_mut(left + x, top + y);
                for (c, v) in [r, g, b].into_iter().enumerate() {
                    let over = f32::from(v) * tint[c];
                    let under = f32::from(pixel[c]);
                    pixel[c] = (under + (over - under) * alpha).clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

/// Nearest, as the renderer samples: a pixel-art page must not be smeared.
fn sample(page: &Image, uv: Vec2) -> [u8; 4] {
    let x = (uv.x * page.width as f32)
        .floor()
        .clamp(0.0, (page.width - 1) as f32) as u32;
    let y = (uv.y * page.height as f32)
        .floor()
        .clamp(0.0, (page.height - 1) as f32) as u32;
    let at = ((y * page.width + x) * 4) as usize;
    page.rgba
        .get(at..at + 4)
        .map_or([255, 0, 255, 255], |p| [p[0], p[1], p[2], p[3]])
}

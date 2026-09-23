//! Project tooling.
//!
//! `dark-cli preview-sheet <project-dir> <sheet.ron> <out.png>` draws every sliced frame's box
//! and pivot over the sheet image, to check slicing without opening the game.
//!
//! `dark-cli preview-land <seed> <tiles> <tiles-per-pixel> <out.png>` draws the country a world
//! seed makes, from far above: water, plain, and the steps up out of it.
//!
//! `dark-cli simulate <project-dir> [--seed n] [--runs n] [--lang code]` fast-forwards the world
//! simulation (`world.ron`) through a year: the chronicle of one seed, or statistics over many.
//!
//! `dark-cli package <project-dir> <out-dir> [--exe <dark-player>]` collects the game and only
//! the files it loads into a folder to hand over: the exe beside a `game` folder.
//!
//! `dark-cli bake-spine <project-dir> <sheet.spine.ron>` measures a Spine skeleton for the host
//! (clip lengths, events, hitboxes) and writes the sheet's `baked` file. Run it after every
//! export from Spine. `dark-cli preview-spine <project-dir> <sheet.spine.ron> <clip> <tick>
//! <out.png>` draws the skeleton at that clip and tick, as the game would, four times enlarged.

mod land;
mod package;
mod simulate;

use std::path::PathBuf;
use std::process::ExitCode;

use dark_assets::Project;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd, seed, tiles, every, out] if cmd == "preview-land" => {
            let numbers = seed
                .parse()
                .ok()
                .zip(tiles.parse().ok())
                .zip(every.parse().ok());
            match numbers {
                Some(((seed, tiles), every)) => match land::preview(seed, tiles, every, out) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(err) => {
                        eprintln!("error: {err}");
                        ExitCode::FAILURE
                    }
                },
                None => {
                    eprintln!("error: preview-land takes a seed, a size in tiles and a step");
                    ExitCode::FAILURE
                }
            }
        }
        [cmd, project, sheet, out] if cmd == "preview-sheet" => {
            match preview_sheet(project, sheet, out) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("error: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        [cmd, input, out, x, y, w, h, scale] if cmd == "zoom" => {
            match zoom(input, out, [x, y, w, h, scale]) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("error: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        [cmd, project, sheet, clip, tick, out] if cmd == "preview-spine" => {
            match preview_spine(project, sheet, clip, tick, out) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("error: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        [cmd, project, sheet] if cmd == "bake-spine" => match bake_spine(project, sheet) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::FAILURE
            }
        },
        [cmd, path] if cmd == "image-info" => match dark_assets::load_image(path.as_ref()) {
            Ok(image) => {
                print_alpha_histogram(&image);
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::FAILURE
            }
        },
        [cmd, rest @ ..] if cmd == "package" => match package::parse(rest) {
            Ok(options) => match package::run(&options) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("error: {err}");
                    ExitCode::FAILURE
                }
            },
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::FAILURE
            }
        },
        [cmd, rest @ ..] if cmd == "simulate" => match simulate::parse(rest) {
            Ok((project, options)) => match simulate::run(&project, &options) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("error: {err}");
                    ExitCode::FAILURE
                }
            },
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("usage: dark-cli preview-sheet <project-dir> <sheet.ron> <out.png>");
            eprintln!("       dark-cli preview-land <seed> <tiles> <tiles-per-pixel> <out.png>");
            eprintln!("       dark-cli bake-spine <project-dir> <sheet.spine.ron>");
            eprintln!(
                "       dark-cli preview-spine <project-dir> <sheet.spine.ron> <clip> <tick> <out.png>"
            );
            eprintln!("       dark-cli image-info <image.png>");
            eprintln!("       dark-cli zoom <in.png> <out.png> <x> <y> <w> <h> <scale>");
            eprintln!(
                "       dark-cli simulate <project-dir> [--seed <n>] [--runs <n>] [--lang <code>]"
            );
            eprintln!("       dark-cli package <project-dir> <out-dir> [--exe <dark-player>]");
            ExitCode::FAILURE
        }
    }
}

/// Crops a region and enlarges it by a whole number with nearest-neighbour, to inspect pixels.
fn zoom(input: &str, out: &str, args: [&String; 5]) -> Result<(), Box<dyn std::error::Error>> {
    let [x, y, w, h, scale] = args.map(|a| a.parse::<u32>());
    let (x, y, w, h, scale) = (x?, y?, w?, h?, scale?.max(1));
    let image = image::open(input)?.into_rgba8();
    if w == 0
        || h == 0
        || x.saturating_add(w) > image.width()
        || y.saturating_add(h) > image.height()
    {
        return Err(format!(
            "crop {x},{y} {w}x{h} is empty or outside the {}x{} image",
            image.width(),
            image.height()
        )
        .into());
    }
    let crop = image::imageops::crop_imm(&image, x, y, w, h).to_image();
    let big = image::imageops::resize(
        &crop,
        w * scale,
        h * scale,
        image::imageops::FilterType::Nearest,
    );
    big.save(out)?;
    Ok(())
}

/// Alpha distribution; picks a sensible `alpha_threshold` for auto slicing.
fn print_alpha_histogram(image: &dark_assets::Image) {
    let mut buckets = [0usize; 9];
    let mut exact = std::collections::BTreeMap::<u8, usize>::new();
    for px in image.rgba.chunks_exact(4) {
        let a = px[3];
        buckets[(a as usize * 8) / 255] += 1;
        if a < 16 {
            *exact.entry(a).or_default() += 1;
        }
    }
    println!("{}x{}", image.width, image.height);
    for (i, count) in buckets.iter().enumerate() {
        println!("alpha ~{:>3}: {count}", i * 255 / 8);
    }
    println!("low alpha values: {exact:?}");
}

fn preview_sheet(project: &str, sheet: &str, out: &str) -> Result<(), Box<dyn std::error::Error>> {
    let project = Project::open(PathBuf::from(project))?;
    let loaded = project.load_sheet(sheet)?;
    let (w, h) = (loaded.image.width, loaded.image.height);
    let mut canvas =
        image::RgbaImage::from_raw(w, h, loaded.image.rgba).ok_or("image buffer size mismatch")?;
    let mut put = |x: u32, y: u32, color: [u8; 4]| {
        if x < w && y < h {
            canvas.put_pixel(x, y, image::Rgba(color));
        }
    };
    let magenta = [255, 0, 255, 255];
    let cyan = [0, 255, 255, 255];
    for frame in &loaded.sheet.frames {
        let r = frame.rect;
        for x in r.x..r.right() {
            put(x, r.y, magenta);
            put(x, r.bottom().saturating_sub(1), magenta);
        }
        for y in r.y..r.bottom() {
            put(r.x, y, magenta);
            put(r.right().saturating_sub(1), y, magenta);
        }
        let (px, py) = (r.x as f32 + frame.pivot.x, r.y as f32 + frame.pivot.y);
        for d in -2i32..=2 {
            put((px as i32 + d).max(0) as u32, py as u32, cyan);
            put(px as u32, (py as i32 + d).max(0) as u32, cyan);
        }
    }
    for (i, frame) in loaded.sheet.frames.iter().enumerate() {
        let r = frame.rect;
        println!("{i:>4}: x={:<5} y={:<5} w={:<5} h={}", r.x, r.y, r.w, r.h);
    }
    canvas.save(out)?;
    println!(
        "{}: {} frames, {} clips -> {out}",
        loaded.image_path.display(),
        loaded.sheet.frames.len(),
        loaded.sheet.clips.len()
    );
    Ok(())
}

/// Bakes the Spine sheet at `sheet` and writes its `baked` file.
fn bake_spine(project: &str, sheet: &str) -> Result<(), Box<dyn std::error::Error>> {
    let project = Project::open(project)?;
    let def = project.load_spine_def(sheet)?;
    let bake = dark_spine::bake(&project, &def)?;
    let out = project.path(&def.baked);
    bake.write(&out)?;
    let strikes = bake
        .clips
        .values()
        .filter(|c| c.events.iter().any(|(n, _)| n == "strike"))
        .count();
    let boxed = bake
        .clips
        .values()
        .filter(|c| !c.hitboxes.is_empty() || !c.hurtboxes.is_empty())
        .count();
    println!(
        "{}: {} clips ({strikes} with a strike, {boxed} with boxes), {:.0} px tall",
        out.display(),
        bake.clips.len(),
        bake.height
    );
    Ok(())
}

/// Draws a Spine sheet's skeleton at `clip` and `tick` on a grey ground, feet at the bottom
/// centre, four times enlarged: the triangles the game draws, filled on the CPU.
fn preview_spine(
    project: &str,
    sheet: &str,
    clip: &str,
    tick: &str,
    out: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    const ZOOM: f32 = 4.0;
    let project = Project::open(project)?;
    let loaded = project.load_sheet(sheet)?;
    let spine = loaded.spine.ok_or("not a Spine sheet")?;
    let baked = spine.bake.clips.get(clip).ok_or("no such clip")?;
    let tick: u32 = tick.parse()?;
    let rig = dark_spine::Rig::load(&project, &spine.def)?;
    let mut pose = dark_spine::Pose::new(&rig);
    pose.pose(&baked.animation, baked.looping, tick as f32 / 60.0);
    let size = (rig.height() * 1.6).ceil().max(16.0);
    let (w, h) = ((size * ZOOM) as u32, (size * ZOOM) as u32);
    let mut canvas = image::RgbaImage::from_pixel(w, h, image::Rgba([90, 110, 90, 255]));
    let feet = glam::Vec2::new(w as f32 / 2.0, h as f32 - 8.0);
    for mesh in pose.meshes(&rig) {
        let page = &rig.pages[mesh.page].image;
        for tri in mesh.vertices.chunks_exact(3) {
            let p: Vec<glam::Vec2> = tri.iter().map(|v| feet + v.position * ZOOM).collect();
            let (lo, hi) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
            let area = (p[1] - p[0]).perp_dot(p[2] - p[0]);
            if area.abs() < f32::EPSILON {
                continue;
            }
            for y in (lo.y.floor().max(0.0) as u32)..(hi.y.ceil().min(h as f32) as u32) {
                for x in (lo.x.floor().max(0.0) as u32)..(hi.x.ceil().min(w as f32) as u32) {
                    let q = glam::Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let b1 = (p[2] - p[1]).perp_dot(q - p[1]) / area;
                    let b2 = (p[0] - p[2]).perp_dot(q - p[2]) / area;
                    let b3 = 1.0 - b1 - b2;
                    if b1 < 0.0 || b2 < 0.0 || b3 < 0.0 {
                        continue;
                    }
                    let uv = tri[0].uv * b1 + tri[1].uv * b2 + tri[2].uv * b3;
                    let tx = ((uv.x * page.width as f32) as u32).min(page.width - 1);
                    let ty = ((uv.y * page.height as f32) as u32).min(page.height - 1);
                    let i = ((ty * page.width + tx) * 4) as usize;
                    let a = f32::from(page.rgba[i + 3]) / 255.0 * tri[0].color[3];
                    let dst = canvas.get_pixel_mut(x, y);
                    for c in 0..3 {
                        let src = f32::from(page.rgba[i + c]) * tri[0].color[c];
                        dst[c] = (src * a + f32::from(dst[c]) * (1.0 - a)) as u8;
                    }
                }
            }
        }
    }
    canvas.save(out)?;
    println!("{out}: {clip} at tick {tick} ({}x{})", w, h);
    Ok(())
}

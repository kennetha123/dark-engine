//! Project tooling.
//!
//! `dark-cli preview-sheet <project-dir> <sheet.ron> <out.png>` draws every sliced frame's box
//! and pivot over the sheet image, to check slicing without opening the game.
//!
//! `dark-cli simulate <project-dir> [--seed n] [--runs n] [--lang code]` fast-forwards the world
//! simulation (`world.ron`) through a year: the chronicle of one seed, or statistics over many.

mod simulate;

use std::path::PathBuf;
use std::process::ExitCode;

use dark_assets::Project;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
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
            eprintln!("       dark-cli image-info <image.png>");
            eprintln!("       dark-cli zoom <in.png> <out.png> <x> <y> <w> <h> <scale>");
            eprintln!(
                "       dark-cli simulate <project-dir> [--seed <n>] [--runs <n>] [--lang <code>]"
            );
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

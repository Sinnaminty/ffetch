//! Preprocesses the logo at compile time.
//!
//! The PNG is decoded, a solid background colour touching the image border is
//! keyed out (made transparent), and the result is box-filtered down to a small
//! RGBA buffer. The binary embeds that buffer and turns it into ASCII at
//! runtime, so the logo can be rendered at any size.
//!
//! Set `FFETCH_LOGO=/path/to/image.png` at build time to use a different image.

use std::{env, fs, io::BufReader, path::PathBuf};

/// Longest side of the embedded image, in pixels.
const WORK_SIZE: usize = 256;
/// Max RGB distance from the detected background colour that still counts as background.
const BG_TOLERANCE: i32 = 70;
/// Accent colour used when the image has no detectable background.
const DEFAULT_ACCENT: [u8; 3] = [196, 63, 86];

fn main() {
    let path = env::var("FFETCH_LOGO").unwrap_or_else(|_| "assets/logo.png".into());
    println!("cargo:rerun-if-env-changed=FFETCH_LOGO");
    println!("cargo:rerun-if-changed={path}");

    let (w, h, mut rgba) = decode(&path);
    let accent = key_out_background(w, h, &mut rgba).unwrap_or(DEFAULT_ACCENT);
    let (sw, sh, small) = downsample(w, h, &rgba, WORK_SIZE);

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out.join("logo.rgba"), small).unwrap();
    let [r, g, b] = accent;
    let src = format!(
        "pub const WIDTH: usize = {sw};\n\
         pub const HEIGHT: usize = {sh};\n\
         pub const ACCENT: Rgb = Rgb({r}, {g}, {b});\n\
         pub static PIXELS: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/logo.rgba\"));\n"
    );
    fs::write(out.join("logo.rs"), src).unwrap();
}

/// Decodes a PNG of any colour type into 8-bit RGBA.
fn decode(path: &str) -> (usize, usize, Vec<u8>) {
    let file = fs::File::open(path).unwrap_or_else(|e| panic!("cannot open logo {path}: {e}"));
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().expect("logo is not a valid PNG");
    let mut buf = vec![0; reader.output_buffer_size().expect("logo is too large")];
    let info = reader.next_frame(&mut buf).expect("logo is not a valid PNG");
    buf.truncate(info.buffer_size());

    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => unreachable!("palette images are expanded by the decoder"),
    };
    (info.width as usize, info.height as usize, rgba)
}

/// If the image is fully opaque and its border is dominated by one colour,
/// flood-fills that colour inward from the edges and makes it transparent.
/// Returns the background colour, which doubles as the accent colour.
fn key_out_background(w: usize, h: usize, rgba: &mut [u8]) -> Option<[u8; 3]> {
    if rgba.chunks(4).any(|p| p[3] < 255) {
        return None;
    }

    let border: Vec<usize> = (0..w)
        .flat_map(|x| [x, (h - 1) * w + x])
        .chain((0..h).flat_map(|y| [y * w, y * w + w - 1]))
        .collect();

    // Most common border colour, bucketed to 5 bits per channel.
    let mut buckets = vec![(0u32, [0u32; 3]); 1 << 15];
    for &i in &border {
        let p = &rgba[i * 4..i * 4 + 3];
        let key = (p[0] as usize >> 3) << 10 | (p[1] as usize >> 3) << 5 | p[2] as usize >> 3;
        let b = &mut buckets[key];
        b.0 += 1;
        for (sum, &v) in b.1.iter_mut().zip(p) {
            *sum += v as u32;
        }
    }
    let (count, sum) = buckets.into_iter().max_by_key(|b| b.0)?;
    let bg = sum.map(|s| (s / count) as u8);

    let is_bg = |i: usize| {
        let p = &rgba[i * 4..i * 4 + 3];
        let d: i32 = (0..3).map(|c| (p[c] as i32 - bg[c] as i32).pow(2)).sum();
        d <= BG_TOLERANCE * BG_TOLERANCE
    };
    // Slightly noisy backgrounds straddle buckets, so count everything close to the peak.
    if border.iter().filter(|&&i| is_bg(i)).count() * 3 < border.len() {
        return None;
    }
    let mut seen = vec![false; w * h];
    let mut stack: Vec<usize> = border.into_iter().filter(|&i| is_bg(i)).collect();
    while let Some(i) = stack.pop() {
        if seen[i] {
            continue;
        }
        seen[i] = true;
        let (x, y) = (i % w, i / w);
        let neighbours = [
            (x > 0).then(|| i - 1),
            (x + 1 < w).then(|| i + 1),
            (y > 0).then(|| i - w),
            (y + 1 < h).then(|| i + w),
        ];
        stack.extend(neighbours.into_iter().flatten().filter(|&n| !seen[n] && is_bg(n)));
    }
    for (i, _) in seen.iter().enumerate().filter(|(_, s)| **s) {
        rgba[i * 4 + 3] = 0;
    }
    Some(bg)
}

/// Area-averaging downsample so the longest side is at most `max` pixels.
/// Colour is averaged over opaque area only, so edges don't bleed the background in.
fn downsample(w: usize, h: usize, rgba: &[u8], max: usize) -> (usize, usize, Vec<u8>) {
    let scale = (w.max(h) as f64 / max as f64).max(1.0);
    let (tw, th) = ((w as f64 / scale).round() as usize, (h as f64 / scale).round() as usize);
    let (sx, sy) = (w as f64 / tw as f64, h as f64 / th as f64);
    let mut out = Vec::with_capacity(tw * th * 4);

    for ty in 0..th {
        let (y0, y1) = (ty as f64 * sy, (ty + 1) as f64 * sy);
        for tx in 0..tw {
            let (x0, x1) = (tx as f64 * sx, (tx + 1) as f64 * sx);
            let (mut area, mut alpha, mut rgb) = (0.0, 0.0, [0.0f64; 3]);
            for y in y0 as usize..(y1.ceil() as usize).min(h) {
                let wy = y1.min(y as f64 + 1.0) - y0.max(y as f64);
                for x in x0 as usize..(x1.ceil() as usize).min(w) {
                    let wt = wy * (x1.min(x as f64 + 1.0) - x0.max(x as f64));
                    let p = &rgba[(y * w + x) * 4..][..4];
                    let a = wt * p[3] as f64 / 255.0;
                    area += wt;
                    alpha += a;
                    for c in 0..3 {
                        rgb[c] += a * p[c] as f64;
                    }
                }
            }
            let rgb = rgb.map(|v| if alpha > 0.0 { (v / alpha).round() as u8 } else { 0 });
            out.extend(rgb);
            out.push((alpha / area * 255.0).round() as u8);
        }
    }
    (tw, th, out)
}

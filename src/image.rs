//! Turns a PNG into the small RGBA buffer the logo is rendered from.
//!
//! The image is decoded, a solid background colour touching the border is
//! keyed out (made transparent), and the result is box-filtered down to at most
//! `WORK_SIZE` pixels. `build.rs` includes this file too (for the embedded
//! logo), so `--image` and the build share one implementation. Keep it free of
//! dependencies other than std and `png`.

use std::{
    fs::File,
    io::{BufRead, BufReader, Seek},
    path::Path,
};

/// Longest side of the processed image, in pixels.
const WORK_SIZE: usize = 256;
/// Images whose longest side exceeds this are first shrunk to at most
/// `PRE_SIZE`, so large photos stay fast. Smaller images skip that step: it
/// changes the result, and the bundled 1254 px logo must not change.
const PRE_THRESHOLD: usize = 2048;
const PRE_SIZE: usize = 1024;
/// Bigger images are refused. The decoder's own limits don't cover the output
/// buffer, so without this a crafted header could exhaust memory.
const MAX_PIXELS: u64 = 1 << 26;
/// Max RGB distance from the detected background colour that still counts as background.
const BG_TOLERANCE: i32 = 70;

/// A processed image: 8-bit RGBA, at most `WORK_SIZE` pixels on its longest side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
    /// The colour keyed out of the background, if there was one.
    pub background: Option<[u8; 3]>,
}

/// Loads the PNG at `path` and runs the whole pipeline. With `keep_background`
/// the background is left in place.
pub fn process(path: &Path, keep_background: bool) -> Result<Image, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let (mut w, mut h, mut rgba) = decode(BufReader::new(file))?;
    if w.max(h) > PRE_THRESHOLD {
        (w, h, rgba) = shrink(w, h, &rgba, pre_factor(w, h));
    }
    let background = if keep_background {
        None
    } else {
        key_out_background(w, h, &mut rgba)
    };
    let (width, height, pixels) = downsample(w, h, &rgba, WORK_SIZE);
    Ok(Image {
        width,
        height,
        pixels,
        background,
    })
}

/// Decodes a PNG of any colour type into 8-bit RGBA.
fn decode(input: impl BufRead + Seek) -> Result<(usize, usize, Vec<u8>), String> {
    let invalid = |e: png::DecodingError| match e {
        png::DecodingError::IoError(e) => e.to_string(),
        e => format!("not a valid PNG ({e})"),
    };
    let mut decoder = png::Decoder::new(input);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(invalid)?;
    let (w, h) = reader.info().size();
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err(format!("image is too large ({w}x{h})"));
    }
    let mut buf = vec![0; reader.output_buffer_size().ok_or("image is too large")?];
    let info = reader.next_frame(&mut buf).map_err(invalid)?;
    buf.truncate(info.buffer_size());

    let (w, h) = (info.width as usize, info.height as usize);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => to_rgba(&buf, 3, w * h, |p| [p[0], p[1], p[2], 255]),
        png::ColorType::GrayscaleAlpha => to_rgba(&buf, 2, w * h, |p| [p[0], p[0], p[0], p[1]]),
        png::ColorType::Grayscale => to_rgba(&buf, 1, w * h, |p| [p[0], p[0], p[0], 255]),
        // The decoder expands palettes, so this only guards against surprises.
        png::ColorType::Indexed => return Err("unexpected indexed output from the decoder".into()),
    };
    Ok((w, h, rgba))
}

/// Expands `pixels` pixels of `channels` bytes each into RGBA.
fn to_rgba(buf: &[u8], channels: usize, pixels: usize, f: impl Fn(&[u8]) -> [u8; 4]) -> Vec<u8> {
    // Reserving up front matters: for a large photo this is hundreds of MB.
    let mut out = Vec::with_capacity(pixels * 4);
    for p in buf.chunks_exact(channels) {
        out.extend_from_slice(&f(p));
    }
    out
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
        stack.extend(
            neighbours
                .into_iter()
                .flatten()
                .filter(|&n| !seen[n] && is_bg(n)),
        );
    }
    for (i, _) in seen.iter().enumerate().filter(|(_, s)| **s) {
        rgba[i * 4 + 3] = 0;
    }
    Some(bg)
}

/// Smallest integer factor that brings the longest side down to `PRE_SIZE`.
fn pre_factor(w: usize, h: usize) -> usize {
    w.max(h).div_ceil(PRE_SIZE)
}

/// Box filter by an integer factor, in integer arithmetic. Much cheaper than
/// `downsample` on big images; like it, colour is averaged over opaque area
/// only. Opaque input stays exactly opaque, so the background can still be keyed.
fn shrink(w: usize, h: usize, rgba: &[u8], factor: usize) -> (usize, usize, Vec<u8>) {
    let (tw, th) = (w.div_ceil(factor), h.div_ceil(factor));
    let mut out = Vec::with_capacity(tw * th * 4);
    // Per output pixel of the current row: alpha-weighted R, G, B, total alpha, pixel count.
    let mut sums = vec![[0u64; 5]; tw];
    for ty in 0..th {
        sums.fill([0; 5]);
        for row in rgba.chunks_exact(w * 4).skip(ty * factor).take(factor) {
            for (s, block) in sums.iter_mut().zip(row.chunks(factor * 4)) {
                for p in block.as_chunks::<4>().0 {
                    let a = p[3] as u64;
                    for c in 0..3 {
                        s[c] += a * p[c] as u64;
                    }
                    s[3] += a;
                    s[4] += 1;
                }
            }
        }
        for s in &sums {
            let channel = |c: u64| (c + s[3] / 2).checked_div(s[3]).map_or(0, |v| v as u8);
            out.extend([
                channel(s[0]),
                channel(s[1]),
                channel(s[2]),
                ((s[3] + s[4] / 2) / s[4]) as u8,
            ]);
        }
    }
    (tw, th, out)
}

/// Area-averaging downsample so the longest side is at most `max` pixels.
/// Colour is averaged over opaque area only, so edges don't bleed the background in.
fn downsample(w: usize, h: usize, rgba: &[u8], max: usize) -> (usize, usize, Vec<u8>) {
    let scale = (w.max(h) as f64 / max as f64).max(1.0);
    // Very thin images would otherwise round down to zero pixels.
    let tw = ((w as f64 / scale).round() as usize).max(1);
    let th = ((h as f64 / scale).round() as usize).max(1);
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
            let rgb = rgb.map(|v| {
                if alpha > 0.0 {
                    (v / alpha).round() as u8
                } else {
                    0
                }
            });
            out.extend(rgb);
            out.push((alpha / area * 255.0).round() as u8);
        }
    }
    (tw, th, out)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Cursor;

    use super::*;

    /// Encodes RGBA pixels as a PNG.
    pub(crate) fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(rgba).unwrap();
        writer.finish().unwrap();
        out
    }

    /// An opaque `w`x`h` image filled with `bg`, with `fg` drawn where `draw(x, y)`.
    fn picture(
        w: usize,
        h: usize,
        bg: [u8; 3],
        fg: [u8; 3],
        draw: impl Fn(usize, usize) -> bool,
    ) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| {
                let [r, g, b] = if draw(i % w, i / w) { fg } else { bg };
                [r, g, b, 255]
            })
            .collect()
    }

    fn alpha(rgba: &[u8], w: usize, x: usize, y: usize) -> u8 {
        rgba[(y * w + x) * 4 + 3]
    }

    const RED: [u8; 3] = [195, 62, 88];
    const CREAM: [u8; 3] = [242, 215, 197];

    #[test]
    fn background_is_keyed_out_from_the_border() {
        let mut rgba = picture(6, 6, RED, CREAM, |x, y| {
            (2..4).contains(&x) && (2..4).contains(&y)
        });
        assert_eq!(key_out_background(6, 6, &mut rgba), Some(RED));
        assert_eq!(alpha(&rgba, 6, 0, 0), 0);
        assert_eq!(alpha(&rgba, 6, 1, 4), 0);
        assert_eq!(alpha(&rgba, 6, 2, 2), 255);
        assert_eq!(alpha(&rgba, 6, 3, 3), 255);
    }

    #[test]
    fn enclosed_background_colour_is_kept() {
        // A cream ring with background colour inside it.
        let ring = |x: usize, y: usize| {
            (1..6).contains(&x) && (1..6).contains(&y) && (x == 1 || x == 5 || y == 1 || y == 5)
        };
        let mut rgba = picture(7, 7, RED, CREAM, ring);
        assert_eq!(key_out_background(7, 7, &mut rgba), Some(RED));
        assert_eq!(alpha(&rgba, 7, 0, 0), 0);
        assert_eq!(alpha(&rgba, 7, 1, 1), 255);
        assert_eq!(alpha(&rgba, 7, 3, 3), 255);
    }

    #[test]
    fn no_background_without_a_dominant_border_colour() {
        // Already transparent somewhere.
        let mut rgba = picture(4, 4, RED, CREAM, |_, _| false);
        rgba[3] = 0;
        let before = rgba.clone();
        assert_eq!(key_out_background(4, 4, &mut rgba), None);
        assert_eq!(rgba, before);
        // A busy border: four far-apart colours, none on a third of the border.
        let colours = [[0, 0, 0], [255, 255, 255], [255, 0, 0], [0, 0, 255]];
        let mut rgba: Vec<u8> = (0..64)
            .flat_map(|i| colours[(i % 8 + i / 8) % 4].into_iter().chain([255]))
            .collect();
        let before = rgba.clone();
        assert_eq!(key_out_background(8, 8, &mut rgba), None);
        assert_eq!(rgba, before);
    }

    #[test]
    fn downsample_dimensions() {
        let dims = |w: usize, h: usize, max: usize| {
            let (tw, th, out) = downsample(w, h, &vec![255; w * h * 4], max);
            assert_eq!(out.len(), tw * th * 4);
            (tw, th)
        };
        assert_eq!(dims(1254, 1254, 256), (256, 256));
        assert_eq!(dims(1000, 500, 256), (256, 128));
        assert_eq!(dims(3000, 2000, 1024), (1024, 683));
        assert_eq!(
            dims(100, 50, 256),
            (100, 50),
            "small images are not enlarged"
        );
        assert_eq!(dims(1, 2000, 256), (1, 256));
        assert_eq!(dims(3000, 2, 256), (256, 1), "never zero pixels wide");
    }

    #[test]
    fn downsample_averages_opaque_colour_only() {
        // Two opaque pixels and two transparent ones become one half-covered pixel.
        let rgba = [200, 0, 0, 255, 100, 0, 0, 255, 0, 0, 255, 0, 0, 0, 255, 0];
        assert_eq!(downsample(2, 2, &rgba, 1), (1, 1, vec![150, 0, 0, 128]));
    }

    #[test]
    fn shrink_by_integer_factor() {
        let opaque = picture(3000, 2, RED, CREAM, |x, _| x % 2 == 0);
        let (w, h, out) = shrink(3000, 2, &opaque, pre_factor(3000, 2));
        assert_eq!((w, h), (1000, 1));
        assert!(
            out.chunks(4).all(|p| p[3] == 255),
            "opaque stays fully opaque for keying"
        );
        // Two opaque pixels and two transparent ones, plus a partial block at the edge.
        let rgba = [
            200, 0, 0, 255, 100, 0, 0, 255, 0, 0, 255, 255, 9, 9, 9, 0, 9, 9, 9, 0, 9, 9, 9, 0,
        ];
        assert_eq!(
            shrink(3, 2, &rgba, 2),
            (2, 1, vec![150, 0, 0, 128, 0, 0, 255, 128])
        );
        assert_eq!(pre_factor(2049, 10), 3);
        assert_eq!(pre_factor(4096, 4096), 4);
        assert_eq!(pre_factor(4097, 100), 5);
    }

    #[test]
    fn decodes_png() {
        let rgba = picture(3, 2, RED, CREAM, |x, _| x == 1);
        let (w, h, out) = decode(Cursor::new(png(3, 2, &rgba))).unwrap();
        assert_eq!((w, h), (3, 2));
        assert_eq!(out, rgba);
    }

    #[test]
    fn bad_input_is_an_error() {
        assert!(decode(Cursor::new(b"")).is_err());
        assert!(decode(Cursor::new(b"GIF89a, definitely not a PNG")).is_err());
        let full = png(16, 16, &picture(16, 16, RED, CREAM, |x, y| x == y));
        for cut in [8, 30, full.len() / 2, full.len() - 13] {
            assert!(
                decode(Cursor::new(&full[..cut])).is_err(),
                "truncated at {cut}"
            );
        }
        assert!(process(Path::new("/nonexistent/ffetch.png"), false).is_err());
    }
}

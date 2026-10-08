//! Renders the embedded logo image as coloured ASCII art or half-block pixels.
//!
//! The image itself is preprocessed by `build.rs`; this module only samples it.

use crate::term::{ColorMode, RESET, Rgb};

include!(concat!(env!("OUT_DIR"), "/logo.rs"));

/// Terminal cells are roughly twice as tall as they are wide.
const CELL_ASPECT: f32 = 0.5;
/// Characters from dim to bright, picked by each cell's luminance.
const RAMP: &[u8] = b":;ox%#@";
/// Gamma applied to luminance before picking from the ramp; < 1 gives dark
/// regions denser characters so they read as shapes rather than gaps.
const RAMP_GAMMA: f32 = 0.7;
/// Cells covered less than this are left blank.
const MIN_COVERAGE: f32 = 0.25;
/// Cells covered less than this sit on the silhouette and get a shape-matched glyph.
const EDGE_COVERAGE: f32 = 0.6;
/// HSL lightness floor so dark fur and outlines stay visible on dark terminals.
const ASCII_LIFT: f32 = 0.25;
const BLOCKS_LIFT: f32 = 0.08;

const GLYPH_W: usize = 6;
const GLYPH_H: usize = 12;
const GLYPH_LEN: usize = GLYPH_W * GLYPH_H;

/// Ink coverage of glyphs used along the silhouette, sampled from DejaVu Sans
/// Mono on a 6x12 grid: one hex digit (0-f) per sub-cell, one group per row.
/// `#` stands in for the dense ramp characters.
const EDGE_GLYPHS: &[(char, &str)] = &[
    ('#', "000000 000000 006363 00b1b0 5bebeb 048480 497972 7d8d83 0b1b00 151400 000000 000000"),
    ('_', "000000 000000 000000 000000 000000 000000 000000 000000 000000 000000 000000 bbbbbb"),
    ('.', "000000 000000 000000 000000 000000 000000 000000 000000 009900 005500 000000 000000"),
    (',', "000000 000000 000000 000000 000000 000000 000000 000000 008a00 00a700 00c100 000000"),
    ('\'', "000000 000000 005500 006600 006600 000000 000000 000000 000000 000000 000000 000000"),
    ('`', "000000 034000 00a200 001200 000000 000000 000000 000000 000000 000000 000000 000000"),
    ('-', "000000 000000 000000 000000 000000 000000 028820 014410 000000 000000 000000 000000"),
    ('/', "000000 000000 000091 0004a0 000b30 003b00 00a400 02c000 095000 1d0000 340000 000000"),
    ('\\', "000000 000000 470000 0d1000 077000 01d000 008600 001c00 000950 0002c0 000061 000000"),
    ('|', "000000 000000 006600 006600 006600 006600 006600 006600 006600 006600 006600 006600"),
    ('(', "000000 000000 000b10 004900 009400 00d200 00e100 00d200 009400 004900 000b10 000000"),
    (')', "000000 000000 01b000 009400 004a00 001d00 000e00 001d00 004a00 009400 01b000 000000"),
    ('^', "000000 000000 008800 079970 4a00a4 000000 000000 000000 000000 000000 000000 000000"),
    ('"', "000000 000000 046540 058760 058760 000000 000000 000000 000000 000000 000000 000000"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Ascii,
    Blocks,
}

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    fg: Option<Rgb>,
    /// Set only for half-block cells whose lower half is filled too.
    bg: Option<Rgb>,
}

const BLANK: Cell = Cell { ch: ' ', fg: None, bg: None };

/// The image's background colour (or a fallback), used to colour the info labels.
pub fn accent() -> Rgb {
    ACCENT
}

/// Number of terminal rows the logo occupies at a given width.
pub fn rows_for(cols: usize) -> usize {
    ((cols as f32 * HEIGHT as f32 / WIDTH as f32 * CELL_ASPECT).round() as usize).max(1)
}

/// Widest logo that fits in `rows` terminal rows.
pub fn cols_for(rows: usize) -> usize {
    (rows as f32 * WIDTH as f32 / HEIGHT as f32 / CELL_ASPECT) as usize
}

/// Renders the logo `cols` cells wide. Every line is exactly `cols` columns.
pub fn render(style: Style, cols: usize, mode: ColorMode) -> Vec<String> {
    let rows = rows_for(cols);
    let (cw, ch) = (WIDTH as f32 / cols as f32, HEIGHT as f32 / rows as f32);
    let glyphs = edge_glyphs();

    (0..rows)
        .map(|r| {
            let cells: Vec<Cell> = (0..cols)
                .map(|c| {
                    let (x, y) = (c as f32 * cw, r as f32 * ch);
                    match style {
                        Style::Ascii => ascii_cell(x, y, cw, ch, &glyphs),
                        Style::Blocks => block_cell(x, y, cw, ch),
                    }
                })
                .collect();
            encode(&cells, mode)
        })
        .collect()
}

fn ascii_cell(x: f32, y: f32, w: f32, h: f32, glyphs: &[(char, [f32; GLYPH_LEN])]) -> Cell {
    let (coverage, rgb) = sample(x, y, x + w, y + h);
    if coverage < MIN_COVERAGE {
        return BLANK;
    }
    let lum = (0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]) / 255.0;
    let level = (lum.powf(RAMP_GAMMA) * RAMP.len() as f32) as usize;
    let mut ch = RAMP[level.min(RAMP.len() - 1)] as char;

    if coverage < EDGE_COVERAGE {
        let (sw, sh) = (w / GLYPH_W as f32, h / GLYPH_H as f32);
        let mut target = [0.0; GLYPH_LEN];
        for (i, t) in target.iter_mut().enumerate() {
            let (sx, sy) = (x + (i % GLYPH_W) as f32 * sw, y + (i / GLYPH_W) as f32 * sh);
            *t = sample(sx, sy, sx + sw, sy + sh).0;
        }
        match best_glyph(&target, glyphs) {
            ' ' => return BLANK,
            '#' => {}
            g => ch = g,
        }
    }
    Cell { ch, fg: Some(lift(rgb, ASCII_LIFT)), bg: None }
}

fn block_cell(x: f32, y: f32, w: f32, h: f32) -> Cell {
    let half = |y: f32| {
        let (coverage, rgb) = sample(x, y, x + w, y + h / 2.0);
        (coverage >= 0.5).then(|| lift(rgb, BLOCKS_LIFT))
    };
    match (half(y), half(y + h / 2.0)) {
        (Some(top), bottom) => Cell { ch: '▀', fg: Some(top), bg: bottom },
        (None, Some(bottom)) => Cell { ch: '▄', fg: Some(bottom), bg: None },
        (None, None) => BLANK,
    }
}

/// Area-weighted average over a rectangle of the embedded image, in pixel units.
/// Returns the fraction covered by the logo and its mean colour over that part.
fn sample(x0: f32, y0: f32, x1: f32, y1: f32) -> (f32, [f32; 3]) {
    let (mut area, mut alpha, mut rgb) = (0.0, 0.0, [0.0f32; 3]);
    for y in y0 as usize..(y1.ceil() as usize).min(HEIGHT) {
        let wy = y1.min(y as f32 + 1.0) - y0.max(y as f32);
        for x in x0 as usize..(x1.ceil() as usize).min(WIDTH) {
            let wt = wy * (x1.min(x as f32 + 1.0) - x0.max(x as f32));
            let p = &PIXELS[(y * WIDTH + x) * 4..][..4];
            let a = wt * p[3] as f32 / 255.0;
            area += wt;
            alpha += a;
            for c in 0..3 {
                rgb[c] += a * p[c] as f32;
            }
        }
    }
    if alpha <= 0.0 {
        return (0.0, [0.0; 3]);
    }
    (alpha / area, rgb.map(|v| v / alpha))
}

fn edge_glyphs() -> Vec<(char, [f32; GLYPH_LEN])> {
    EDGE_GLYPHS
        .iter()
        .map(|&(c, hex)| {
            let mut mask = [0.0; GLYPH_LEN];
            let digits = hex.chars().filter_map(|d| d.to_digit(16));
            for (m, d) in mask.iter_mut().zip(digits) {
                *m = d as f32 / 15.0;
            }
            (c, mask)
        })
        .collect()
}

/// Picks the glyph whose shape, at its best brightness, is closest to the
/// coverage pattern. Returns `' '` when leaving the cell empty fits best.
fn best_glyph(target: &[f32; GLYPH_LEN], glyphs: &[(char, [f32; GLYPH_LEN])]) -> char {
    let mut best = (' ', target.iter().map(|t| t * t).sum::<f32>());
    for (c, g) in glyphs {
        let dot: f32 = target.iter().zip(g).map(|(t, g)| t * g).sum();
        let norm: f32 = g.iter().map(|g| g * g).sum();
        let peak = g.iter().cloned().fold(0.0, f32::max);
        let k = (dot / norm).clamp(0.0, 1.0 / peak);
        let err: f32 = target.iter().zip(g).map(|(t, g)| (t - k * g).powi(2)).sum();
        if err < best.1 {
            best = (*c, err);
        }
    }
    best.0
}

/// Raises HSL lightness to at least `floor`, keeping hue and saturation.
fn lift([r, g, b]: [f32; 3], floor: f32) -> Rgb {
    let [r, g, b] = [r, g, b].map(|v| v / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let (l, chroma) = ((max + min) / 2.0, max - min);
    let sat = if chroma > 0.0 { (chroma / (1.0 - (2.0 * l - 1.0).abs())).min(1.0) } else { 0.0 };
    let l2 = floor + (1.0 - floor) * l;
    let chroma2 = (1.0 - (2.0 * l2 - 1.0).abs()) * sat;
    let channel = |v: f32| {
        let hue = if chroma > 0.0 { (v - min) / chroma } else { 0.0 };
        ((l2 - chroma2 / 2.0 + chroma2 * hue) * 255.0).round() as u8
    };
    Rgb(channel(r), channel(g), channel(b))
}

/// Turns a row of cells into a string, emitting colour codes only on change.
fn encode(cells: &[Cell], mode: ColorMode) -> String {
    let mut s = String::new();
    if mode == ColorMode::None {
        // Without colours a two-tone half block can only be drawn as a full block.
        s.extend(cells.iter().map(|c| if c.bg.is_some() { '█' } else { c.ch }));
        return s;
    }
    // Compare escape codes rather than colours: in 256-colour mode many map to one code.
    let (mut fg, mut bg) = (String::new(), "\x1b[49m".to_string());
    for c in cells {
        let code = c.bg.map_or_else(|| "\x1b[49m".to_string(), |b| mode.bg(b));
        if code != bg {
            s += &code;
            bg = code;
        }
        if let Some(f) = c.fg.filter(|_| c.ch != ' ') {
            let code = mode.fg(f);
            if code != fg {
                s += &code;
                fg = code;
            }
        }
        s.push(c.ch);
    }
    s + RESET
}

mod cache;
mod image;
mod info;
mod logo;
mod palette;
mod term;

use std::{
    io::{self, Write},
    path::PathBuf,
    process::exit,
};

use logo::{LogoImage, Style};
use palette::{Palette, Roles, Theme};
use term::{ColorMode, RESET};

/// Spaces between the logo and the info column.
const GAP: usize = 3;
const DEFAULT_LOGO_COLS: usize = 48;
const MIN_LOGO_COLS: usize = 16;
/// The info column is truncated down to this before the logo is dropped entirely.
const MIN_INFO_COLS: usize = 32;
/// Width of one colour swatch at the bottom of the info column.
const SWATCH_COLS: usize = 3;

const HELP: &str = "\
ffetch - a neofetch-style system info tool

Usage: ffetch [options]

Options:
  -l, --logo <style>     Logo style: ascii (default), blocks, none
  -s, --size <cols>      Logo width in columns (default 48; shrinks to fit the terminal)
      --image <path>     Use a PNG image as the logo and take the colors from it
      --keep-background  With --image, keep the image's background instead of removing it
      --swatches <kind>  Color swatches: palette (default), ansi, none
      --no-color         Disable colors (NO_COLOR is honored too)
  -h, --help             Show this help
  -V, --version          Show the version
";

/// What the swatch rows at the bottom of the info column show.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Swatches {
    /// One row with the palette taken from the logo.
    Palette,
    /// Two rows with the 16 ANSI colors.
    Ansi,
    None,
}

struct Args {
    logo: Option<Style>,
    size: Option<usize>,
    no_color: bool,
    image: Option<PathBuf>,
    keep_background: bool,
    swatches: Swatches,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        logo: Some(Style::Ascii),
        size: None,
        no_color: false,
        image: None,
        keep_background: false,
        swatches: Swatches::Palette,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        // Accept both "--opt value" and "--opt=value".
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg, None),
        };
        let mut value = || inline.clone().or_else(|| argv.next());
        match flag.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                exit(0);
            }
            "-V" | "--version" => {
                println!("ffetch {}", env!("CARGO_PKG_VERSION"));
                exit(0);
            }
            "--no-color" => args.no_color = true,
            "-l" | "--logo" => {
                args.logo = match value().as_deref() {
                    Some("ascii") => Some(Style::Ascii),
                    Some("blocks") => Some(Style::Blocks),
                    Some("none") => None,
                    v => return Err(expected("--logo", "ascii, blocks or none", v)),
                }
            }
            "--image" => args.image = Some(value().ok_or("--image expects a path")?.into()),
            "--keep-background" => args.keep_background = true,
            "--swatches" => {
                args.swatches = match value().as_deref() {
                    Some("palette") => Swatches::Palette,
                    Some("ansi") => Swatches::Ansi,
                    Some("none") => Swatches::None,
                    v => return Err(expected("--swatches", "palette, ansi or none", v)),
                }
            }
            "-s" | "--size" => {
                let cols = value()
                    .and_then(|v| v.parse().ok())
                    .filter(|&n| n >= MIN_LOGO_COLS);
                args.size =
                    Some(cols.ok_or(format!("--size expects a number >= {MIN_LOGO_COLS}"))?);
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(args)
}

/// Error for an option that takes one of a few words.
fn expected(flag: &str, choices: &str, got: Option<&str>) -> String {
    let got = got.map_or("nothing".into(), |v| format!("'{v}'"));
    format!("{flag} expects {choices}, got {got}")
}

fn main() {
    let args = parse_args().unwrap_or_else(|e| {
        eprintln!("ffetch: {e}\nTry 'ffetch --help'.");
        exit(2);
    });
    let mode = ColorMode::detect(args.no_color);
    let image = load_image(&args);
    let palette = palette::extract(&image.pixels, image.background);
    let roles = Theme::default().roles(&palette);
    let swatches = swatch_rows(args.swatches, &palette, mode);
    let sys = info::collect();
    let term = term::size();

    let info_cols = natural_info_width(&sys, &swatches);
    let logo_cols = args
        .logo
        .and_then(|_| fit_logo(&image, args.size, info_cols, term));
    let max_info_cols = term.map(|(w, _)| w.saturating_sub(logo_cols.map_or(0, |c| c + GAP)));

    let info = info_lines(&sys, mode, roles, &swatches, max_info_cols);
    let logo = match (args.logo, logo_cols) {
        (Some(style), Some(cols)) => image.render(style, cols, mode),
        _ => Vec::new(),
    };

    let mut out = String::new();
    for i in 0..logo.len().max(info.len()) {
        let line = info.get(i).map_or("", String::as_str);
        if let Some(cols) = logo_cols {
            match logo.get(i) {
                Some(l) => out += l,
                None if !line.is_empty() => out += &" ".repeat(cols),
                None => {}
            }
            if !line.is_empty() {
                out += &" ".repeat(GAP);
            }
        }
        out += line;
        out.push('\n');
    }
    out.push('\n');
    // Ignore errors such as a closed pipe (`ffetch | head`).
    let _ = io::stdout().lock().write_all(out.as_bytes());
}

/// The logo image: `--image` if it loads, otherwise the embedded one. A broken
/// image costs one warning, never the run.
fn load_image(args: &Args) -> LogoImage {
    let Some(path) = &args.image else {
        return LogoImage::embedded();
    };
    match cache::image(path, args.keep_background, cache::dir().as_deref()) {
        Ok(img) => img.into(),
        Err(e) => {
            eprintln!("ffetch: can't load image {path:?}: {e}; using the built-in logo");
            LogoImage::embedded()
        }
    }
}

/// Chooses the logo width: the requested (or default) size, shrunk so the logo
/// and the info column fit the terminal. `None` means there is no room for it.
fn fit_logo(
    image: &LogoImage,
    requested: Option<usize>,
    info_cols: usize,
    term: Option<(usize, usize)>,
) -> Option<usize> {
    let mut cols = requested.unwrap_or(DEFAULT_LOGO_COLS);
    let Some((width, height)) = term else {
        return Some(cols);
    };

    if requested.is_none() {
        // Leave a row for the trailing blank line and one for the prompt.
        cols = cols.min(image.cols_for(height.saturating_sub(2)));
    }
    // Shrink the logo first; only once it is at its minimum, squeeze the info.
    let fits_full_info = width.saturating_sub(GAP + info_cols);
    if cols > fits_full_info {
        cols = fits_full_info.max(MIN_LOGO_COLS).min(cols);
    }
    cols = cols.min(width.saturating_sub(GAP + info_cols.min(MIN_INFO_COLS)));
    (cols >= MIN_LOGO_COLS).then_some(cols)
}

fn natural_info_width(sys: &info::System, swatches: &[Vec<String>]) -> usize {
    let title = sys.user.chars().count() + 1 + sys.host.chars().count();
    let fields = sys
        .fields
        .iter()
        .map(|(label, value)| label.len() + 2 + value.chars().count());
    let swatches = swatches.iter().map(|row| row.len() * SWATCH_COLS);
    fields.chain(swatches).chain([title]).max().unwrap_or(0)
}

/// Background colour codes for each swatch row. There are none without colour.
fn swatch_rows(kind: Swatches, palette: &Palette, mode: ColorMode) -> Vec<Vec<String>> {
    if mode == ColorMode::None {
        return Vec::new();
    }
    let mut rows: Vec<Vec<String>> = match kind {
        Swatches::Palette => vec![palette.colors.iter().map(|&c| mode.bg(c)).collect()],
        Swatches::Ansi => vec![
            (0..8).map(|i| format!("\x1b[4{i}m")).collect(),
            (0..8).map(|i| format!("\x1b[10{i}m")).collect(),
        ],
        Swatches::None => Vec::new(),
    };
    // An image with no opaque pixels has an empty palette.
    rows.retain(|r| !r.is_empty());
    rows
}

fn info_lines(
    sys: &info::System,
    mode: ColorMode,
    roles: Roles,
    swatches: &[Vec<String>],
    max_cols: Option<usize>,
) -> Vec<String> {
    let title_len = sys.user.chars().count() + 1 + sys.host.chars().count();
    let mut lines = vec![
        format!(
            "{}{}{}",
            mode.paint(&sys.user, roles.accent),
            mode.tint("@", roles.muted),
            mode.paint(&sys.host, roles.secondary)
        ),
        mode.tint(&"-".repeat(title_len), roles.muted),
    ];

    for (label, value) in &sys.fields {
        let room = max_cols.map(|m| m.saturating_sub(label.len() + 2));
        lines.push(format!(
            "{}: {}",
            mode.paint(label, roles.accent),
            truncate(value, room)
        ));
    }

    if !swatches.is_empty() {
        lines.push(String::new());
    }
    let block = " ".repeat(SWATCH_COLS);
    for row in swatches {
        let row: String = row.iter().map(|code| format!("{code}{block}")).collect();
        lines.push(row + RESET);
    }
    lines
}

fn truncate(s: &str, max: Option<usize>) -> String {
    match max {
        Some(max) if s.chars().count() > max => {
            let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
            t.push('…');
            t
        }
        _ => s.to_string(),
    }
}

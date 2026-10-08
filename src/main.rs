mod info;
mod logo;
mod term;

use std::{
    io::{self, Write},
    process::exit,
};

use logo::Style;
use term::{ColorMode, RESET, Rgb};

/// Spaces between the logo and the info column.
const GAP: usize = 3;
const DEFAULT_LOGO_COLS: usize = 48;
const MIN_LOGO_COLS: usize = 16;
/// The info column is truncated down to this before the logo is dropped entirely.
const MIN_INFO_COLS: usize = 32;
/// Width of the colour-swatch rows at the bottom of the info column.
const SWATCH_COLS: usize = 24;

const HELP: &str = "\
ffetch - a neofetch-style system info tool

Usage: ffetch [options]

Options:
  -l, --logo <style>   Logo style: ascii (default), blocks, none
  -s, --size <cols>    Logo width in columns (default 48; shrinks to fit the terminal)
      --no-color       Disable colors (NO_COLOR is honored too)
  -h, --help           Show this help
  -V, --version        Show the version
";

struct Args {
    logo: Option<Style>,
    size: Option<usize>,
    no_color: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { logo: Some(Style::Ascii), size: None, no_color: false };
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
                    v => {
                        let got = v.map_or("nothing".into(), |v| format!("'{v}'"));
                        return Err(format!("--logo expects ascii, blocks or none, got {got}"));
                    }
                }
            }
            "-s" | "--size" => {
                let cols = value().and_then(|v| v.parse().ok()).filter(|&n| n >= MIN_LOGO_COLS);
                args.size = Some(cols.ok_or(format!("--size expects a number >= {MIN_LOGO_COLS}"))?);
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(args)
}

fn main() {
    let args = parse_args().unwrap_or_else(|e| {
        eprintln!("ffetch: {e}\nTry 'ffetch --help'.");
        exit(2);
    });
    let mode = ColorMode::detect(args.no_color);
    let accent = logo::accent();
    let sys = info::collect();
    let term = term::size();

    let info_cols = natural_info_width(&sys);
    let logo_cols = args.logo.and_then(|_| fit_logo(args.size, info_cols, term));
    let max_info_cols = term.map(|(w, _)| w.saturating_sub(logo_cols.map_or(0, |c| c + GAP)));

    let info = info_lines(&sys, mode, accent, max_info_cols);
    let logo = match (args.logo, logo_cols) {
        (Some(style), Some(cols)) => logo::render(style, cols, mode),
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

/// Chooses the logo width: the requested (or default) size, shrunk so the logo
/// and the info column fit the terminal. `None` means there is no room for it.
fn fit_logo(requested: Option<usize>, info_cols: usize, term: Option<(usize, usize)>) -> Option<usize> {
    let mut cols = requested.unwrap_or(DEFAULT_LOGO_COLS);
    let Some((width, height)) = term else { return Some(cols) };

    if requested.is_none() {
        // Leave a row for the trailing blank line and one for the prompt.
        cols = cols.min(logo::cols_for(height.saturating_sub(2)));
    }
    // Shrink the logo first; only once it is at its minimum, squeeze the info.
    let fits_full_info = width.saturating_sub(GAP + info_cols);
    if cols > fits_full_info {
        cols = fits_full_info.max(MIN_LOGO_COLS).min(cols);
    }
    cols = cols.min(width.saturating_sub(GAP + info_cols.min(MIN_INFO_COLS)));
    (cols >= MIN_LOGO_COLS).then_some(cols)
}

fn natural_info_width(sys: &info::System) -> usize {
    let title = sys.user.chars().count() + 1 + sys.host.chars().count();
    let fields = sys.fields.iter().map(|(label, value)| label.len() + 2 + value.chars().count());
    fields.chain([title, SWATCH_COLS]).max().unwrap_or(0)
}

fn info_lines(sys: &info::System, mode: ColorMode, accent: Rgb, max_cols: Option<usize>) -> Vec<String> {
    let title_len = sys.user.chars().count() + 1 + sys.host.chars().count();
    let mut lines = vec![
        format!("{}@{}", mode.paint(&sys.user, accent), mode.paint(&sys.host, accent)),
        "-".repeat(title_len),
    ];

    for (label, value) in &sys.fields {
        let room = max_cols.map(|m| m.saturating_sub(label.len() + 2));
        lines.push(format!("{}: {}", mode.paint(label, accent), truncate(value, room)));
    }

    if mode != ColorMode::None {
        lines.push(String::new());
        lines.push((0..8).map(|i| format!("\x1b[4{i}m   ")).collect::<String>() + RESET);
        lines.push((0..8).map(|i| format!("\x1b[10{i}m   ")).collect::<String>() + RESET);
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

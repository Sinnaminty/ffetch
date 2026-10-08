mod cache;
mod command;
mod image;
mod info;
mod layout;
mod logo;
mod palette;
mod socket;
mod term;
mod wsl;

use std::{
    io::{self, Write},
    path::PathBuf,
    process::exit,
};

use layout::{Layout, MIN_LOGO_COLS, Options, Swatches};
use logo::{LogoImage, Style};
use palette::Theme;
use term::ColorMode;

const HELP: &str = "\
ffetch - a neofetch-style system info tool

Usage: ffetch [options]

Options:
  -l, --logo <style>     Logo style: ascii (default), blocks, none
  -s, --size <cols>      Logo width in columns (default 48; shrinks to fit the terminal)
      --image <path>     Use a PNG image as the logo and take the colors from it
      --keep-background  With --image, keep the image's background instead of removing it
      --layout <kind>    Logo placement: auto (default; beside the info if it fits,
                         else above it), side, stacked
      --modules <ids>    Show these modules, in this order, e.g. os,load,git,ip
      --swatches <kind>  Color swatches: palette (default), ansi, none
      --no-bars          Hide the usage bars of memory, disk and battery
      --no-color         Disable colors (NO_COLOR is honored too)
      --refresh          Recompute the facts cached until the next boot
  -h, --help             Show this help
  -V, --version          Show the version
";

struct Args {
    logo: Option<Style>,
    size: Option<usize>,
    no_color: bool,
    image: Option<PathBuf>,
    keep_background: bool,
    layout: Layout,
    /// `--modules`: the module list, replacing the default one.
    modules: Option<String>,
    swatches: Swatches,
    bars: bool,
    /// Recompute the facts cached per boot.
    refresh: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        logo: Some(Style::Ascii),
        size: None,
        no_color: false,
        image: None,
        keep_background: false,
        layout: Layout::Auto,
        modules: None,
        swatches: Swatches::Palette,
        bars: true,
        refresh: false,
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
                print!("{HELP}{}", module_help());
                exit(0);
            }
            "-V" | "--version" => {
                println!("ffetch {}", env!("CARGO_PKG_VERSION"));
                exit(0);
            }
            "--no-color" => args.no_color = true,
            "--no-bars" => args.bars = false,
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
            "--refresh" => args.refresh = true,
            "--layout" => {
                args.layout = match value().as_deref() {
                    Some("auto") => Layout::Auto,
                    Some("side") => Layout::Side,
                    Some("stacked") => Layout::Stacked,
                    v => return Err(expected("--layout", "auto, side or stacked", v)),
                }
            }
            "--modules" => {
                args.modules = Some(value().ok_or("--modules expects a list of module ids")?)
            }
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

/// The module ids for `--help`: the default ones in display order, then the
/// others.
fn module_help() -> String {
    let ids = |default_on: bool| {
        let ids: Vec<&str> = info::MODULES
            .iter()
            .filter(|m| m.default_on == default_on)
            .map(|m| m.id)
            .collect();
        wrap(&ids)
    };
    format!(
        "\nModules (for --modules), shown by default:\n{}Off by default:\n{}",
        ids(true),
        ids(false)
    )
}

/// `words` separated by commas, in indented lines of at most 80 columns.
fn wrap(words: &[&str]) -> String {
    let mut lines = vec![String::new()];
    for (i, word) in words.iter().enumerate() {
        let word = match i + 1 < words.len() {
            true => format!("{word},"),
            false => word.to_string(),
        };
        let line = lines.last_mut().unwrap();
        if !line.is_empty() && 2 + line.len() + 1 + word.len() > 80 {
            lines.push(String::new());
        }
        let line = lines.last_mut().unwrap();
        if !line.is_empty() {
            line.push(' ');
        }
        *line += &word;
    }
    lines.iter().map(|l| format!("  {l}\n")).collect()
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
    let ids = match &args.modules {
        Some(list) => module_list(list),
        None => info::default_modules(),
    };
    let image = load_image(&args);
    let palette = palette::extract(&image.pixels, image.background);
    let opts = Options {
        logo: args.logo,
        size: args.size,
        layout: args.layout,
        bars: args.bars,
        mode,
        roles: Theme::default().roles(&palette),
        swatches: layout::swatch_rows(args.swatches, &palette, mode),
    };
    let ctx = info::Ctx::live(args.refresh);
    let sys = info::collect(&ctx, &ids);
    let out = layout::render(&sys, &image, &opts, term::size());
    // Ignore errors such as a closed pipe (`ffetch | head`).
    let _ = io::stdout().lock().write_all(out.as_bytes());
}

/// The modules named in `--modules`, with a warning for each name that isn't
/// one. They are skipped, so a typo never breaks a shell's startup.
fn module_list(list: &str) -> Vec<&'static str> {
    let (ids, unknown) = info::parse_list(list);
    for name in unknown {
        eprintln!("ffetch: unknown module '{name}'; skipping it");
    }
    ids
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

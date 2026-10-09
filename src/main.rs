mod cache;
mod command;
mod config;
mod image;
mod info;
mod layout;
mod logo;
mod output;
mod palette;
mod quip;
mod rng;
mod socket;
mod term;
mod wsl;

use std::{
    io::{self, Write},
    path::PathBuf,
    process::exit,
};

use config::{Config, Flags};
use layout::{MIN_LOGO_COLS, Options};
use logo::LogoImage;
use output::format::{self, Template};
use term::ColorMode;

const HELP: &str = "\
ffetch - a neofetch-style system info tool

Usage: ffetch [options]

Options:
  -l, --logo <style>     Logo style: blocks (default), ascii, none
  -s, --size <cols>      Logo width in columns (default 48; shrinks to fit the terminal)
      --image <path>     Use a PNG image as the logo and take the colors from it
      --keep-background  With --image, keep the image's background instead of removing it
      --layout <kind>    Logo placement: auto (default; beside the info if it fits,
                         else above it), side, stacked
      --modules <ids>    Show these modules, in this order, e.g. os,load,git,ip
      --swatches <kind>  Color swatches: palette (default), ansi, none
      --no-bars          Hide the usage bars of memory, disk and battery
      --no-quip          Hide the character's remark under the info
      --no-color         Disable colors (NO_COLOR is honored too)
      --refresh          Recompute the facts cached until the next boot
      --json             Print the info and the palette as JSON
      --format <text>    Print one line of text: {module} is a module's value,
                         {module.field} one of its fields (e.g. {memory.pct}),
                         and {{ and }} are braces
      --oneline          OS, uptime, memory and disk on one line (a --format preset)
      --config <path>    Read this config file instead of
                         ${XDG_CONFIG_HOME:-~/.config}/ffetch/config.toml
      --print-config     Print the settings in effect as a config file
  -h, --help             Show this help
  -V, --version          Show the version

Options override the config file.
";

/// What to print.
enum Output {
    /// The logo beside (or above) the info.
    Text,
    Json,
    /// One line from a template: `--format` or `--oneline`.
    Line(Template),
}

struct Args {
    /// What the options say about the settings in the config file.
    flags: Flags,
    /// `--modules`, as given.
    modules: Option<String>,
    no_color: bool,
    /// Recompute the facts cached per boot.
    refresh: bool,
    /// `--config`.
    config: Option<PathBuf>,
    print_config: bool,
    output: Output,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        flags: Flags::default(),
        modules: None,
        no_color: false,
        refresh: false,
        config: None,
        print_config: false,
        output: Output::Text,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        // Accept both "--opt value" and "--opt=value".
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg, None),
        };
        let mut value = || inline.clone().or_else(|| argv.next());
        let mut output = |output: Output| {
            if !matches!(args.output, Output::Text) {
                return Err("--json, --format and --oneline don't go together");
            }
            args.output = output;
            Ok(())
        };
        let flags = &mut args.flags;
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
            "--no-bars" => flags.bars = Some(false),
            "--no-quip" => flags.quip = Some(false),
            "-l" | "--logo" => flags.style = Some(choose("--logo", config::STYLES, value())?),
            "--image" => flags.image = Some(value().ok_or("--image expects a path")?.into()),
            "--keep-background" => flags.keep_background = Some(true),
            "--refresh" => args.refresh = true,
            "--layout" => flags.layout = Some(choose("--layout", config::LAYOUTS, value())?),
            "--modules" => {
                args.modules = Some(value().ok_or("--modules expects a list of module ids")?)
            }
            "--swatches" => flags.swatches = Some(choose("--swatches", config::SWATCHES, value())?),
            "-s" | "--size" => {
                let cols = value()
                    .and_then(|v| v.parse().ok())
                    .filter(|&n| n >= MIN_LOGO_COLS);
                flags.size =
                    Some(cols.ok_or(format!("--size expects a number >= {MIN_LOGO_COLS}"))?);
            }
            "--json" => output(Output::Json)?,
            "--format" => {
                let template = value().ok_or("--format expects a template")?;
                let template = template.parse().map_err(|e| format!("--format: {e}"))?;
                output(Output::Line(template))?
            }
            "--oneline" => output(Output::Line(
                format::ONELINE.parse().expect("the preset parses"),
            ))?,
            "--config" => args.config = Some(value().ok_or("--config expects a path")?.into()),
            "--print-config" => args.print_config = true,
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(args)
}

/// The value of the word `got` in `table`, for the option `flag`.
fn choose<T: Copy>(flag: &str, table: &[(&str, T)], got: Option<String>) -> Result<T, String> {
    got.as_deref()
        .and_then(|word| config::lookup(table, word))
        .ok_or_else(|| {
            let got = got.map_or("nothing".into(), |v| format!("'{v}'"));
            format!("{flag} expects {}, got {got}", config::choices(table, ""))
        })
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

fn main() {
    let args = parse_args().unwrap_or_else(|e| {
        eprintln!("ffetch: {e}\nTry 'ffetch --help'.");
        exit(2);
    });
    let out = match &args.output {
        // Only the template's modules run: no config file, logo or palette.
        Output::Line(template) if !args.print_config => {
            template.line(&info::Ctx::live(args.refresh)) + "\n"
        }
        _ => {
            let config = settings(&args);
            match (&args.output, args.print_config) {
                (_, true) => config.to_toml(),
                (Output::Json, _) => json(&args, &config),
                _ => text(&args, &config),
            }
        }
    };
    // Ignore errors such as a closed pipe (`ffetch | head`).
    let _ = io::stdout().lock().write_all(out.as_bytes());
}

/// The settings: the config file's, with the options over them. Problems with
/// the file are warnings.
fn settings(args: &Args) -> Config {
    let (mut config, warnings) = config::load(args.config.as_deref());
    for warning in warnings {
        eprintln!("ffetch: {warning}");
    }
    let mut flags = args.flags.clone();
    flags.modules = args.modules.as_deref().map(module_list);
    config.apply(flags);
    config
}

/// The logo, the info and the palette, the usual way.
fn text(args: &Args, config: &Config) -> String {
    let mode = ColorMode::detect(args.no_color);
    let image = load_image(&config.logo);
    let palette = palette::extract(&image.pixels, image.background);
    let ctx = info::Ctx::live(args.refresh);
    let sys = info::collect(&ctx, &config.modules);
    let quip = match config.quip {
        true => {
            let state = quip::State::new(&sys, &ctx);
            quip::pick(&config.rules(), &state, quip::clock_seed())
        }
        false => None,
    };
    let opts = Options {
        logo: config.logo.style,
        size: config.logo.size,
        layout: config.layout,
        bars: config.bars,
        mode,
        roles: config.theme.roles(&palette),
        swatches: layout::swatch_rows(config.swatches, &palette, mode),
        quip,
    };
    layout::render(&sys, &image, &opts, term::size())
}

/// `--json`: the info and the palette, which still comes from the logo image.
fn json(args: &Args, config: &Config) -> String {
    let image = load_image(&config.logo);
    let palette = palette::extract(&image.pixels, image.background);
    let ctx = info::Ctx::live(args.refresh);
    let sys = info::collect(&ctx, &config.modules);
    output::json::render(&sys, &palette, config.theme.roles(&palette))
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

/// The logo image: `logo.image` if it loads, otherwise the embedded one. A
/// broken image costs one warning, never the run.
fn load_image(logo: &config::Logo) -> LogoImage {
    let Some(path) = &logo.image else {
        return LogoImage::embedded();
    };
    match cache::image(path, logo.keep_background, cache::dir().as_deref()) {
        Ok(img) => img.into(),
        Err(e) => {
            eprintln!("ffetch: can't load image {path:?}: {e}; using the built-in logo");
            LogoImage::embedded()
        }
    }
}

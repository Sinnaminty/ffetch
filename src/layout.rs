//! Arranging the output: the logo beside the info column, above it, or left
//! out to fit the terminal; and drawing the info column itself.

use crate::{
    info::{Info, Level, Meter, Row},
    logo::{LogoImage, Style},
    palette::{Palette, Roles},
    term::{ColorMode, RESET, Rgb},
};

/// Spaces between the logo and the info column.
const GAP: usize = 3;
const DEFAULT_LOGO_COLS: usize = 48;
pub const MIN_LOGO_COLS: usize = 16;
/// The info column is truncated down to this before the logo is dropped entirely.
const MIN_INFO_COLS: usize = 32;
/// Width of one colour swatch at the bottom of the info column.
const SWATCH_COLS: usize = 3;
/// Cells in a usage bar.
const BAR_CELLS: usize = 10;

/// Where the logo should go (`--layout`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Beside the info if it fits, otherwise above it, otherwise nowhere.
    Auto,
    /// Beside the info, or nowhere.
    Side,
    /// Above the info, or nowhere.
    Stacked,
}

/// Where the logo went, and its width in columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Side(usize),
    Stacked(usize),
    NoLogo,
}

/// What the swatch rows at the bottom of the info column show.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Swatches {
    /// One row with the palette taken from the logo.
    Palette,
    /// Two rows with the 16 ANSI colors.
    Ansi,
    None,
}

/// How the output is drawn.
pub struct Options {
    /// `None` for no logo.
    pub logo: Option<Style>,
    /// The logo width asked for with `--size`.
    pub size: Option<usize>,
    pub layout: Layout,
    /// Usage bars for memory, disk and battery.
    pub bars: bool,
    pub mode: ColorMode,
    pub roles: Roles,
    /// Background colour codes for each swatch row, from `swatch_rows`.
    pub swatches: Vec<Vec<String>>,
}

/// The whole output for a terminal `term` (columns, rows) wide and high, or
/// for a pipe (`None`).
pub fn render(
    info: &Info,
    image: &LogoImage,
    opts: &Options,
    term: Option<(usize, usize)>,
) -> String {
    let rows = info.rows();
    let natural = natural_width(info, &rows, opts);
    let height = info_lines(info, &rows, opts, None).len();
    let placement = match opts.logo {
        Some(_) => place(opts.layout, image, opts.size, (natural, height), term),
        None => Placement::NoLogo,
    };
    let max_info_cols = term.map(|(width, _)| match placement {
        Placement::Side(cols) => width.saturating_sub(cols + GAP),
        _ => width,
    });
    let column = info_lines(info, &rows, opts, max_info_cols);
    let logo = match (opts.logo, placement) {
        (Some(style), Placement::Side(cols) | Placement::Stacked(cols)) => {
            image.render(style, cols, opts.mode)
        }
        _ => Vec::new(),
    };
    compose(placement, &logo, &column)
}

/// Chooses where the logo goes and how wide it is, for an info column `info`
/// (columns, rows) big, on a terminal `term` (columns, rows) or a pipe.
pub fn place(
    layout: Layout,
    image: &LogoImage,
    requested: Option<usize>,
    (info_cols, info_rows): (usize, usize),
    term: Option<(usize, usize)>,
) -> Placement {
    let side = || fit_side(image, requested, info_cols, term).map(Placement::Side);
    let stacked = || fit_stacked(image, requested, info_rows, term).map(Placement::Stacked);
    let placement = match layout {
        Layout::Auto => side().or_else(stacked),
        Layout::Side => side(),
        Layout::Stacked => stacked(),
    };
    placement.unwrap_or(Placement::NoLogo)
}

/// The logo width beside the info column: the requested (or default) size,
/// shrunk so the logo and the info column fit the terminal. `None` means
/// there is no room for it.
fn fit_side(
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

/// The logo width above an info column of `info_rows` lines: the requested
/// (or default) size, no wider than the terminal and, unless requested, short
/// enough for the logo and the info to fit its height. `None` means there is
/// no room for it.
fn fit_stacked(
    image: &LogoImage,
    requested: Option<usize>,
    info_rows: usize,
    term: Option<(usize, usize)>,
) -> Option<usize> {
    let mut cols = requested.unwrap_or(DEFAULT_LOGO_COLS);
    if let Some((width, height)) = term {
        cols = cols.min(width);
        if requested.is_none() {
            // A blank row under the logo, one for the trailing blank line and
            // one for the prompt.
            cols = cols.min(image.cols_for(height.saturating_sub(info_rows + 3)));
        }
    }
    (cols >= MIN_LOGO_COLS).then_some(cols)
}

/// Puts the logo and the info column together as `placement` says.
fn compose(placement: Placement, logo: &[String], column: &[String]) -> String {
    let mut out = String::new();
    match placement {
        Placement::Side(cols) => {
            for i in 0..logo.len().max(column.len()) {
                let line = column.get(i).map_or("", String::as_str);
                match logo.get(i) {
                    Some(l) => out += l,
                    None if !line.is_empty() => out += &" ".repeat(cols),
                    None => {}
                }
                if !line.is_empty() {
                    out += &" ".repeat(GAP);
                }
                out += line;
                out.push('\n');
            }
        }
        Placement::Stacked(_) | Placement::NoLogo => {
            let gap = (!logo.is_empty()).then(String::new);
            for line in logo.iter().chain(gap.as_ref()).chain(column) {
                out += line;
                out.push('\n');
            }
        }
    }
    out.push('\n');
    out
}

/// How wide the info column is without truncation.
fn natural_width(info: &Info, rows: &[Row], opts: &Options) -> usize {
    let title = info.user.chars().count() + 1 + info.host.chars().count();
    let rows = rows
        .iter()
        .map(|r| r.label.len() + 2 + bar_cols(r, opts) + r.text.chars().count());
    let swatches = opts.swatches.iter().map(|row| row.len() * SWATCH_COLS);
    rows.chain(swatches).chain([title]).max().unwrap_or(0)
}

/// Background colour codes for each swatch row. There are none without colour.
pub fn swatch_rows(kind: Swatches, palette: &Palette, mode: ColorMode) -> Vec<Vec<String>> {
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

/// The info column: the title, then a "Label: value" line per row (with a
/// usage bar where there is one), then the swatch rows. Lines are truncated
/// to `max_cols`.
pub fn info_lines(
    info: &Info,
    rows: &[Row],
    opts: &Options,
    max_cols: Option<usize>,
) -> Vec<String> {
    let (mode, roles) = (opts.mode, opts.roles);
    let title_len = info.user.chars().count() + 1 + info.host.chars().count();
    let mut lines = vec![
        format!(
            "{}{}{}",
            mode.paint(&info.user, roles.accent),
            mode.tint("@", roles.muted),
            mode.paint(&info.host, roles.secondary)
        ),
        mode.tint(&"-".repeat(title_len), roles.muted),
    ];

    for row in rows {
        let bar = match row.meter.filter(|_| opts.bars) {
            Some(meter) => bar(meter, mode, roles.muted) + " ",
            None => String::new(),
        };
        let room = max_cols.map(|m| m.saturating_sub(row.label.len() + 2 + bar_cols(row, opts)));
        lines.push(format!(
            "{}: {bar}{}",
            mode.paint(row.label, roles.accent),
            truncate(&row.text, room)
        ));
    }

    if !opts.swatches.is_empty() {
        lines.push(String::new());
    }
    let block = " ".repeat(SWATCH_COLS);
    for row in &opts.swatches {
        let row: String = row.iter().map(|code| format!("{code}{block}")).collect();
        lines.push(row + RESET);
    }
    lines
}

/// Columns taken by the bar of `row` and the space after it.
fn bar_cols(row: &Row, opts: &Options) -> usize {
    match (row.meter, opts.bars, opts.mode) {
        (Some(_), true, ColorMode::None) => BAR_CELLS + 3,
        (Some(_), true, _) => BAR_CELLS + 1,
        _ => 0,
    }
}

/// A 10-cell usage bar, filled in green, yellow or red by level, with the
/// empty cells in `muted`. A cell is filled once any of its tenth is used.
/// Without colour: "[##--------]".
fn bar(meter: Meter, mode: ColorMode, muted: Rgb) -> String {
    let filled = (meter.pct.min(100) as usize).div_ceil(100 / BAR_CELLS);
    let empty = BAR_CELLS - filled;
    if mode == ColorMode::None {
        return format!("[{}{}]", "#".repeat(filled), "-".repeat(empty));
    }
    // Plain ANSI colours, so they follow the terminal's theme.
    let sgr = match meter.level() {
        Level::Ok => 32,
        Level::Warn => 33,
        Level::Crit => 31,
    };
    let mut out = String::new();
    if filled > 0 {
        out += &format!("\x1b[{sgr}m{}{RESET}", "█".repeat(filled));
    }
    if empty > 0 {
        out += &mode.tint(&"░".repeat(empty), muted);
    }
    out
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

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;
    use crate::info::Gauge;

    /// A square logo, like the built-in one: n columns are n/2 rows.
    fn square() -> LogoImage {
        LogoImage {
            width: 256,
            height: 256,
            pixels: Cow::Borrowed(&[]),
            background: None,
        }
    }

    /// The info column of a default run on the dev machine (WSL2): 52
    /// columns (the Disk line with its bar) and 20 rows (title and
    /// separator, 16 modules, a blank line and the palette swatches).
    const INFO: (usize, usize) = (52, 20);

    #[test]
    fn auto_layout_by_terminal_size() {
        use Placement::*;
        for (term, expected) in [
            // Side by side at full size...
            ((200, 50), Side(48)),
            ((120, 30), Side(48)),
            // ...shrunk to keep the info whole (22 rows would allow 44)...
            ((80, 24), Side(25)),
            // ...down to 16 columns, with the info squeezed to 41.
            ((60, 60), Side(16)),
            // Too narrow for both: above the info, as tall as the height
            // leaves room for (40 - 20 - 3 = 17 rows)...
            ((50, 40), Stacked(34)),
            // ...or as wide as the terminal.
            ((40, 50), Stacked(40)),
            // 31 - 20 - 3 = 8 rows leave room for the smallest logo, 30 don't.
            ((50, 31), Stacked(16)),
            ((50, 30), NoLogo),
            // Too small for the logo anywhere.
            ((40, 20), NoLogo),
            ((30, 10), NoLogo),
        ] {
            let got = place(Layout::Auto, &square(), None, INFO, Some(term));
            assert_eq!(got, expected, "{}x{}", term.0, term.1);
        }
    }

    #[test]
    fn forced_layouts_requested_sizes_and_pipes() {
        use Placement::*;
        let place = |layout, size, term| place(layout, &square(), size, INFO, term);
        assert_eq!(place(Layout::Side, None, Some((50, 40))), NoLogo);
        assert_eq!(place(Layout::Side, None, Some((120, 30))), Side(48));
        assert_eq!(place(Layout::Stacked, None, Some((200, 50))), Stacked(48));
        assert_eq!(place(Layout::Stacked, None, Some((80, 24))), NoLogo);
        // A requested size is kept, if the width allows, whatever the height.
        assert_eq!(place(Layout::Auto, Some(30), Some((200, 50))), Side(30));
        assert_eq!(place(Layout::Auto, Some(30), Some((50, 40))), Stacked(30));
        assert_eq!(
            place(Layout::Stacked, Some(64), Some((50, 10))),
            Stacked(50)
        );
        // Not a terminal: the requested or default size.
        assert_eq!(place(Layout::Auto, None, None), Side(48));
        assert_eq!(place(Layout::Stacked, Some(20), None), Stacked(20));
    }

    #[test]
    fn stacked_output_puts_the_logo_above() {
        let logo = ["AAAA".to_string(), "BBBB".to_string()];
        let column = ["me@box".to_string(), "------".to_string()];
        assert_eq!(
            compose(Placement::Stacked(4), &logo, &column),
            "AAAA\nBBBB\n\nme@box\n------\n\n"
        );
        assert_eq!(
            compose(Placement::Side(4), &logo, &column[..1]),
            "AAAA   me@box\nBBBB\n\n"
        );
        assert_eq!(
            compose(Placement::NoLogo, &[], &column),
            "me@box\n------\n\n"
        );
    }

    const MUTED: Rgb = Rgb(1, 2, 3);

    fn colored(meter: Meter) -> String {
        bar(meter, ColorMode::TrueColor, MUTED)
    }

    /// The bar as "<sgr>:<filled cells>" (no colour code when nothing is
    /// filled), checking the empty cells' colour on the way.
    fn summary(bar: &str) -> String {
        let (filled, empty) = match bar.split_once("\x1b[38;2;1;2;3m") {
            Some((filled, empty)) => (filled, empty.strip_suffix(RESET).unwrap()),
            None => (bar, ""),
        };
        assert!(empty.chars().all(|c| c == '░'), "{bar:?}");
        let cells = filled.chars().filter(|&c| c == '█').count();
        assert_eq!(cells + empty.chars().count(), BAR_CELLS, "{bar:?}");
        match filled.strip_prefix("\x1b[") {
            Some(rest) => format!("{}:{cells}", &rest[..2]),
            None => format!("-:{cells}"),
        }
    }

    #[test]
    fn bars_by_level() {
        let used = |pct| {
            summary(&colored(Meter {
                gauge: Gauge::Used,
                pct,
            }))
        };
        let charge = |pct| {
            summary(&colored(Meter {
                gauge: Gauge::Charge,
                pct,
            }))
        };
        let plain = |pct| {
            bar(
                Meter {
                    gauge: Gauge::Used,
                    pct,
                },
                ColorMode::None,
                MUTED,
            )
        };
        for (pct, used_bar, charge_bar, plain_bar) in [
            (0, "-:0", "-:0", "[----------]"),
            (1, "32:1", "31:1", "[#---------]"),
            (14, "32:2", "31:2", "[##--------]"),
            (15, "32:2", "33:2", "[##--------]"),
            (40, "32:4", "33:4", "[####------]"),
            (41, "32:5", "32:5", "[#####-----]"),
            (59, "32:6", "32:6", "[######----]"),
            (60, "33:6", "32:6", "[######----]"),
            (85, "33:9", "32:9", "[#########-]"),
            (86, "31:9", "32:9", "[#########-]"),
            (100, "31:10", "32:10", "[##########]"),
            (120, "31:10", "32:10", "[##########]"),
        ] {
            assert_eq!(used(pct), used_bar, "{pct}% used");
            assert_eq!(charge(pct), charge_bar, "{pct}% charged");
            assert_eq!(plain(pct), plain_bar, "{pct}%, no colour");
        }
        assert_eq!(
            colored(Meter {
                gauge: Gauge::Used,
                pct: 12
            }),
            "\x1b[32m██\x1b[0m\x1b[38;2;1;2;3m░░░░░░░░\x1b[0m"
        );
        assert_eq!(
            colored(Meter {
                gauge: Gauge::Used,
                pct: 100
            }),
            "\x1b[31m██████████\x1b[0m",
            "nothing empty to tint"
        );
        let ansi256 = bar(
            Meter {
                gauge: Gauge::Charge,
                pct: 10,
            },
            ColorMode::Ansi256,
            MUTED,
        );
        assert!(
            ansi256.starts_with("\x1b[31m█\x1b[0m\x1b[38;5;"),
            "{ansi256:?}"
        );
    }

    fn options(mode: ColorMode, bars: bool) -> Options {
        let gray = Rgb(128, 128, 128);
        Options {
            logo: None,
            size: None,
            layout: Layout::Auto,
            bars,
            mode,
            roles: Roles {
                accent: gray,
                secondary: gray,
                muted: gray,
            },
            swatches: Vec::new(),
        }
    }

    fn info() -> Info {
        Info {
            user: "me".into(),
            host: "box".into(),
            modules: Vec::new(),
        }
    }

    fn rows() -> Vec<Row> {
        let row = |label, text: &str, meter| Row {
            label,
            text: text.into(),
            meter,
        };
        let meter = |gauge, pct| Some(Meter { gauge, pct });
        vec![
            row("Kernel", "6.6.87", None),
            row(
                "Memory",
                "2.00 GiB / 16.00 GiB (12%)",
                meter(Gauge::Used, 12),
            ),
            row("Battery", "80% [Charging]", meter(Gauge::Charge, 80)),
            row("Battery", "9% [Discharging]", meter(Gauge::Charge, 9)),
        ]
    }

    #[test]
    fn info_column_with_and_without_bars() {
        let plain = info_lines(&info(), &rows(), &options(ColorMode::None, true), None);
        assert_eq!(
            plain,
            [
                "me@box",
                "------",
                "Kernel: 6.6.87",
                "Memory: [##--------] 2.00 GiB / 16.00 GiB (12%)",
                "Battery: [########--] 80% [Charging]",
                "Battery: [#---------] 9% [Discharging]",
            ]
        );
        let no_bars = info_lines(&info(), &rows(), &options(ColorMode::None, false), None);
        assert_eq!(no_bars[3], "Memory: 2.00 GiB / 16.00 GiB (12%)");
        assert_eq!(no_bars[5], "Battery: 9% [Discharging]");

        let opts = options(ColorMode::TrueColor, true);
        let lines = info_lines(&info(), &rows(), &opts, None);
        let label = |l: &str| ColorMode::TrueColor.paint(l, Rgb(128, 128, 128));
        let muted = "\x1b[38;2;128;128;128m";
        assert_eq!(
            lines[5],
            format!(
                "{}: \x1b[31m█\x1b[0m{muted}░░░░░░░░░\x1b[0m 9% [Discharging]",
                label("Battery")
            )
        );
    }

    #[test]
    fn bars_count_toward_the_width_and_truncation() {
        for (mode, bar) in [(ColorMode::None, 13), (ColorMode::Ansi256, 11)] {
            let opts = options(mode, true);
            assert_eq!(
                natural_width(&info(), &rows(), &opts),
                "Memory: ".len() + bar + "2.00 GiB / 16.00 GiB (12%)".len()
            );
            // The bar stays whole; the text makes room.
            let lines = info_lines(&info(), &rows(), &opts, Some(8 + bar + 9));
            assert!(lines[3].ends_with("] 2.00 GiB…") || lines[3].ends_with("m 2.00 GiB…"));
            assert!(lines[2].ends_with(": 6.6.87"), "short lines stay whole");
        }
        let opts = options(ColorMode::None, false);
        assert_eq!(natural_width(&info(), &rows(), &opts), 34);
    }
}

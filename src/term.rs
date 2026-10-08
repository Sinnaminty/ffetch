//! Terminal capabilities: colour support, escape sequences and window size.

use std::env;

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    None,
    Ansi256,
    TrueColor,
}

impl ColorMode {
    pub fn detect(disabled: bool) -> Self {
        if disabled || env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
            return Self::None;
        }
        let truecolor_env = env::var("COLORTERM").is_ok_and(|v| v == "truecolor" || v == "24bit");
        let truecolor_term = env::var("TERM")
            .is_ok_and(|t| t.contains("truecolor") || t.contains("24bit") || t.contains("direct"));
        let truecolor_app = env::var_os("WT_SESSION").is_some()
            || env::var("TERM_PROGRAM").is_ok_and(|p| {
                matches!(
                    p.as_str(),
                    "iTerm.app" | "WezTerm" | "vscode" | "ghostty" | "Hyper"
                )
            });
        if truecolor_env || truecolor_term || truecolor_app {
            Self::TrueColor
        } else {
            Self::Ansi256
        }
    }

    pub fn fg(self, c: Rgb) -> String {
        match self {
            Self::None => String::new(),
            Self::Ansi256 => format!("\x1b[38;5;{}m", to_ansi256(c)),
            Self::TrueColor => format!("\x1b[38;2;{};{};{}m", c.0, c.1, c.2),
        }
    }

    pub fn bg(self, c: Rgb) -> String {
        match self {
            Self::None => String::new(),
            Self::Ansi256 => format!("\x1b[48;5;{}m", to_ansi256(c)),
            Self::TrueColor => format!("\x1b[48;2;{};{};{}m", c.0, c.1, c.2),
        }
    }

    /// Wraps `s` in bold + `c`, or returns it unchanged when colour is off.
    pub fn paint(self, s: &str, c: Rgb) -> String {
        match self {
            Self::None => s.to_string(),
            _ => format!("{BOLD}{}{s}{RESET}", self.fg(c)),
        }
    }

    /// Like `paint`, but without bold.
    pub fn tint(self, s: &str, c: Rgb) -> String {
        match self {
            Self::None => s.to_string(),
            _ => format!("{}{s}{RESET}", self.fg(c)),
        }
    }
}

/// Nearest colour in the xterm 256-colour palette (6x6x6 cube or grey ramp).
fn to_ansi256(Rgb(r, g, b): Rgb) -> u8 {
    const LEVELS: [i32; 6] = [0, 95, 135, 175, 215, 255];
    let nearest_level = |v: u8| {
        (0..6)
            .min_by_key(|&i| (LEVELS[i] - v as i32).abs())
            .unwrap()
    };
    let (ri, gi, bi) = (nearest_level(r), nearest_level(g), nearest_level(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);

    let avg = (r as i32 + g as i32 + b as i32) / 3;
    let grey_idx = ((avg - 8).max(0) / 10).min(23);
    let grey = 8 + grey_idx * 10;

    let dist = |(cr, cg, cb): (i32, i32, i32)| {
        (cr - r as i32).pow(2) + (cg - g as i32).pow(2) + (cb - b as i32).pow(2)
    };
    if dist((grey, grey, grey)) < dist(cube) {
        232 + grey_idx as u8
    } else {
        16 + (36 * ri + 6 * gi + bi) as u8
    }
}

/// Terminal size as (columns, rows), if stdout is a terminal.
pub fn size() -> Option<(usize, usize)> {
    // SAFETY: TIOCGWINSZ only writes into the provided winsize struct.
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0;
    (ok && ws.ws_col > 0 && ws.ws_row > 0).then_some((ws.ws_col as usize, ws.ws_row as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_ansi256_known_colors() {
        assert_eq!(to_ansi256(Rgb(0, 0, 0)), 16);
        assert_eq!(to_ansi256(Rgb(255, 255, 255)), 231);
        assert_eq!(to_ansi256(Rgb(255, 0, 0)), 196);
        assert_eq!(to_ansi256(Rgb(95, 135, 175)), 67);
        // Greys go to the grey ramp rather than the coarser cube.
        assert_eq!(to_ansi256(Rgb(128, 128, 128)), 244);
        assert_eq!(to_ansi256(Rgb(0xc3, 0x3e, 0x58)), 131);
    }

    #[test]
    fn color_codes_per_mode() {
        let c = Rgb(195, 62, 88);
        assert_eq!(ColorMode::TrueColor.fg(c), "\x1b[38;2;195;62;88m");
        assert_eq!(ColorMode::Ansi256.bg(c), "\x1b[48;5;131m");
        assert_eq!(ColorMode::None.paint("x", c), "x");
        assert_eq!(ColorMode::None.tint("x", c), "x");
        assert_eq!(ColorMode::Ansi256.tint("x", c), "\x1b[38;5;131mx\x1b[0m");
    }
}

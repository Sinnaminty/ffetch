//! Modules about the graphical session and the terminal: resolution, desktop
//! environment, window manager and terminal emulator.

use super::{
    Ctx,
    value::{Name, Resolution, Value},
};

pub fn resolution(ctx: &Ctx) -> Option<Value> {
    let modes: Vec<String> = ctx
        .sorted_dir("/sys/class/drm")
        .into_iter()
        .filter(|p| ctx.read(p.join("status")).as_deref() == Some("connected"))
        .filter_map(|p| Some(ctx.read(p.join("modes"))?.lines().next()?.to_string()))
        .collect();
    (!modes.is_empty()).then_some(Value::Resolution(Resolution { modes }))
}

pub fn de(ctx: &Ctx) -> Option<Value> {
    let de = ctx
        .env_nonempty("XDG_CURRENT_DESKTOP")
        .or_else(|| ctx.env_nonempty("DESKTOP_SESSION"))?;
    // e.g. "ubuntu:GNOME" -> "GNOME"
    Some(Value::De(Name::new(de.rsplit(':').next().unwrap_or(&de))))
}

/// Process name (as in /proc/<pid>/comm) -> window manager name.
const WMS: &[(&str, &str)] = &[
    ("Hyprland", "Hyprland"),
    ("awesome", "awesome"),
    ("bspwm", "bspwm"),
    ("budgie-wm", "Budgie WM"),
    ("cinnamon", "Muffin"),
    ("cosmic-comp", "COSMIC"),
    ("dwm", "dwm"),
    ("enlightenment", "Enlightenment"),
    ("fluxbox", "Fluxbox"),
    ("fvwm", "FVWM"),
    ("gnome-shell", "Mutter"),
    ("herbstluftwm", "herbstluftwm"),
    ("i3", "i3"),
    ("icewm", "IceWM"),
    ("kwin_wayland", "KWin"),
    ("kwin_x11", "KWin"),
    ("labwc", "labwc"),
    ("leftwm", "LeftWM"),
    ("marco", "Marco"),
    ("mutter", "Mutter"),
    ("niri", "niri"),
    ("openbox", "Openbox"),
    ("qtile", "Qtile"),
    ("river", "river"),
    ("spectrwm", "spectrwm"),
    ("sway", "Sway"),
    ("wayfire", "Wayfire"),
    ("weston", "Weston"),
    ("xfwm4", "Xfwm4"),
    ("xmonad", "xmonad"),
];

pub fn wm(ctx: &Ctx) -> Option<Value> {
    if ctx.env("DISPLAY").is_none() && ctx.env("WAYLAND_DISPLAY").is_none() {
        return None;
    }
    let name = ctx.sorted_dir("/proc").into_iter().find_map(|p| {
        let comm = ctx.read(p.join("comm"))?;
        // xmonad's binary is named after the platform, e.g. "xmonad-x86_64-linux".
        let comm = if comm.starts_with("xmonad") {
            "xmonad"
        } else {
            &comm
        };
        WMS.iter()
            .find(|(c, _)| *c == comm)
            .map(|(_, name)| name.to_string())
    })?;
    Some(Value::Wm(Name::new(name)))
}

/// Parent processes skipped while looking for the terminal emulator.
const NOT_TERMINALS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "mksh",
    "tcsh",
    "csh",
    "nu",
    "xonsh",
    "elvish",
    "sudo",
    "doas",
    "su",
    "login",
    "env",
    "time",
    "script",
    "nix-shell",
    "direnv",
    "watch",
    "ffetch",
    "cargo",
    "init",
    "systemd",
    "SessionLeader",
];

pub fn terminal(ctx: &Ctx) -> Option<Value> {
    terminal_name(ctx).map(|name| Value::Terminal(Name::new(name)))
}

fn terminal_name(ctx: &Ctx) -> Option<String> {
    if let Some(name) = env_terminal(|key| ctx.env(key)) {
        return Some(name);
    }
    let mut pid = ctx.ppid();
    while pid > 1 {
        let Some(stat) = ctx.read(format!("/proc/{pid}/stat")) else {
            break;
        };
        // "<pid> (<comm>) <state> <ppid> ..."; comm itself may contain spaces or parens.
        let Some((comm, rest)) = stat.split_once(" (").and_then(|(_, s)| s.rsplit_once(") "))
        else {
            break;
        };
        // WSL's init shows up as "Relay(<pid>)".
        if !NOT_TERMINALS.contains(&comm) && !comm.starts_with("Relay(") {
            return Some(pretty_terminal(comm));
        }
        let Some(ppid) = rest.split_whitespace().nth(1).and_then(|p| p.parse().ok()) else {
            break;
        };
        pid = ppid;
    }
    ctx.env_nonempty("TERM")
}

/// The terminal as named by the environment (`var` looks up a variable), if it is.
fn env_terminal(var: impl Fn(&str) -> Option<String>) -> Option<String> {
    let windows_terminal = var("WT_SESSION").is_some();
    // Windows Terminal's variable reaches a multiplexer started from it.
    if windows_terminal && let Some(mux) = multiplexer(&var) {
        return Some(format!("{mux} (Windows Terminal)"));
    }
    if let Some(program) = var("TERM_PROGRAM").filter(|p| !p.is_empty()) {
        return Some(pretty_terminal(&program));
    }
    windows_terminal.then(|| "Windows Terminal".into())
}

fn multiplexer(var: impl Fn(&str) -> Option<String>) -> Option<&'static str> {
    if var("TMUX").is_some() || var("TERM_PROGRAM").as_deref() == Some("tmux") {
        Some("tmux")
    } else if var("ZELLIJ").is_some() {
        Some("zellij")
    } else if var("STY").is_some() {
        Some("screen")
    } else {
        None
    }
}

fn pretty_terminal(name: &str) -> String {
    match name {
        "gnome-terminal-" | "gnome-terminal-server" => "GNOME Terminal",
        "kgx" => "GNOME Console",
        "konsole" => "Konsole",
        "alacritty" => "Alacritty",
        "foot" | "footclient" => "foot",
        "wezterm-gui" | "WezTerm" => "WezTerm",
        "xfce4-terminal" => "Xfce Terminal",
        "tilix" => "Tilix",
        "terminator" => "Terminator",
        "ghostty" => "Ghostty",
        "Apple_Terminal" => "Apple Terminal",
        "iTerm.app" => "iTerm2",
        "vscode" => "VS Code",
        "WarpTerminal" => "Warp",
        "sshd" => "SSH",
        n if n.starts_with("tmux") => "tmux",
        n => n,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `env_terminal` with only the variables in `vars` set.
    fn terminal_with(vars: &[(&str, &str)]) -> Option<String> {
        env_terminal(|key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        })
    }

    #[test]
    fn windows_terminal_around_a_multiplexer() {
        let wt = ("WT_SESSION", "0a6f0c4e-1c7e-4d1a-9a8e-2f6f0c4e1c7e");
        let tmux = ("TMUX", "/tmp/tmux-1000/default,1266,0");
        let both = Some("tmux (Windows Terminal)");
        assert_eq!(
            terminal_with(&[wt, tmux, ("TERM_PROGRAM", "tmux")]).as_deref(),
            both
        );
        assert_eq!(terminal_with(&[wt, tmux]).as_deref(), both);
        assert_eq!(
            terminal_with(&[wt, ("TERM_PROGRAM", "tmux")]).as_deref(),
            both
        );
        assert_eq!(
            terminal_with(&[wt, ("STY", "1234.pts-0.host")]).as_deref(),
            Some("screen (Windows Terminal)")
        );
        assert_eq!(
            terminal_with(&[wt, ("ZELLIJ", "0")]).as_deref(),
            Some("zellij (Windows Terminal)")
        );
    }

    #[test]
    fn terminal_from_the_environment_is_otherwise_unchanged() {
        let wt = ("WT_SESSION", "0a6f0c4e-1c7e-4d1a-9a8e-2f6f0c4e1c7e");
        let tmux = ("TMUX", "/tmp/tmux-1000/default,1266,0");
        assert_eq!(terminal_with(&[wt]).as_deref(), Some("Windows Terminal"));
        assert_eq!(
            terminal_with(&[wt, ("TERM_PROGRAM", "vscode")]).as_deref(),
            Some("VS Code")
        );
        assert_eq!(
            terminal_with(&[tmux, ("TERM_PROGRAM", "tmux")]).as_deref(),
            Some("tmux")
        );
        // Left to the process tree walk.
        assert_eq!(terminal_with(&[tmux]), None);
        assert_eq!(terminal_with(&[("TERM_PROGRAM", "")]), None);
        assert_eq!(terminal_with(&[]), None);
    }
}

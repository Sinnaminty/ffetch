//! System information, read mostly straight from /proc and /sys (Linux).

use std::{
    env,
    ffi::CStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
    thread,
};

use crate::{cache::Facts, command, wsl};

pub struct System {
    pub user: String,
    pub host: String,
    /// (label, value) pairs in display order.
    pub fields: Vec<(&'static str, String)>,
}

/// A module returns `None` when it has nothing to report. Multi-line values
/// (e.g. several GPUs) are shown as one entry per line under the same label.
type Module = fn() -> Option<String>;

const MODULES: &[(&str, Module)] = &[
    ("OS", os),
    ("Host", host),
    ("Windows", windows),
    ("Kernel", kernel),
    ("Uptime", uptime),
    ("Packages", packages),
    ("Shell", shell),
    ("Resolution", resolution),
    ("DE", de),
    ("WM", wm),
    ("Terminal", terminal),
    ("CPU", cpu),
    ("GPU", gpu),
    ("Memory", memory),
    ("Disk (/)", disk),
    ("Battery", battery),
    ("Locale", locale),
];

/// Facts cached until the next boot. `collect` loads them before the module
/// threads start and saves them after. (A global only until modules get a
/// context to carry it, see SPEC §5.1.)
static FACTS: OnceLock<Facts> = OnceLock::new();

/// The per-boot fact `key`, computed by `compute` when it isn't cached.
fn cached(key: &str, compute: impl FnOnce() -> Option<String>) -> Option<String> {
    match FACTS.get() {
        Some(facts) => facts.get(key, compute),
        None => compute(),
    }
}

/// Runs every module concurrently; a few of them spawn processes or scan big
/// files. With `refresh`, cached facts are looked up again.
pub fn collect(refresh: bool) -> System {
    let facts = FACTS.get_or_init(|| Facts::load(refresh));
    let fields = thread::scope(|s| {
        let handles: Vec<_> = MODULES
            .iter()
            .map(|&(label, f)| (label, s.spawn(f)))
            .collect();
        handles
            .into_iter()
            .filter_map(|(label, h)| Some((label, h.join().ok()??)))
            .flat_map(|(label, value)| {
                value
                    .lines()
                    .map(|l| (label, l.to_string()))
                    .collect::<Vec<_>>()
            })
            .collect()
    });
    // Failing to cache only costs the slow lookups again next run.
    let _ = facts.save();
    System {
        user: user(),
        host: uname().nodename,
        fields,
    }
}

fn read(path: impl AsRef<Path>) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn env_nonempty(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.is_empty())
}

fn sorted_dir(path: impl AsRef<Path>) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(path)
        .map(|d| d.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    paths.sort();
    paths
}

struct Uname {
    nodename: String,
    release: String,
    machine: String,
}

fn uname() -> Uname {
    // SAFETY: uname only fills in the zeroed struct; its fields are NUL-terminated.
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    unsafe { libc::uname(&mut u) };
    let field = |f: &[libc::c_char]| {
        unsafe { CStr::from_ptr(f.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    };
    Uname {
        nodename: field(&u.nodename),
        release: field(&u.release),
        machine: field(&u.machine),
    }
}

fn user() -> String {
    if let Some(user) = env_nonempty("USER") {
        return user;
    }
    // SAFETY: getpwuid returns null or a pointer to a valid passwd entry; it is
    // only called from the main thread.
    let pw = unsafe { libc::getpwuid(libc::getuid()) };
    if pw.is_null() {
        return "user".into();
    }
    unsafe { CStr::from_ptr((*pw).pw_name) }
        .to_string_lossy()
        .into_owned()
}

fn os() -> Option<String> {
    let release = read("/etc/os-release").or_else(|| read("/usr/lib/os-release"));
    let field = |key: &str| {
        release.as_deref()?.lines().find_map(|l| {
            Some(
                l.strip_prefix(key)?
                    .strip_prefix('=')?
                    .trim_matches('"')
                    .to_string(),
            )
        })
    };
    let name = field("PRETTY_NAME")
        .or_else(|| field("NAME"))
        .unwrap_or_else(|| "Linux".into());
    Some(format!("{name} {}", uname().machine))
}

const DMI_PLACEHOLDERS: &[&str] = &[
    "To be filled by O.E.M.",
    "System Product Name",
    "System Version",
    "Default string",
    "Not Applicable",
    "None",
    "OEM",
    "O.E.M.",
    "Type1ProductConfigId",
    "Undefined",
];

fn host() -> Option<String> {
    if wsl::current().is_some()
        && let Some(host) = wsl::host()
    {
        return Some(host);
    }
    let dmi = |f: &str| {
        read(format!("/sys/devices/virtual/dmi/id/{f}"))
            .filter(|v| !DMI_PLACEHOLDERS.iter().any(|p| v.eq_ignore_ascii_case(p)))
    };
    if let Some(name) = dmi("product_name") {
        return Some(match dmi("product_version") {
            Some(version) if !name.contains(&version) => format!("{name} {version}"),
            _ => name,
        });
    }
    if let Some(model) = read("/sys/firmware/devicetree/base/model") {
        return Some(model.trim_end_matches('\0').to_string());
    }
    // WSL without a working `wslinfo`.
    wsl::current().map(|version| match version {
        wsl::Wsl::V2 => "Windows Subsystem for Linux (WSL2)".into(),
        wsl::Wsl::V1 => "Windows Subsystem for Linux".into(),
    })
}

/// The Windows version under WSL. `cmd.exe` is slow, so it's cached per boot.
fn windows() -> Option<String> {
    wsl::current()?;
    cached("windows", wsl::windows)
}

fn kernel() -> Option<String> {
    Some(uname().release)
}

fn uptime() -> Option<String> {
    let secs: u64 = read("/proc/uptime")?.split('.').next()?.parse().ok()?;
    let unit = |n: u64, name: &str| match n {
        0 => None,
        1 => Some(format!("1 {name}")),
        _ => Some(format!("{n} {name}s")),
    };
    let parts: Vec<String> = [
        unit(secs / 86400, "day"),
        unit(secs / 3600 % 24, "hour"),
        unit(secs / 60 % 60, "min"),
    ]
    .into_iter()
    .flatten()
    .collect();
    Some(if parts.is_empty() {
        unit(secs, "sec")?
    } else {
        parts.join(", ")
    })
}

fn packages() -> Option<String> {
    let home = env::var("HOME").unwrap_or_default();
    let count_lines = |path: &str, pred: fn(&str) -> bool| {
        fs::read_to_string(path)
            .ok()
            .map(|s| s.lines().filter(|l| pred(l)).count())
    };
    let counts = [
        (
            "dpkg",
            count_lines("/var/lib/dpkg/status", |l| {
                l.starts_with("Status: ") && l.ends_with(" installed")
            }),
        ),
        ("pacman", count_dirs("/var/lib/pacman/local", &[])),
        ("rpm", rpm_count()),
        (
            "apk",
            count_lines("/lib/apk/db/installed", |l| l.starts_with("P:")),
        ),
        (
            "emerge",
            Some(
                sorted_dir("/var/db/pkg")
                    .iter()
                    .filter_map(|c| count_dirs(c, &[]))
                    .sum(),
            ),
        ),
        ("brew", count_dirs("/home/linuxbrew/.linuxbrew/Cellar", &[])),
        (
            "flatpak",
            Some(
                count_dirs("/var/lib/flatpak/app", &[]).unwrap_or(0)
                    + count_dirs(format!("{home}/.local/share/flatpak/app"), &[]).unwrap_or(0),
            ),
        ),
        ("snap", count_dirs("/snap", &["bin"])),
    ];
    let parts: Vec<String> = counts
        .into_iter()
        .filter_map(|(manager, n)| n.filter(|&n| n > 0).map(|n| format!("{n} ({manager})")))
        .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn count_dirs(path: impl AsRef<Path>, skip: &[&str]) -> Option<usize> {
    let entries = fs::read_dir(path).ok()?.filter_map(Result::ok);
    Some(
        entries
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| !e.file_name().to_str().is_some_and(|n| skip.contains(&n)))
            .count(),
    )
}

fn rpm_count() -> Option<usize> {
    if !Path::new("/var/lib/rpm").exists() && !Path::new("/usr/lib/sysimage/rpm").exists() {
        return None;
    }
    let mut rpm = Command::new("rpm");
    rpm.args(["-qa", "--qf", ".\n"]);
    let out = command::run(rpm)?;
    Some(out.stdout.iter().filter(|&&b| b == b'\n').count())
}

fn shell() -> Option<String> {
    let path = env_nonempty("SHELL")?;
    let name = Path::new(&path).file_name()?.to_string_lossy().into_owned();
    // Only ask shells known to answer `--version` sanely.
    if !matches!(name.as_str(), "bash" | "zsh" | "fish" | "nu" | "tcsh") {
        return Some(name);
    }
    let mut cmd = Command::new(&path);
    cmd.arg("--version");
    let version = command::run(cmd).and_then(|o| {
        let out = String::from_utf8_lossy(&o.stdout).into_owned();
        let word = out
            .lines()
            .next()?
            .split_whitespace()
            .find(|w| w.starts_with(|c: char| c.is_ascii_digit()))?;
        Some(
            word.chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect::<String>(),
        )
    });
    Some(match version {
        Some(v) => format!("{name} {v}"),
        None => name,
    })
}

fn resolution() -> Option<String> {
    let modes: Vec<String> = sorted_dir("/sys/class/drm")
        .into_iter()
        .filter(|p| read(p.join("status")).as_deref() == Some("connected"))
        .filter_map(|p| Some(read(p.join("modes"))?.lines().next()?.to_string()))
        .collect();
    (!modes.is_empty()).then(|| modes.join(", "))
}

fn de() -> Option<String> {
    let de = env_nonempty("XDG_CURRENT_DESKTOP").or_else(|| env_nonempty("DESKTOP_SESSION"))?;
    // e.g. "ubuntu:GNOME" -> "GNOME"
    Some(de.rsplit(':').next().unwrap_or(&de).to_string())
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

fn wm() -> Option<String> {
    if env::var_os("DISPLAY").is_none() && env::var_os("WAYLAND_DISPLAY").is_none() {
        return None;
    }
    sorted_dir("/proc").into_iter().find_map(|p| {
        let comm = read(p.join("comm"))?;
        // xmonad's binary is named after the platform, e.g. "xmonad-x86_64-linux".
        let comm = if comm.starts_with("xmonad") {
            "xmonad"
        } else {
            &comm
        };
        WMS.iter()
            .find(|(c, _)| *c == comm)
            .map(|(_, name)| name.to_string())
    })
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

fn terminal() -> Option<String> {
    if let Some(name) = env_terminal(|key| env::var(key).ok()) {
        return Some(name);
    }
    let mut pid = unsafe { libc::getppid() } as u32;
    while pid > 1 {
        let Some(stat) = read(format!("/proc/{pid}/stat")) else {
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
    env_nonempty("TERM")
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

fn cpu() -> Option<String> {
    let info = read("/proc/cpuinfo")?;
    let field = |key: &str| {
        info.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == key).then(|| v.trim().to_string())
        })
    };
    let threads = info.lines().filter(|l| l.starts_with("processor")).count();
    let raw = field("model name")
        .or_else(|| field("Hardware"))
        .or_else(|| field("cpu model"))?;

    // Strip the marketing noise, e.g. "Intel(R) Core(TM) i7 CPU @ 2.50GHz", "8-Core Processor".
    let (name, nominal) = match raw.split_once(" @ ") {
        Some((name, freq)) => (
            name,
            freq.trim().strip_suffix("GHz").and_then(|f| f.parse().ok()),
        ),
        None => (raw.as_str(), None),
    };
    let mut name = ["(R)", "(r)", "(TM)", "(tm)", " CPU", " Processor"]
        .iter()
        .fold(name.to_string(), |n, junk| n.replace(junk, ""));
    if let Some(i) = name.find(" with Radeon") {
        name.truncate(i);
    }
    let name = name
        .split_whitespace()
        .filter(|w| !w.ends_with("-Core"))
        .collect::<Vec<_>>()
        .join(" ");

    let max_khz: Option<f64> =
        read("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq").and_then(|s| s.parse().ok());
    let ghz = max_khz
        .map(|khz| khz / 1e6)
        .or(nominal)
        .or_else(|| field("cpu MHz")?.parse::<f64>().ok().map(|mhz| mhz / 1e3));

    let mut out = name;
    if threads > 0 {
        out += &format!(" ({threads})");
    }
    if let Some(ghz) = ghz {
        out += &format!(" @ {ghz:.2}GHz");
    }
    Some(out)
}

fn gpu() -> Option<String> {
    // On WSL the PCI scan only finds the virtual "Microsoft Basic Render Driver".
    if wsl::current().is_some()
        && let Some(gpus) = cached("gpu", wsl::gpus)
    {
        return Some(gpus);
    }
    let mut pci_ids: Option<Option<String>> = None;
    let mut gpus: Vec<String> = Vec::new();
    for dev in sorted_dir("/sys/bus/pci/devices") {
        // PCI class 0x03xxxx = display controller.
        if !read(dev.join("class")).is_some_and(|c| c.starts_with("0x03")) {
            continue;
        }
        let id = |f: &str| read(dev.join(f)).map(|s| s.trim_start_matches("0x").to_lowercase());
        let (Some(vendor), Some(device)) = (id("vendor"), id("device")) else {
            continue;
        };
        let ids = pci_ids.get_or_insert_with(|| {
            [
                "/usr/share/hwdata/pci.ids",
                "/usr/share/misc/pci.ids",
                "/usr/share/pci.ids",
            ]
            .iter()
            .find_map(|p| fs::read_to_string(p).ok())
        });
        let name = gpu_name(ids.as_deref(), &vendor, &device);
        if !gpus.contains(&name) {
            gpus.push(name);
        }
    }
    (!gpus.is_empty()).then(|| gpus.join("\n"))
}

fn gpu_name(ids: Option<&str>, vendor: &str, device: &str) -> String {
    let (vendor_name, device_name) = ids
        .map(|ids| pci_lookup(ids, vendor, device))
        .unwrap_or_default();
    let vendor = match vendor {
        "10de" => "NVIDIA",
        "1002" => "AMD",
        "8086" => "Intel",
        "1414" => "Microsoft",
        "15ad" => "VMware",
        "80ee" => "VirtualBox",
        "1af4" => "Red Hat",
        "1234" => "QEMU",
        _ => vendor_name.as_deref().unwrap_or(vendor),
    };
    // "GA102 [GeForce RTX 3080]" -> "GeForce RTX 3080"
    let model = match device_name {
        Some(d) => match (d.rfind('['), d.rfind(']')) {
            (Some(a), Some(b)) if a < b => d[a + 1..b].to_string(),
            _ => d,
        },
        None => format!("Device {device}"),
    };
    format!("{vendor} {model}")
}

/// Looks up vendor and device names in a pci.ids database.
fn pci_lookup(ids: &str, vendor: &str, device: &str) -> (Option<String>, Option<String>) {
    let vendor_name = |l: &str| {
        l.strip_prefix(vendor)?
            .strip_prefix("  ")
            .map(str::to_string)
    };
    let mut lines = ids.lines().skip_while(|l| vendor_name(l).is_none());
    let Some(vendor_line) = lines.next() else {
        return (None, None);
    };
    let device_name = lines
        .take_while(|l| l.starts_with('\t') || l.starts_with('#'))
        .find_map(|l| {
            Some(
                l.strip_prefix('\t')?
                    .strip_prefix(device)?
                    .strip_prefix("  ")?
                    .to_string(),
            )
        });
    (vendor_name(vendor_line), device_name)
}

fn memory() -> Option<String> {
    let info = read("/proc/meminfo")?;
    let kib = |key: &str| {
        info.lines().find_map(|l| {
            l.strip_prefix(key)?
                .strip_prefix(':')?
                .trim()
                .strip_suffix(" kB")?
                .parse::<u64>()
                .ok()
        })
    };
    let total = kib("MemTotal")?;
    // Same formula as neofetch (and htop).
    let free = kib("MemFree")?
        + kib("Buffers").unwrap_or(0)
        + kib("Cached").unwrap_or(0)
        + kib("SReclaimable").unwrap_or(0);
    let used = (total + kib("Shmem").unwrap_or(0)).saturating_sub(free);
    Some(usage(used * 1024, total * 1024, total * 1024))
}

fn disk() -> Option<String> {
    // SAFETY: statvfs only fills in the zeroed struct.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c"/".as_ptr(), &mut st) } != 0 {
        return None;
    }
    let block = st.f_frsize as u64;
    let total = st.f_blocks as u64 * block;
    let used = (st.f_blocks as u64 - st.f_bfree as u64) * block;
    // Like df: percentage of the space available to unprivileged users.
    Some(usage(used, total, used + st.f_bavail as u64 * block))
}

fn usage(used: u64, total: u64, capacity: u64) -> String {
    format!(
        "{} / {} ({}%)",
        human_size(used),
        human_size(total),
        used * 100 / capacity.max(1)
    )
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    let (mut value, mut unit) = (bytes as f64 / 1024.0, 0);
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2} {}", UNITS[unit])
}

fn battery() -> Option<String> {
    let batteries: Vec<String> = sorted_dir("/sys/class/power_supply")
        .into_iter()
        .filter(|p| read(p.join("type")).as_deref() == Some("Battery"))
        .filter_map(|p| {
            let capacity = read(p.join("capacity"))?;
            Some(match read(p.join("status")) {
                Some(status) => format!("{capacity}% [{status}]"),
                None => format!("{capacity}%"),
            })
        })
        .collect();
    (!batteries.is_empty()).then(|| batteries.join("\n"))
}

fn locale() -> Option<String> {
    env_nonempty("LC_ALL").or_else(|| env_nonempty("LANG"))
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

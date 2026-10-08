//! What modules report: plain structured data, plus how each value is shown
//! after its label and which named fields it exposes (for quips and `--format`).
//! The types are plain structs so they can derive `Serialize` for `--json`.

use std::fmt;

/// What every module value can do.
pub trait Report {
    /// The text after the label. A value with several lines (two GPUs, say) is
    /// shown as one row per line under the same label; an empty one, not at all.
    fn display(&self) -> String;

    /// The field `name`, e.g. `pct` of `memory`. `None` for a field the value
    /// doesn't have (or doesn't know, such as an unknown clock speed).
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "for quips (M5) and --format (M6)")
    )]
    fn field(&self, name: &str) -> Option<Field>;
}

/// A named field of a module value.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "for quips (M5) and --format (M6)")
)]
pub enum Field {
    Int(u64),
    Float(f64),
    Text(String),
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Field::Int(n) => write!(f, "{n}"),
            Field::Float(x) => write!(f, "{x:.2}"),
            Field::Text(s) => f.write_str(s),
        }
    }
}

/// The value of one module. Each variant is named after its module.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Os(Os),
    Host(Name),
    Windows(Windows),
    Kernel(Kernel),
    Uptime(Uptime),
    Packages(Packages),
    Shell(Shell),
    Resolution(Resolution),
    De(Name),
    Wm(Name),
    Terminal(Name),
    Cpu(Cpu),
    Gpu(Gpus),
    Memory(Usage),
    Disk(Usage),
    Battery(Batteries),
    Locale(Name),
}

impl Value {
    fn report(&self) -> &dyn Report {
        match self {
            Value::Os(v) => v,
            Value::Host(v) | Value::De(v) | Value::Wm(v) | Value::Terminal(v) => v,
            Value::Locale(v) => v,
            Value::Windows(v) => v,
            Value::Kernel(v) => v,
            Value::Uptime(v) => v,
            Value::Packages(v) => v,
            Value::Shell(v) => v,
            Value::Resolution(v) => v,
            Value::Cpu(v) => v,
            Value::Gpu(v) => v,
            Value::Memory(v) | Value::Disk(v) => v,
            Value::Battery(v) => v,
        }
    }
}

impl Report for Value {
    fn display(&self) -> String {
        self.report().display()
    }

    fn field(&self, name: &str) -> Option<Field> {
        self.report().field(name)
    }
}

fn text(s: &str) -> Option<Field> {
    Some(Field::Text(s.to_string()))
}

fn int(n: impl TryInto<u64>) -> Option<Field> {
    n.try_into().ok().map(Field::Int)
}

/// A value that is just a name: host model, DE, WM, terminal, locale.
#[derive(Clone, Debug, PartialEq)]
pub struct Name {
    pub name: String,
}

impl Name {
    pub fn new(name: impl Into<String>) -> Name {
        Name { name: name.into() }
    }
}

impl Report for Name {
    fn display(&self) -> String {
        self.name.clone()
    }

    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "name" => text(&self.name),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Os {
    /// e.g. "Ubuntu 24.04.5 LTS".
    pub name: String,
    /// e.g. "x86_64".
    pub arch: String,
}

impl Report for Os {
    fn display(&self) -> String {
        format!("{} {}", self.name, self.arch)
    }

    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "name" => text(&self.name),
            "arch" => text(&self.arch),
            _ => None,
        }
    }
}

/// The Windows version under WSL.
#[derive(Clone, Debug, PartialEq)]
pub struct Windows {
    /// "Windows 11" or "Windows 10".
    pub name: String,
    pub build: Option<u32>,
}

impl Windows {
    /// Parses what `display` shows, "Windows 11 (build 26300)", which is also
    /// the form the fact cache keeps. Anything else is all name.
    pub fn parse(s: &str) -> Windows {
        let parsed = s
            .strip_suffix(')')
            .and_then(|s| s.rsplit_once(" (build "))
            .and_then(|(name, build)| Some((name, build.parse::<u32>().ok()?)))
            .map(|(name, build)| Windows {
                name: name.to_string(),
                build: Some(build),
            });
        match parsed {
            // Only when it shows exactly as before, e.g. not for "(build 007)".
            Some(w) if w.display() == s => w,
            _ => Windows {
                name: s.to_string(),
                build: None,
            },
        }
    }
}

impl Report for Windows {
    fn display(&self) -> String {
        match self.build {
            Some(build) => format!("{} (build {build})", self.name),
            None => self.name.clone(),
        }
    }

    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "name" => text(&self.name),
            "build" => int(self.build?),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Kernel {
    pub release: String,
}

impl Report for Kernel {
    fn display(&self) -> String {
        self.release.clone()
    }

    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "release" => text(&self.release),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Uptime {
    pub seconds: u64,
}

impl Report for Uptime {
    /// "2 days, 3 hours, 1 min"; seconds only in the first minute.
    fn display(&self) -> String {
        let secs = self.seconds;
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
        match parts.is_empty() {
            true => unit(secs, "sec").unwrap_or_default(),
            false => parts.join(", "),
        }
    }

    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "seconds" => int(self.seconds),
            "days" => int(self.seconds / 86400),
            _ => None,
        }
    }
}

/// Installed packages per package manager.
#[derive(Clone, Debug, PartialEq)]
pub struct Packages {
    /// Managers with at least one package, in display order.
    pub managers: Vec<ManagerCount>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ManagerCount {
    /// "dpkg", "pacman", "flatpak", …
    pub manager: &'static str,
    pub count: usize,
}

impl Packages {
    pub fn total(&self) -> usize {
        self.managers.iter().map(|m| m.count).sum()
    }
}

impl Report for Packages {
    fn display(&self) -> String {
        let parts: Vec<String> = self
            .managers
            .iter()
            .map(|m| format!("{} ({})", m.count, m.manager))
            .collect();
        parts.join(", ")
    }

    /// `total`, or the count of one manager by its name, e.g. `flatpak`.
    fn field(&self, name: &str) -> Option<Field> {
        if name == "total" {
            return int(self.total());
        }
        let manager = self.managers.iter().find(|m| m.manager == name)?;
        int(manager.count)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shell {
    /// The binary's name, e.g. "zsh".
    pub name: String,
    pub version: Option<String>,
}

impl Report for Shell {
    fn display(&self) -> String {
        match &self.version {
            Some(version) => format!("{} {version}", self.name),
            None => self.name.clone(),
        }
    }

    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "name" => text(&self.name),
            "version" => text(self.version.as_deref()?),
            _ => None,
        }
    }
}

/// The preferred mode of each connected display.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolution {
    /// e.g. "2560x1440".
    pub modes: Vec<String>,
}

impl Report for Resolution {
    fn display(&self) -> String {
        self.modes.join(", ")
    }

    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "count" => int(self.modes.len()),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cpu {
    /// The model without the marketing noise, e.g. "AMD Ryzen 9 7950X".
    pub name: String,
    /// Logical CPUs; 0 if unknown.
    pub threads: usize,
    /// The maximum (or else nominal or current) clock.
    pub ghz: Option<f64>,
}

impl Report for Cpu {
    fn display(&self) -> String {
        let mut out = self.name.clone();
        if self.threads > 0 {
            out += &format!(" ({})", self.threads);
        }
        if let Some(ghz) = self.ghz {
            out += &format!(" @ {ghz:.2}GHz");
        }
        out
    }

    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "name" => text(&self.name),
            "threads" => int(self.threads),
            "ghz" => Some(Field::Float(self.ghz?)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gpus {
    /// Full names, e.g. "NVIDIA GeForce RTX 4090", one per GPU.
    pub names: Vec<String>,
}

impl Report for Gpus {
    fn display(&self) -> String {
        self.names.join("\n")
    }

    /// `name` is the first GPU's.
    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "name" => text(self.names.first()?),
            "count" => int(self.names.len()),
            _ => None,
        }
    }
}

/// Used and total space, for memory and disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    pub used_bytes: u64,
    pub total_bytes: u64,
    /// Percent used, rounded down. Not always `used / total`: a disk's is of
    /// the space available to unprivileged users, as `df` shows it.
    pub pct: u64,
}

impl Usage {
    /// `used` of `total`, where `capacity` is what the percentage is out of.
    pub fn new(used_bytes: u64, total_bytes: u64, capacity: u64) -> Usage {
        Usage {
            used_bytes,
            total_bytes,
            pct: used_bytes * 100 / capacity.max(1),
        }
    }
}

impl Report for Usage {
    fn display(&self) -> String {
        format!(
            "{} / {} ({}%)",
            human_size(self.used_bytes),
            human_size(self.total_bytes),
            self.pct
        )
    }

    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "used_bytes" => int(self.used_bytes),
            "total_bytes" => int(self.total_bytes),
            "pct" => int(self.pct),
            _ => None,
        }
    }
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

#[derive(Clone, Debug, PartialEq)]
pub struct Batteries {
    pub batteries: Vec<Battery>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Battery {
    /// Charge in percent.
    pub pct: u64,
    /// As the kernel says it: "Charging", "Discharging", "Full", …
    pub status: Option<String>,
}

impl Report for Batteries {
    fn display(&self) -> String {
        let lines: Vec<String> = self
            .batteries
            .iter()
            .map(|b| match &b.status {
                Some(status) => format!("{}% [{status}]", b.pct),
                None => format!("{}%", b.pct),
            })
            .collect();
        lines.join("\n")
    }

    /// The first battery's `pct` and `status`.
    fn field(&self, name: &str) -> Option<Field> {
        let first = self.batteries.first()?;
        match name {
            "pct" => int(first.pct),
            "status" => text(first.status.as_deref()?),
            "count" => int(self.batteries.len()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_formatting() {
        let up = |seconds| Uptime { seconds }.display();
        assert_eq!(up(0), "");
        assert_eq!(up(1), "1 sec");
        assert_eq!(up(59), "59 secs");
        assert_eq!(up(60), "1 min");
        assert_eq!(up(3600), "1 hour");
        assert_eq!(up(21919), "6 hours, 5 mins");
        assert_eq!(up(86400 + 61), "1 day, 1 min");
        assert_eq!(up(1_000_000), "11 days, 13 hours, 46 mins");
        assert_eq!(Uptime { seconds: 1_000_000 }.field("days"), int(11));
    }

    #[test]
    fn usage_formatting() {
        let gib = 1024 * 1024 * 1024;
        let mem = Usage::new(3 * gib / 2, 16 * gib, 16 * gib);
        assert_eq!(mem.display(), "1.50 GiB / 16.00 GiB (9%)");
        assert_eq!(mem.field("pct"), int(9u64));
        assert_eq!(
            Usage::new(512, 1024, 1024).display(),
            "0.50 KiB / 1.00 KiB (50%)"
        );
        assert_eq!(Usage::new(0, 0, 0).pct, 0, "no division by zero");
        assert_eq!(human_size(5 * 1024 * gib * 1024), "5120.00 TiB");
    }

    #[test]
    fn windows_round_trips_through_the_fact_cache() {
        for s in [
            "Windows 11 (build 26300)",
            "Windows 10 (build 19045)",
            "Windows 99 (build 1)",
            "Windows 11 (build 007)",
            "Windows 11 (build )",
            "Windows 11",
            "",
        ] {
            assert_eq!(Windows::parse(s).display(), s);
        }
        let w = Windows::parse("Windows 11 (build 26300)");
        assert_eq!(w.name, "Windows 11");
        assert_eq!(w.field("build"), int(26300u32));
        assert_eq!(Windows::parse("Windows 11 (build 007)").build, None);
    }

    #[test]
    fn field_formatting() {
        assert_eq!(Field::Int(42).to_string(), "42");
        assert_eq!(Field::Float(2.5).to_string(), "2.50");
        assert_eq!(Field::Text("KDE".into()).to_string(), "KDE");
    }

    #[test]
    fn packages_fields() {
        let p = Packages {
            managers: vec![
                ManagerCount {
                    manager: "pacman",
                    count: 7,
                },
                ManagerCount {
                    manager: "flatpak",
                    count: 3,
                },
            ],
        };
        assert_eq!(p.display(), "7 (pacman), 3 (flatpak)");
        assert_eq!(p.field("total"), int(10usize));
        assert_eq!(p.field("flatpak"), int(3usize));
        assert_eq!(p.field("dpkg"), None);
    }
}

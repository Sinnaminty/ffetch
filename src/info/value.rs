//! What modules report: plain structured data, plus how each value is shown
//! after its label and which named fields it exposes (for quips and `--format`).
//! The types are plain structs so they can derive `Serialize` for `--json`.

use std::{fmt, net::Ipv4Addr};

/// What every module value can do.
pub trait Report {
    /// The text after the label. A value with several lines (two GPUs, say) is
    /// shown as one row per line under the same label; an empty one, not at all.
    fn display(&self) -> String;

    /// The field `name`, e.g. `pct` of `memory`. `None` for a field the value
    /// doesn't have (or doesn't know, such as an unknown clock speed).
    fn field(&self, name: &str) -> Option<Field>;

    /// What a usage bar before the text measures; the bar shows the `pct`
    /// field. `None` for values shown without one.
    fn gauge(&self) -> Option<Gauge> {
        None
    }
}

/// What a usage bar measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gauge {
    /// Memory or disk space in use: the more, the worse.
    Used,
    /// Battery charge: the less, the worse.
    Charge,
}

/// How worrying a usage bar's percentage is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Crit,
}

/// A percentage shown as a usage bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meter {
    pub gauge: Gauge,
    pub pct: u64,
}

impl Meter {
    /// In use: ok below 60%, warn up to 85%, crit above. A charge is the other
    /// way round: ok above 40%, warn down to 15%, crit below.
    pub fn level(self) -> Level {
        match (self.gauge, self.pct) {
            (Gauge::Used, 0..60) | (Gauge::Charge, 41..) => Level::Ok,
            (Gauge::Used, 60..=85) | (Gauge::Charge, 15..=40) => Level::Warn,
            _ => Level::Crit,
        }
    }
}

/// A named field of a module value.
#[derive(Clone, Debug, PartialEq)]
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
    Temp(Temp),
    Load(Load),
    Gpu(Gpus),
    Memory(Usage),
    Disk(Usage),
    Battery(Batteries),
    Git(Git),
    Locale(Name),
    Toolchains(Toolchains),
    Docker(Containers),
    Ip(LocalIp),
    Updates(Updates),
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
            Value::Temp(v) => v,
            Value::Load(v) => v,
            Value::Gpu(v) => v,
            Value::Memory(v) | Value::Disk(v) => v,
            Value::Battery(v) => v,
            Value::Git(v) => v,
            Value::Toolchains(v) => v,
            Value::Docker(v) => v,
            Value::Ip(v) => v,
            Value::Updates(v) => v,
        }
    }

    /// The value in the pieces that get a usage bar each: one per battery, or
    /// else the whole value.
    pub fn parts(&self) -> Vec<&dyn Report> {
        match self {
            Value::Battery(b) => b.batteries.iter().map(|b| b as &dyn Report).collect(),
            v => vec![v.report()],
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

    fn gauge(&self) -> Option<Gauge> {
        self.report().gauge()
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

    fn gauge(&self) -> Option<Gauge> {
        Some(Gauge::Used)
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
    /// A line per battery.
    fn display(&self) -> String {
        let lines: Vec<String> = self.batteries.iter().map(Battery::display).collect();
        lines.join("\n")
    }

    /// The first battery's `pct` and `status`.
    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "count" => int(self.batteries.len()),
            _ => self.batteries.first()?.field(name),
        }
    }
}

impl Report for Battery {
    fn display(&self) -> String {
        match &self.status {
            Some(status) => format!("{}% [{status}]", self.pct),
            None => format!("{}%", self.pct),
        }
    }

    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "pct" => int(self.pct),
            "status" => text(self.status.as_deref()?),
            _ => None,
        }
    }

    fn gauge(&self) -> Option<Gauge> {
        Some(Gauge::Charge)
    }
}

/// The CPU temperature.
#[derive(Clone, Debug, PartialEq)]
pub struct Temp {
    pub celsius: f64,
}

impl Report for Temp {
    /// "54°C"
    fn display(&self) -> String {
        format!("{}°C", self.celsius.round())
    }

    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "celsius" => Some(Field::Float(self.celsius)),
            _ => None,
        }
    }
}

/// The load average: runnable and waiting tasks over 1, 5 and 15 minutes.
#[derive(Clone, Debug, PartialEq)]
pub struct Load {
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    /// Logical CPUs, for scale; 0 if unknown.
    pub threads: usize,
}

impl Report for Load {
    /// "0.14, 0.15, 0.08 (16 threads)"
    fn display(&self) -> String {
        let averages = format!("{:.2}, {:.2}, {:.2}", self.load1, self.load5, self.load15);
        match self.threads {
            0 => averages,
            1 => format!("{averages} (1 thread)"),
            n => format!("{averages} ({n} threads)"),
        }
    }

    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "load1" => Some(Field::Float(self.load1)),
            "load5" => Some(Field::Float(self.load5)),
            "load15" => Some(Field::Float(self.load15)),
            "threads" if self.threads > 0 => int(self.threads),
            _ => None,
        }
    }
}

/// The git status of the working directory.
#[derive(Clone, Debug, PartialEq)]
pub struct Git {
    /// The branch, or the short commit ID when the HEAD is detached.
    pub branch: String,
    /// Commits ahead of and behind the upstream branch, if there is one.
    pub ahead_behind: Option<(u64, u64)>,
    /// Files with changes, staged or not, including untracked ones.
    pub changed: u64,
}

impl Report for Git {
    /// "main ↑1 ↓0, 3 changed" or "main, clean". The arrows only show when the
    /// branch and its upstream differ.
    fn display(&self) -> String {
        let mut out = self.branch.clone();
        if let Some((ahead, behind)) = self.ahead_behind
            && (ahead, behind) != (0, 0)
        {
            out += &format!(" ↑{ahead} ↓{behind}");
        }
        match self.changed {
            0 => out + ", clean",
            n => out + &format!(", {n} changed"),
        }
    }

    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "branch" => text(&self.branch),
            "ahead" => int(self.ahead_behind?.0),
            "behind" => int(self.ahead_behind?.1),
            "changed" => int(self.changed),
            _ => None,
        }
    }
}

/// The installed toolchains that answered `--version`.
#[derive(Clone, Debug, PartialEq)]
pub struct Toolchains {
    pub tools: Vec<Tool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tool {
    /// "rust", "node" or "python".
    pub name: &'static str,
    /// e.g. "1.98.1".
    pub version: String,
}

impl Report for Toolchains {
    /// "rust 1.98.1 · node 22.11.0 · python 3.12.3"
    fn display(&self) -> String {
        let tools: Vec<String> = self
            .tools
            .iter()
            .map(|t| format!("{} {}", t.name, t.version))
            .collect();
        tools.join(" · ")
    }

    /// `count`, or the version of one toolchain by its name, e.g. `rust`.
    fn field(&self, name: &str) -> Option<Field> {
        if name == "count" {
            return int(self.tools.len());
        }
        text(&self.tools.iter().find(|t| t.name == name)?.version)
    }
}

/// Docker's running containers.
#[derive(Clone, Debug, PartialEq)]
pub struct Containers {
    pub running: usize,
}

impl Report for Containers {
    /// "3 running"
    fn display(&self) -> String {
        format!("{} running", self.running)
    }

    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "running" => int(self.running),
            _ => None,
        }
    }
}

/// The machine's address on the local network.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalIp {
    pub address: Ipv4Addr,
    /// e.g. "eth0".
    pub interface: String,
}

impl Report for LocalIp {
    /// "192.168.1.20 (eth0)"
    fn display(&self) -> String {
        format!("{} ({})", self.address, self.interface)
    }

    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "address" => text(&self.address.to_string()),
            "interface" => text(&self.interface),
            _ => None,
        }
    }
}

/// Pending package updates.
#[derive(Clone, Debug, PartialEq)]
pub struct Updates {
    pub total: u64,
    /// How many of them are security updates.
    pub security: u64,
}

impl Report for Updates {
    /// "15 (5 security)", "15", or "up to date".
    fn display(&self) -> String {
        match (self.total, self.security) {
            (0, _) => "up to date".into(),
            (total, 0) => total.to_string(),
            (total, security) => format!("{total} ({security} security)"),
        }
    }

    fn field(&self, name: &str) -> Option<Field> {
        match name {
            "total" => int(self.total),
            "security" => int(self.security),
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

    #[test]
    fn meter_levels() {
        let level = |gauge, pct| Meter { gauge, pct }.level();
        for (pct, used, charge) in [
            (0, Level::Ok, Level::Crit),
            (14, Level::Ok, Level::Crit),
            (15, Level::Ok, Level::Warn),
            (40, Level::Ok, Level::Warn),
            (41, Level::Ok, Level::Ok),
            (59, Level::Ok, Level::Ok),
            (60, Level::Warn, Level::Ok),
            (85, Level::Warn, Level::Ok),
            (86, Level::Crit, Level::Ok),
            (100, Level::Crit, Level::Ok),
        ] {
            assert_eq!(level(Gauge::Used, pct), used, "{pct}% used");
            assert_eq!(level(Gauge::Charge, pct), charge, "{pct}% charged");
        }
    }

    #[test]
    fn each_battery_is_a_part_with_a_gauge() {
        let battery = |pct, status: Option<&str>| Battery {
            pct,
            status: status.map(str::to_string),
        };
        let value = Value::Battery(Batteries {
            batteries: vec![battery(80, Some("Charging")), battery(12, None)],
        });
        assert_eq!(value.display(), "80% [Charging]\n12%");
        assert_eq!(value.gauge(), None, "the parts have the bars");
        assert_eq!(value.field("pct"), int(80u64));
        assert_eq!(value.field("count"), int(2usize));
        let parts = value.parts();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1].display(), "12%");
        assert_eq!(parts[1].field("pct"), int(12u64));
        assert_eq!(parts[1].gauge(), Some(Gauge::Charge));

        let memory = Value::Memory(Usage::new(1, 2, 2));
        assert_eq!(memory.parts().len(), 1);
        assert_eq!(memory.gauge(), Some(Gauge::Used));
        assert_eq!(
            Value::Kernel(Kernel {
                release: "6.1".into()
            })
            .gauge(),
            None
        );
    }

    #[test]
    fn load_formatting() {
        let load = |threads| Load {
            load1: 0.14,
            load5: 0.155,
            load15: 12.0,
            threads,
        };
        assert_eq!(load(16).display(), "0.14, 0.15, 12.00 (16 threads)");
        assert_eq!(load(1).display(), "0.14, 0.15, 12.00 (1 thread)");
        assert_eq!(load(0).display(), "0.14, 0.15, 12.00");
        assert_eq!(load(16).field("load1"), Some(Field::Float(0.14)));
        assert_eq!(load(16).field("threads"), int(16usize));
        assert_eq!(load(0).field("threads"), None, "unknown, not zero");
    }

    #[test]
    fn temp_formatting() {
        let temp = |celsius| Temp { celsius }.display();
        assert_eq!(temp(54.125), "54°C");
        assert_eq!(temp(54.5), "55°C");
        assert_eq!(temp(100.0), "100°C");
    }

    #[test]
    fn git_formatting() {
        let git = |ahead_behind, changed| Git {
            branch: "main".into(),
            ahead_behind,
            changed,
        };
        assert_eq!(git(Some((1, 0)), 3).display(), "main ↑1 ↓0, 3 changed");
        assert_eq!(git(Some((0, 2)), 0).display(), "main ↑0 ↓2, clean");
        assert_eq!(git(Some((0, 0)), 0).display(), "main, clean");
        assert_eq!(git(None, 1).display(), "main, 1 changed");
        assert_eq!(git(Some((0, 0)), 0).field("ahead"), int(0u64));
        assert_eq!(git(None, 0).field("behind"), None);
        assert_eq!(git(None, 4).field("changed"), int(4u64));
    }

    #[test]
    fn small_module_formatting() {
        let tools = Toolchains {
            tools: vec![
                Tool {
                    name: "rust",
                    version: "1.98.1".into(),
                },
                Tool {
                    name: "python",
                    version: "3.12.3".into(),
                },
            ],
        };
        assert_eq!(tools.display(), "rust 1.98.1 · python 3.12.3");
        assert_eq!(tools.field("python"), text("3.12.3"));
        assert_eq!(tools.field("node"), None);
        assert_eq!(tools.field("count"), int(2usize));

        let updates = |total, security| Updates { total, security }.display();
        assert_eq!(updates(15, 5), "15 (5 security)");
        assert_eq!(updates(1, 0), "1");
        assert_eq!(updates(0, 0), "up to date");

        assert_eq!(Containers { running: 3 }.display(), "3 running");
        let ip = LocalIp {
            address: Ipv4Addr::new(192, 168, 1, 20),
            interface: "eth0".into(),
        };
        assert_eq!(ip.display(), "192.168.1.20 (eth0)");
        assert_eq!(ip.field("address"), text("192.168.1.20"));
    }
}

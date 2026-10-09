//! What modules report: plain structured data, plus how each value is shown
//! after its label and which named fields it exposes (for quips and `--format`).
//! The types are plain structs that serialize to the structured values of
//! `--json`: snake_case keys, sizes in bytes, and unknown parts left out
//! rather than `null`.

use std::{fmt, net::Ipv4Addr};

use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeStruct},
};

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

/// The value of one module. Each variant is named after its module, and
/// serializes as the value it holds.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
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

    /// The names `field` answers to for this kind of value; the registry
    /// lists the same for each module.
    #[cfg(test)]
    pub fn fields(&self) -> &'static [&'static str] {
        match self {
            Value::Os(_) => Os::FIELDS,
            Value::Host(_) | Value::De(_) | Value::Wm(_) | Value::Terminal(_) => Name::FIELDS,
            Value::Locale(_) => Name::FIELDS,
            Value::Windows(_) => Windows::FIELDS,
            Value::Kernel(_) => Kernel::FIELDS,
            Value::Uptime(_) => Uptime::FIELDS,
            Value::Packages(_) => Packages::FIELDS,
            Value::Shell(_) => Shell::FIELDS,
            Value::Resolution(_) => Resolution::FIELDS,
            Value::Cpu(_) => Cpu::FIELDS,
            Value::Temp(_) => Temp::FIELDS,
            Value::Load(_) => Load::FIELDS,
            Value::Gpu(_) => Gpus::FIELDS,
            Value::Memory(_) | Value::Disk(_) => Usage::FIELDS,
            Value::Battery(_) => Batteries::FIELDS,
            Value::Git(_) => Git::FIELDS,
            Value::Toolchains(_) => Toolchains::FIELDS,
            Value::Docker(_) => Containers::FIELDS,
            Value::Ip(_) => LocalIp::FIELDS,
            Value::Updates(_) => Updates::FIELDS,
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

/// For `skip_serializing_if`: a count of 0 means unknown.
fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// A value that is just a name: host model, DE, WM, terminal, locale.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Name {
    pub name: String,
}

impl Name {
    pub const FIELDS: &[&str] = &["name"];

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

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Os {
    /// e.g. "Ubuntu 24.04.5 LTS".
    pub name: String,
    /// e.g. "x86_64".
    pub arch: String,
}

impl Os {
    pub const FIELDS: &[&str] = &["name", "arch"];
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
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Windows {
    /// "Windows 11" or "Windows 10".
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<u32>,
}

impl Windows {
    pub const FIELDS: &[&str] = &["name", "build"];

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

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Kernel {
    pub release: String,
}

impl Kernel {
    pub const FIELDS: &[&str] = &["release"];
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

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Uptime {
    pub seconds: u64,
}

impl Uptime {
    pub const FIELDS: &[&str] = &["seconds", "days"];
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
    /// `total`, then each package manager `packages` knows.
    pub const FIELDS: &[&str] = &[
        "total", "dpkg", "pacman", "rpm", "apk", "emerge", "brew", "flatpak", "snap",
    ];

    pub fn total(&self) -> usize {
        self.managers.iter().map(|m| m.count).sum()
    }
}

/// `{"total": 1146, "managers": {"dpkg": 1140, "snap": 6}}`, managers in
/// display order.
impl Serialize for Packages {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        struct Managers<'a>(&'a [ManagerCount]);
        impl Serialize for Managers<'_> {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let mut map = s.serialize_map(Some(self.0.len()))?;
                for m in self.0 {
                    map.serialize_entry(m.manager, &m.count)?;
                }
                map.end()
            }
        }
        let mut st = s.serialize_struct("Packages", 2)?;
        st.serialize_field("total", &self.total())?;
        st.serialize_field("managers", &Managers(&self.managers))?;
        st.end()
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

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Shell {
    /// The binary's name, e.g. "zsh".
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl Shell {
    pub const FIELDS: &[&str] = &["name", "version"];
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

impl Resolution {
    pub const FIELDS: &[&str] = &["count"];
}

/// The width and height in a mode's name: "1920x1080", or "1920x1080i" for
/// an interlaced mode.
fn mode_size(mode: &str) -> Option<(u32, u32)> {
    let (width, rest) = mode.split_once('x')?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    Some((width.parse().ok()?, rest[..digits].parse().ok()?))
}

/// `[{"width": 2560, "height": 1440}, …]`, one per display. A mode named
/// some other way (the kernel doesn't) is left out.
impl Serialize for Resolution {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Size {
            width: u32,
            height: u32,
        }
        let sizes: Vec<Size> = self
            .modes
            .iter()
            .filter_map(|m| mode_size(m))
            .map(|(width, height)| Size { width, height })
            .collect();
        sizes.serialize(s)
    }
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

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Cpu {
    /// The model without the marketing noise, e.g. "AMD Ryzen 9 7950X".
    pub name: String,
    /// Logical CPUs; 0 if unknown.
    #[serde(skip_serializing_if = "is_zero")]
    pub threads: usize,
    /// The maximum (or else nominal or current) clock.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ghz: Option<f64>,
}

impl Cpu {
    pub const FIELDS: &[&str] = &["name", "threads", "ghz"];
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

/// The GPUs, in the order found. Serialized as a list.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Gpus {
    pub gpus: Vec<Gpu>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Gpu {
    /// "NVIDIA", "AMD", …, when the source says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// The model, without the vendor when that is known: "GeForce RTX 4090".
    pub name: String,
    pub source: GpuSource,
}

/// Where a GPU's name came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GpuSource {
    /// The PCI bus, with names from pci.ids.
    Pci,
    /// The NVIDIA driver, on WSL.
    NvidiaSmi,
    /// Windows' list of video controllers, on WSL.
    Powershell,
}

impl GpuSource {
    const ALL: [GpuSource; 3] = [GpuSource::Pci, GpuSource::NvidiaSmi, GpuSource::Powershell];

    /// "pci", "nvidia-smi" or "powershell", as in the JSON output.
    pub fn name(self) -> &'static str {
        match self {
            GpuSource::Pci => "pci",
            GpuSource::NvidiaSmi => "nvidia-smi",
            GpuSource::Powershell => "powershell",
        }
    }

    pub fn from_name(name: &str) -> Option<GpuSource> {
        GpuSource::ALL.into_iter().find(|s| s.name() == name)
    }
}

impl Gpus {
    pub const FIELDS: &[&str] = &["name", "vendor", "source", "count"];
}

impl Gpu {
    /// "NVIDIA GeForce RTX 4090": the vendor, if known, and the model.
    pub fn full_name(&self) -> String {
        match &self.vendor {
            Some(vendor) => format!("{vendor} {}", self.name),
            None => self.name.clone(),
        }
    }
}

impl Report for Gpus {
    /// A line per GPU.
    fn display(&self) -> String {
        let names: Vec<String> = self.gpus.iter().map(Gpu::full_name).collect();
        names.join("\n")
    }

    /// `count`, or the first GPU's `name`, `vendor` or `source`.
    fn field(&self, name: &str) -> Option<Field> {
        if name == "count" {
            return int(self.gpus.len());
        }
        let gpu = self.gpus.first()?;
        match name {
            "name" => text(&gpu.name),
            "vendor" => text(gpu.vendor.as_deref()?),
            "source" => text(gpu.source.name()),
            _ => None,
        }
    }
}

/// Used and total space, for memory and disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub used_bytes: u64,
    pub total_bytes: u64,
    /// Percent used, rounded down. Not always `used / total`: a disk's is of
    /// the space available to unprivileged users, as `df` shows it.
    pub pct: u64,
}

impl Usage {
    pub const FIELDS: &[&str] = &["used_bytes", "total_bytes", "pct"];

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

/// Serialized as a list, one per battery.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Batteries {
    pub batteries: Vec<Battery>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Battery {
    /// Charge in percent.
    pub pct: u64,
    /// As the kernel says it: "Charging", "Discharging", "Full", …
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

impl Batteries {
    pub const FIELDS: &[&str] = &["count", "pct", "status"];
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
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Temp {
    pub celsius: f64,
}

impl Temp {
    pub const FIELDS: &[&str] = &["celsius"];
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
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Load {
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    /// Logical CPUs, for scale; 0 if unknown.
    #[serde(skip_serializing_if = "is_zero")]
    pub threads: usize,
}

impl Load {
    pub const FIELDS: &[&str] = &["load1", "load5", "load15", "threads"];
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

impl Git {
    pub const FIELDS: &[&str] = &["branch", "ahead", "behind", "changed"];
}

/// `{"branch": "main", "ahead": 1, "behind": 0, "changed": 3}`, without
/// `ahead` and `behind` when there is no upstream.
impl Serialize for Git {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("Git", 4)?;
        st.serialize_field("branch", &self.branch)?;
        match self.ahead_behind {
            Some((ahead, behind)) => {
                st.serialize_field("ahead", &ahead)?;
                st.serialize_field("behind", &behind)?;
            }
            None => {
                st.skip_field("ahead")?;
                st.skip_field("behind")?;
            }
        }
        st.serialize_field("changed", &self.changed)?;
        st.end()
    }
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

impl Toolchains {
    /// `count`, then each toolchain `toolchains` asks for.
    pub const FIELDS: &[&str] = &["count", "rust", "node", "python"];
}

/// `{"rust": "1.98.1", "node": "22.11.0"}`, the installed ones only.
impl Serialize for Toolchains {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.tools.len()))?;
        for tool in &self.tools {
            map.serialize_entry(tool.name, &tool.version)?;
        }
        map.end()
    }
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
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Containers {
    pub running: usize,
}

impl Containers {
    pub const FIELDS: &[&str] = &["running"];
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
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LocalIp {
    /// Serialized as text, "192.168.1.20".
    #[serde(serialize_with = "as_text")]
    pub address: Ipv4Addr,
    /// e.g. "eth0".
    pub interface: String,
}

impl LocalIp {
    pub const FIELDS: &[&str] = &["address", "interface"];
}

/// Serializes `value` as the text it displays as.
fn as_text<S: Serializer>(value: &impl fmt::Display, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(value)
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
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Updates {
    pub total: u64,
    /// How many of them are security updates.
    pub security: u64,
}

impl Updates {
    pub const FIELDS: &[&str] = &["total", "security"];
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
    fn git_fields() {
        let git = |ahead_behind| Git {
            branch: "main".into(),
            ahead_behind,
            changed: 0,
        };
        assert_eq!(git(Some((1, 2))).field("ahead"), int(1u64));
        assert_eq!(git(Some((1, 2))).field("behind"), int(2u64));
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

    fn gpu(vendor: Option<&str>, name: &str, source: GpuSource) -> Gpu {
        Gpu {
            vendor: vendor.map(str::to_string),
            name: name.into(),
            source,
        }
    }

    #[test]
    fn gpus_show_the_vendor_before_the_name() {
        let gpus = Gpus {
            gpus: vec![
                gpu(Some("NVIDIA"), "T1200 Laptop GPU", GpuSource::NvidiaSmi),
                gpu(None, "Intel(R) UHD Graphics", GpuSource::Powershell),
            ],
        };
        assert_eq!(
            gpus.display(),
            "NVIDIA T1200 Laptop GPU\nIntel(R) UHD Graphics"
        );
        assert_eq!(gpus.field("name"), text("T1200 Laptop GPU"));
        assert_eq!(gpus.field("vendor"), text("NVIDIA"));
        assert_eq!(gpus.field("source"), text("nvidia-smi"));
        assert_eq!(gpus.field("count"), int(2usize));
        for (source, name) in [
            (GpuSource::Pci, "pci"),
            (GpuSource::NvidiaSmi, "nvidia-smi"),
            (GpuSource::Powershell, "powershell"),
        ] {
            assert_eq!(source.name(), name);
            assert_eq!(GpuSource::from_name(name), Some(source));
            assert_eq!(json(&source), serde_json::json!(name));
        }
        assert_eq!(GpuSource::from_name("PCI"), None);
    }

    fn json(value: &impl Serialize) -> serde_json::Value {
        serde_json::to_value(value).unwrap()
    }

    /// One value of every kind, with every optional part filled in, and the
    /// JSON it gives.
    fn every_kind() -> Vec<(Value, serde_json::Value)> {
        use serde_json::json;
        let name = |s: &str| Name::new(s);
        let all_managers = Packages::FIELDS[1..]
            .iter()
            .enumerate()
            .map(|(i, &manager)| ManagerCount {
                manager,
                count: i + 1,
            })
            .collect();
        vec![
            (
                Value::Os(Os {
                    name: "Ubuntu 24.04.5 LTS".into(),
                    arch: "x86_64".into(),
                }),
                json!({"name": "Ubuntu 24.04.5 LTS", "arch": "x86_64"}),
            ),
            (Value::Host(name("MS-7D75")), json!({"name": "MS-7D75"})),
            (
                Value::Windows(Windows::parse("Windows 11 (build 26300)")),
                json!({"name": "Windows 11", "build": 26300}),
            ),
            (
                Value::Kernel(Kernel {
                    release: "6.6.87".into(),
                }),
                json!({"release": "6.6.87"}),
            ),
            (
                Value::Uptime(Uptime { seconds: 11580 }),
                json!({"seconds": 11580}),
            ),
            (
                Value::Packages(Packages {
                    managers: all_managers,
                }),
                json!({"total": 36, "managers": {"dpkg": 1, "pacman": 2, "rpm": 3, "apk": 4,
                    "emerge": 5, "brew": 6, "flatpak": 7, "snap": 8}}),
            ),
            (
                Value::Shell(Shell {
                    name: "zsh".into(),
                    version: Some("5.9".into()),
                }),
                json!({"name": "zsh", "version": "5.9"}),
            ),
            (
                Value::Resolution(Resolution {
                    modes: vec!["2560x1440".into(), "1920x1080i".into()],
                }),
                json!([{"width": 2560, "height": 1440}, {"width": 1920, "height": 1080}]),
            ),
            (Value::De(name("KDE")), json!({"name": "KDE"})),
            (Value::Wm(name("KWin")), json!({"name": "KWin"})),
            (Value::Terminal(name("kitty")), json!({"name": "kitty"})),
            (
                Value::Cpu(Cpu {
                    name: "AMD Ryzen 9 7950X".into(),
                    threads: 32,
                    ghz: Some(5.881),
                }),
                json!({"name": "AMD Ryzen 9 7950X", "threads": 32, "ghz": 5.881}),
            ),
            (
                Value::Temp(Temp { celsius: 54.125 }),
                json!({"celsius": 54.125}),
            ),
            (
                Value::Load(Load {
                    load1: 0.14,
                    load5: 0.15,
                    load15: 0.08,
                    threads: 16,
                }),
                json!({"load1": 0.14, "load5": 0.15, "load15": 0.08, "threads": 16}),
            ),
            (
                Value::Gpu(Gpus {
                    gpus: vec![gpu(
                        Some("NVIDIA"),
                        "T1200 Laptop GPU",
                        GpuSource::NvidiaSmi,
                    )],
                }),
                json!([{"vendor": "NVIDIA", "name": "T1200 Laptop GPU", "source": "nvidia-smi"}]),
            ),
            (
                Value::Memory(Usage::new(1024, 4096, 4096)),
                json!({"used_bytes": 1024, "total_bytes": 4096, "pct": 25}),
            ),
            (
                Value::Disk(Usage::new(10, 100, 50)),
                json!({"used_bytes": 10, "total_bytes": 100, "pct": 20}),
            ),
            (
                Value::Battery(Batteries {
                    batteries: vec![Battery {
                        pct: 80,
                        status: Some("Charging".into()),
                    }],
                }),
                json!([{"pct": 80, "status": "Charging"}]),
            ),
            (
                Value::Git(Git {
                    branch: "main".into(),
                    ahead_behind: Some((1, 0)),
                    changed: 3,
                }),
                json!({"branch": "main", "ahead": 1, "behind": 0, "changed": 3}),
            ),
            (Value::Locale(name("C.UTF-8")), json!({"name": "C.UTF-8"})),
            (
                Value::Toolchains(Toolchains {
                    tools: vec![
                        Tool {
                            name: "rust",
                            version: "1.98.1".into(),
                        },
                        Tool {
                            name: "node",
                            version: "22.11.0".into(),
                        },
                        Tool {
                            name: "python",
                            version: "3.12.3".into(),
                        },
                    ],
                }),
                json!({"rust": "1.98.1", "node": "22.11.0", "python": "3.12.3"}),
            ),
            (
                Value::Docker(Containers { running: 3 }),
                json!({"running": 3}),
            ),
            (
                Value::Ip(LocalIp {
                    address: Ipv4Addr::new(192, 168, 1, 20),
                    interface: "eth0".into(),
                }),
                json!({"address": "192.168.1.20", "interface": "eth0"}),
            ),
            (
                Value::Updates(Updates {
                    total: 15,
                    security: 5,
                }),
                json!({"total": 15, "security": 5}),
            ),
        ]
    }

    #[test]
    fn json_shapes() {
        let values = every_kind();
        assert_eq!(values.len(), 24, "one per kind of value");
        for (value, expected) in values {
            assert_eq!(json(&value), expected, "{value:?}");
        }
    }

    #[test]
    fn json_leaves_out_what_is_unknown() {
        use serde_json::json;
        let cases = [
            (
                Value::Windows(Windows::parse("Windows 11")),
                json!({"name": "Windows 11"}),
            ),
            (
                Value::Shell(Shell {
                    name: "dash".into(),
                    version: None,
                }),
                json!({"name": "dash"}),
            ),
            (
                Value::Cpu(Cpu {
                    name: "Some CPU".into(),
                    threads: 0,
                    ghz: None,
                }),
                json!({"name": "Some CPU"}),
            ),
            (
                Value::Load(Load {
                    load1: 1.0,
                    load5: 2.0,
                    load15: 3.0,
                    threads: 0,
                }),
                json!({"load1": 1.0, "load5": 2.0, "load15": 3.0}),
            ),
            (
                Value::Gpu(Gpus {
                    gpus: vec![gpu(None, "abcd Device 0001", GpuSource::Pci)],
                }),
                json!([{"name": "abcd Device 0001", "source": "pci"}]),
            ),
            (
                Value::Battery(Batteries {
                    batteries: vec![Battery {
                        pct: 9,
                        status: None,
                    }],
                }),
                json!([{"pct": 9}]),
            ),
            (
                Value::Git(Git {
                    branch: "dev".into(),
                    ahead_behind: None,
                    changed: 0,
                }),
                json!({"branch": "dev", "changed": 0}),
            ),
            (
                Value::Resolution(Resolution {
                    modes: vec!["preferred".into(), "800x600".into()],
                }),
                json!([{"width": 800, "height": 600}]),
            ),
        ];
        for (value, expected) in cases {
            assert_eq!(json(&value), expected, "{value:?}");
        }
    }

    #[test]
    fn every_listed_field_is_answered() {
        for (value, _) in every_kind() {
            for name in value.fields() {
                assert!(value.field(name).is_some(), "{value:?} has no {name}");
            }
            assert_eq!(value.field("nope"), None, "{value:?}");
        }
    }
}

//! System information, read mostly straight from /proc and /sys (Linux).
//!
//! Each module (`ModuleDef`) is a function from the context (`Ctx`) to a
//! structured `Value`. `collect` runs the requested modules concurrently.

mod ctx;
mod desktop;
mod dev;
#[cfg(test)]
mod fixture;
mod hardware;
mod software;
pub mod value;

use std::thread;

pub use ctx::{Ctx, System};
#[cfg(test)]
pub use value::Gauge;
use value::{
    Batteries, Containers, Cpu, Git, Gpus, Kernel, Load, LocalIp, Name, Os, Packages, Resolution,
    Shell, Temp, Toolchains, Updates, Uptime, Usage, Windows,
};
pub use value::{Field, Gpu, GpuSource, Level, Meter, Report, Value};

/// How expensive a module is to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cost {
    /// Reads files only.
    Fast,
    /// Starts a program, or reads big files: a few milliseconds.
    Spawn,
    /// Over 50 ms; must be cached (per boot) or opt-in.
    Slow,
}

pub struct ModuleDef {
    /// The name in the config file and the JSON output.
    pub id: &'static str,
    /// The label shown before the value.
    pub label: &'static str,
    /// Documents the R1 budget; only the tests check it so far.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "nothing chooses modules by cost at runtime")
    )]
    pub cost: Cost,
    /// Whether it runs without being asked for.
    pub default_on: bool,
    /// The names `Report::field` answers to for its values, which
    /// `{id.field}` placeholders (`--format`) may use.
    pub fields: &'static [&'static str],
    /// `None` when it has nothing to report.
    pub run: fn(&Ctx) -> Option<Value>,
}

/// A module that runs by default.
const fn module(
    id: &'static str,
    label: &'static str,
    cost: Cost,
    fields: &'static [&'static str],
    run: fn(&Ctx) -> Option<Value>,
) -> ModuleDef {
    ModuleDef {
        id,
        label,
        cost,
        default_on: true,
        fields,
        run,
    }
}

/// A module that only runs when asked for (`--modules`).
const fn opt_in(
    id: &'static str,
    label: &'static str,
    cost: Cost,
    fields: &'static [&'static str],
    run: fn(&Ctx) -> Option<Value>,
) -> ModuleDef {
    ModuleDef {
        default_on: false,
        ..module(id, label, cost, fields, run)
    }
}

/// Every module: the default ones in display order, then the opt-in ones.
#[rustfmt::skip]
pub static MODULES: &[ModuleDef] = &[
    module("os", "OS", Cost::Fast, Os::FIELDS, software::os),
    // `wslinfo` on WSL, ~1 ms.
    module("host", "Host", Cost::Spawn, Name::FIELDS, hardware::host),
    // `cmd.exe`, cached per boot; only on WSL.
    module("windows", "Windows", Cost::Slow, Windows::FIELDS, software::windows),
    module("kernel", "Kernel", Cost::Fast, Kernel::FIELDS, software::kernel),
    module("uptime", "Uptime", Cost::Fast, Uptime::FIELDS, software::uptime),
    // The dpkg database is big, and `rpm -qa` is a program.
    module("packages", "Packages", Cost::Spawn, Packages::FIELDS, software::packages),
    module("shell", "Shell", Cost::Spawn, Shell::FIELDS, software::shell),
    module("resolution", "Resolution", Cost::Fast, Resolution::FIELDS, desktop::resolution),
    module("de", "DE", Cost::Fast, Name::FIELDS, desktop::de),
    module("wm", "WM", Cost::Fast, Name::FIELDS, desktop::wm),
    module("terminal", "Terminal", Cost::Fast, Name::FIELDS, desktop::terminal),
    module("cpu", "CPU", Cost::Fast, Cpu::FIELDS, hardware::cpu),
    module("temp", "CPU Temp", Cost::Fast, Temp::FIELDS, hardware::temp),
    module("load", "Load", Cost::Fast, Load::FIELDS, hardware::load),
    // `nvidia-smi` or PowerShell on WSL, cached per boot.
    module("gpu", "GPU", Cost::Slow, Gpus::FIELDS, hardware::gpu),
    module("memory", "Memory", Cost::Fast, Usage::FIELDS, hardware::memory),
    module("disk", "Disk (/)", Cost::Fast, Usage::FIELDS, hardware::disk),
    module("battery", "Battery", Cost::Fast, Batteries::FIELDS, hardware::battery),
    // `git status`, given 50 ms.
    module("git", "Git", Cost::Spawn, Git::FIELDS, dev::git),
    module("locale", "Locale", Cost::Fast, Name::FIELDS, software::locale),
    // rustc, node and python3, at once; 20-30 ms through rustup's proxy.
    opt_in("toolchains", "Toolchains", Cost::Spawn, Toolchains::FIELDS, dev::toolchains),
    opt_in("docker", "Containers", Cost::Fast, Containers::FIELDS, dev::docker),
    // Off by default: people post screenshots.
    opt_in("ip", "Local IP", Cost::Fast, LocalIp::FIELDS, hardware::ip),
    opt_in("updates", "Updates", Cost::Fast, Updates::FIELDS, software::updates),
];

/// The module with this id.
pub fn find(id: &str) -> Option<&'static ModuleDef> {
    MODULES.iter().find(|m| m.id == id)
}

/// The ids of the modules that run by default, in display order.
pub fn default_modules() -> Vec<&'static str> {
    MODULES
        .iter()
        .filter(|m| m.default_on)
        .map(|m| m.id)
        .collect()
}

/// The modules named in `list` ("os,load,git"), in its order, and the names in
/// it that aren't modules. Repeats and empty names are skipped.
pub fn parse_list(list: &str) -> (Vec<&'static str>, Vec<&str>) {
    let (mut ids, mut unknown) = (Vec::new(), Vec::new());
    for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        match find(name) {
            Some(def) if !ids.contains(&def.id) => ids.push(def.id),
            None if !unknown.contains(&name) => unknown.push(name),
            _ => {}
        }
    }
    (ids, unknown)
}

/// One line of the info column.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: &'static str,
    pub text: String,
    /// The usage bar before the text, for values that have one.
    pub meter: Option<Meter>,
}

/// What `collect` found.
pub struct Info {
    pub user: String,
    pub host: String,
    /// The modules that had something to report, in the order asked for.
    pub modules: Vec<(&'static ModuleDef, Value)>,
}

impl Info {
    /// The info column's lines in display order. A value with several lines
    /// (e.g. several GPUs) gives one row per line, each with the module's
    /// label; each battery gets its own row and bar.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (def, value) in &self.modules {
            for part in value.parts() {
                let meter = part.gauge().and_then(|gauge| match part.field("pct")? {
                    Field::Int(pct) => Some(Meter { gauge, pct }),
                    _ => None,
                });
                for (i, line) in part.display().lines().enumerate() {
                    rows.push(Row {
                        label: def.label,
                        text: line.to_string(),
                        meter: meter.filter(|_| i == 0),
                    });
                }
            }
        }
        rows
    }

    /// The value of the module `id`, if it ran and had one.
    pub fn get(&self, id: &str) -> Option<&Value> {
        self.modules
            .iter()
            .find(|(def, _)| def.id == id)
            .map(|(_, value)| value)
    }
}

/// Runs the modules with the given ids concurrently; a few of them start
/// programs or scan big files. Unknown ids are skipped. The facts computed on
/// the way are saved once all modules have finished.
pub fn collect(ctx: &Ctx, ids: &[&str]) -> Info {
    let defs: Vec<&'static ModuleDef> = ids.iter().filter_map(|id| find(id)).collect();
    let modules = thread::scope(|s| {
        let handles: Vec<_> = defs
            .into_iter()
            .map(|def| (def, s.spawn(move || (def.run)(ctx))))
            .collect();
        handles
            .into_iter()
            .filter_map(|(def, h)| Some((def, h.join().ok()??)))
            .collect()
    });
    // Failing to cache only costs the slow lookups again next run.
    let _ = ctx.facts.save();
    Info {
        user: ctx.user.name.clone(),
        host: ctx.uname.nodename.clone(),
        modules,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_order_matches_the_spec() {
        let ids = default_modules();
        assert_eq!(
            ids,
            [
                "os",
                "host",
                "windows",
                "kernel",
                "uptime",
                "packages",
                "shell",
                "resolution",
                "de",
                "wm",
                "terminal",
                "cpu",
                "temp",
                "load",
                "gpu",
                "memory",
                "disk",
                "battery",
                "git",
                "locale"
            ]
        );
        let labels: Vec<&str> = ids.iter().map(|id| find(id).unwrap().label).collect();
        assert_eq!(
            labels,
            [
                "OS",
                "Host",
                "Windows",
                "Kernel",
                "Uptime",
                "Packages",
                "Shell",
                "Resolution",
                "DE",
                "WM",
                "Terminal",
                "CPU",
                "CPU Temp",
                "Load",
                "GPU",
                "Memory",
                "Disk (/)",
                "Battery",
                "Git",
                "Locale"
            ]
        );
        assert!(find("nope").is_none());
    }

    #[test]
    fn opt_in_modules() {
        let opt_in: Vec<(&str, &str)> = MODULES
            .iter()
            .filter(|m| !m.default_on)
            .map(|m| (m.id, m.label))
            .collect();
        assert_eq!(
            opt_in,
            [
                ("toolchains", "Toolchains"),
                ("docker", "Containers"),
                ("ip", "Local IP"),
                ("updates", "Updates")
            ]
        );
        let ids: Vec<&str> = MODULES.iter().map(|m| m.id).collect();
        let unique: std::collections::BTreeSet<&str> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "ids are unique");
    }

    #[test]
    fn module_lists() {
        assert_eq!(
            parse_list("ip,os, load ,git"),
            (vec!["ip", "os", "load", "git"], vec![])
        );
        assert_eq!(
            parse_list("os,bogus,kernel,,nah,bogus,os"),
            (vec!["os", "kernel"], vec!["bogus", "nah"])
        );
        assert_eq!(parse_list(""), (vec![], vec![]));
    }

    #[test]
    fn slow_modules_are_cached_or_opt_in() {
        for m in MODULES.iter().filter(|m| m.cost == Cost::Slow) {
            assert!(
                ["windows", "gpu"].contains(&m.id) || !m.default_on,
                "{} is slow, on by default and not cached",
                m.id
            );
        }
    }
}

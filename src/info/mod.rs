//! System information, read mostly straight from /proc and /sys (Linux).
//!
//! Each module (`ModuleDef`) is a function from the context (`Ctx`) to a
//! structured `Value`. `collect` runs the requested modules concurrently.

mod ctx;
mod desktop;
#[cfg(test)]
mod fixture;
mod hardware;
mod software;
mod value;

use std::thread;

pub use ctx::{Ctx, System};
pub use value::{Report, Value};

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
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "for the config file and --format (M6)")
    )]
    pub cost: Cost,
    /// Whether it runs without being asked for.
    pub default_on: bool,
    /// `None` when it has nothing to report.
    pub run: fn(&Ctx) -> Option<Value>,
}

const fn module(
    id: &'static str,
    label: &'static str,
    cost: Cost,
    run: fn(&Ctx) -> Option<Value>,
) -> ModuleDef {
    ModuleDef {
        id,
        label,
        cost,
        default_on: true,
        run,
    }
}

/// Every module, in the default display order.
pub static MODULES: &[ModuleDef] = &[
    module("os", "OS", Cost::Fast, software::os),
    // `wslinfo` on WSL, ~1 ms.
    module("host", "Host", Cost::Spawn, hardware::host),
    // `cmd.exe`, cached per boot; only on WSL.
    module("windows", "Windows", Cost::Slow, software::windows),
    module("kernel", "Kernel", Cost::Fast, software::kernel),
    module("uptime", "Uptime", Cost::Fast, software::uptime),
    // The dpkg database is big, and `rpm -qa` is a program.
    module("packages", "Packages", Cost::Spawn, software::packages),
    module("shell", "Shell", Cost::Spawn, software::shell),
    module("resolution", "Resolution", Cost::Fast, desktop::resolution),
    module("de", "DE", Cost::Fast, desktop::de),
    module("wm", "WM", Cost::Fast, desktop::wm),
    module("terminal", "Terminal", Cost::Fast, desktop::terminal),
    module("cpu", "CPU", Cost::Fast, hardware::cpu),
    // `nvidia-smi` or PowerShell on WSL, cached per boot.
    module("gpu", "GPU", Cost::Slow, hardware::gpu),
    module("memory", "Memory", Cost::Fast, hardware::memory),
    module("disk", "Disk (/)", Cost::Fast, hardware::disk),
    module("battery", "Battery", Cost::Fast, hardware::battery),
    module("locale", "Locale", Cost::Fast, software::locale),
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

/// What `collect` found.
pub struct Info {
    pub user: String,
    pub host: String,
    /// The modules that had something to report, in the order asked for.
    pub modules: Vec<(&'static ModuleDef, Value)>,
}

impl Info {
    /// (label, line) pairs in display order. A value with several lines (e.g.
    /// several GPUs) gives one entry per line, each with the module's label.
    pub fn fields(&self) -> Vec<(&'static str, String)> {
        self.modules
            .iter()
            .flat_map(|(def, value)| {
                value
                    .display()
                    .lines()
                    .map(|l| (def.label, l.to_string()))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// The value of the module `id`, if it ran and had one.
    #[cfg_attr(not(test), expect(dead_code, reason = "for quips (M5) and M6"))]
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
    fn registry_keeps_todays_order() {
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
                "gpu",
                "memory",
                "disk",
                "battery",
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
                "GPU",
                "Memory",
                "Disk (/)",
                "Battery",
                "Locale"
            ]
        );
        assert!(find("nope").is_none());
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

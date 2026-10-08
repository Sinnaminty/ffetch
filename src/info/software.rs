//! Modules about the installed system: OS, Windows (on WSL), kernel, uptime,
//! packages, shell, locale and pending updates.

use std::{path::Path, process::Command};

use super::{
    Ctx,
    value::{Kernel, ManagerCount, Name, Os, Packages, Shell, Updates, Uptime, Value, Windows},
};
use crate::wsl;

pub fn os(ctx: &Ctx) -> Option<Value> {
    let release = ctx
        .read("/etc/os-release")
        .or_else(|| ctx.read("/usr/lib/os-release"));
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
    Some(Value::Os(Os {
        name,
        arch: ctx.uname.machine.clone(),
    }))
}

/// The Windows version under WSL. `cmd.exe` is slow, so it's cached per boot.
pub fn windows(ctx: &Ctx) -> Option<Value> {
    ctx.wsl?;
    let version = ctx.cached("windows", || wsl::windows(ctx))?;
    Some(Value::Windows(Windows::parse(&version)))
}

pub fn kernel(ctx: &Ctx) -> Option<Value> {
    Some(Value::Kernel(Kernel {
        release: ctx.uname.release.clone(),
    }))
}

pub fn uptime(ctx: &Ctx) -> Option<Value> {
    let seconds = ctx.read("/proc/uptime")?.split('.').next()?.parse().ok()?;
    Some(Value::Uptime(Uptime { seconds }))
}

pub fn packages(ctx: &Ctx) -> Option<Value> {
    let home = ctx.env("HOME").unwrap_or_default();
    let count_lines = |path: &str, pred: fn(&str) -> bool| {
        ctx.read_full(path)
            .map(|s| s.lines().filter(|l| pred(l)).count())
    };
    let counts = [
        (
            "dpkg",
            count_lines("/var/lib/dpkg/status", |l| {
                l.starts_with("Status: ") && l.ends_with(" installed")
            }),
        ),
        ("pacman", ctx.count_dirs("/var/lib/pacman/local", &[])),
        ("rpm", rpm_count(ctx)),
        (
            "apk",
            count_lines("/lib/apk/db/installed", |l| l.starts_with("P:")),
        ),
        (
            "emerge",
            Some(
                ctx.sorted_dir("/var/db/pkg")
                    .iter()
                    .filter_map(|c| ctx.count_dirs(c, &[]))
                    .sum(),
            ),
        ),
        (
            "brew",
            ctx.count_dirs("/home/linuxbrew/.linuxbrew/Cellar", &[]),
        ),
        (
            "flatpak",
            Some(
                ctx.count_dirs("/var/lib/flatpak/app", &[]).unwrap_or(0)
                    + ctx
                        .count_dirs(format!("{home}/.local/share/flatpak/app"), &[])
                        .unwrap_or(0),
            ),
        ),
        ("snap", ctx.count_dirs("/snap", &["bin"])),
    ];
    let managers: Vec<ManagerCount> = counts
        .into_iter()
        .filter_map(|(manager, n)| {
            let count = n.filter(|&n| n > 0)?;
            Some(ManagerCount { manager, count })
        })
        .collect();
    (!managers.is_empty()).then_some(Value::Packages(Packages { managers }))
}

fn rpm_count(ctx: &Ctx) -> Option<usize> {
    if !ctx.exists("/var/lib/rpm") && !ctx.exists("/usr/lib/sysimage/rpm") {
        return None;
    }
    let mut rpm = Command::new("rpm");
    rpm.args(["-qa", "--qf", ".\n"]);
    let out = ctx.run(rpm)?;
    Some(out.stdout.iter().filter(|&&b| b == b'\n').count())
}

pub fn shell(ctx: &Ctx) -> Option<Value> {
    let path = ctx.env_nonempty("SHELL")?;
    let name = Path::new(&path).file_name()?.to_string_lossy().into_owned();
    // Only ask shells known to answer `--version` sanely.
    if !matches!(name.as_str(), "bash" | "zsh" | "fish" | "nu" | "tcsh") {
        return Some(Value::Shell(Shell {
            name,
            version: None,
        }));
    }
    let mut cmd = Command::new(&path);
    cmd.arg("--version");
    let version = ctx.run(cmd).and_then(|o| {
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
    Some(Value::Shell(Shell { name, version }))
}

pub fn locale(ctx: &Ctx) -> Option<Value> {
    let locale = ctx
        .env_nonempty("LC_ALL")
        .or_else(|| ctx.env_nonempty("LANG"))?;
    Some(Value::Locale(Name::new(locale)))
}

/// Pending updates, as Ubuntu's update-notifier last counted them (for the
/// login message). Package managers are too slow to ask.
pub fn updates(ctx: &Ctx) -> Option<Value> {
    let text = ctx.read("/var/lib/update-notifier/updates-available")?;
    parse_updates(&text).map(Value::Updates)
}

/// Parses update-notifier's summary, e.g.
///
/// ```text
/// 15 updates can be applied immediately.
/// 5 of these updates are standard security updates.
/// ```
///
/// or the older "15 packages can be updated. / 5 updates are security
/// updates." The security count includes ESM updates that can be applied, but
/// not the "additional" ones that would need ESM enabled. `None` for text in
/// another form, e.g. another language.
fn parse_updates(text: &str) -> Option<Updates> {
    let (mut total, mut security) = (None, 0);
    for line in text.lines() {
        let line = line.trim();
        let digits = line.bytes().take_while(u8::is_ascii_digit).count();
        let Ok(n) = line[..digits].parse::<u64>() else {
            continue;
        };
        let rest = &line[digits..];
        if rest.contains("can be applied immediately") || rest.contains("can be updated") {
            total.get_or_insert(n);
        } else if rest.contains("security") && !rest.contains("additional") {
            security += n;
        }
    }
    Some(Updates {
        total: total?,
        security,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn updates(text: &str) -> Option<(u64, u64)> {
        parse_updates(text).map(|u| (u.total, u.security))
    }

    #[test]
    fn updates_available_plural_and_singular() {
        let plural = "\n15 updates can be applied immediately.\n\
            5 of these updates are standard security updates.\n\
            To see these additional updates run: apt list --upgradable\n\n";
        assert_eq!(updates(plural), Some((15, 5)));
        let singular = "1 update can be applied immediately.\n\
            1 of these updates is a standard security update.\n\
            To see these additional updates run: apt list --upgradable\n";
        assert_eq!(updates(singular), Some((1, 1)));
        assert_eq!(
            updates(
                "2 updates can be applied immediately.\nTo see these additional updates run: apt list --upgradable\n"
            ),
            Some((2, 0))
        );
    }

    #[test]
    fn updates_available_with_esm_and_none() {
        let esm = "Expanded Security Maintenance for Applications is enabled.\n\n\
            7 updates can be applied immediately.\n\
            2 of these updates are ESM Apps security updates.\n\
            1 of these updates is a standard security update.\n\
            To see these additional updates run: apt list --upgradable\n";
        assert_eq!(updates(esm), Some((7, 3)));
        let none = "Expanded Security Maintenance for Applications is not enabled.\n\n\
            0 updates can be applied immediately.\n\n\
            3 additional security updates can be applied with ESM Apps.\n\
            Learn more about enabling ESM Apps service at https://ubuntu.com/esm\n";
        assert_eq!(updates(none), Some((0, 0)));
    }

    #[test]
    fn updates_available_older_format_and_garbage() {
        let old = "15 packages can be updated.\n5 updates are security updates.\n";
        assert_eq!(updates(old), Some((15, 5)));
        let old_one = "1 package can be updated.\n1 update is a security update.\n";
        assert_eq!(updates(old_one), Some((1, 1)));
        assert_eq!(updates(""), None);
        assert_eq!(
            updates("15 Aktualisierungen können sofort angewendet werden.\n"),
            None
        );
    }
}

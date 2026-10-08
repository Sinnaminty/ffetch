//! A `System` read from a fixture directory instead of the machine, and the
//! fixture snapshot tests. The layout is described in `tests/fixtures/README.md`.

use std::{
    collections::BTreeMap,
    fs,
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Output},
    sync::{Arc, Mutex},
};

use super::ctx::{Entry, Statvfs, System, Uname, list_dir};

/// A canned program run: its argv (and working directory, if it matters) and
/// what it printed.
pub struct Canned {
    argv: Vec<String>,
    cwd: Option<PathBuf>,
    status: i32,
    stdout: Vec<u8>,
}

pub struct Fixture {
    root: PathBuf,
    pub env: BTreeMap<String, String>,
    uname: Uname,
    statvfs: BTreeMap<PathBuf, Statvfs>,
    pub commands: Vec<Canned>,
    pub ppid: u32,
    pub uid: u32,
    /// Every path read, in order (shared, so it can be checked after the
    /// fixture moves into a `Ctx`).
    pub reads: Arc<Mutex<Vec<PathBuf>>>,
}

/// The directory of the fixture `name`.
pub fn dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// `key=value` lines; blank lines and `#` comments are skipped.
fn pairs(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            l.split_once('=')
                .unwrap_or_else(|| panic!("expected key=value: {l:?}"))
        })
}

fn number<T: std::str::FromStr>(key: &str, value: &str) -> T {
    value
        .parse()
        .unwrap_or_else(|_| panic!("{key}: not a number: {value:?}"))
}

impl Fixture {
    pub fn load(name: &str) -> Fixture {
        let dir = dir(name);
        assert!(dir.is_dir(), "no fixture {}", dir.display());
        let file = |f: &str| fs::read_to_string(dir.join(f)).unwrap_or_default();

        let env = pairs(&file("env"))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        let mut uname = Uname::default();
        for (key, value) in pairs(&file("uname")) {
            let field = match key {
                "nodename" => &mut uname.nodename,
                "release" => &mut uname.release,
                "machine" => &mut uname.machine,
                _ => panic!("uname: unknown key {key:?}"),
            };
            *field = value.to_string();
        }

        let (mut ppid, mut uid) = (1, 1000);
        for (key, value) in pairs(&file("process")) {
            match key {
                "ppid" => ppid = number(key, value),
                "uid" => uid = number(key, value),
                _ => panic!("process: unknown key {key:?}"),
            }
        }

        let mut statvfs = BTreeMap::new();
        for line in file("statvfs").lines() {
            let mut words = line.split_whitespace();
            let Some(path) = words.next().filter(|w| !w.starts_with('#')) else {
                continue;
            };
            let mut st = Statvfs {
                frsize: 0,
                blocks: 0,
                bfree: 0,
                bavail: 0,
            };
            for (key, value) in words.map(|w| w.split_once('=').expect("statvfs: key=value")) {
                let field = match key {
                    "frsize" => &mut st.frsize,
                    "blocks" => &mut st.blocks,
                    "bfree" => &mut st.bfree,
                    "bavail" => &mut st.bavail,
                    _ => panic!("statvfs: unknown key {key:?}"),
                };
                *field = number(key, value);
            }
            statvfs.insert(PathBuf::from(path), st);
        }

        let mut commands = Vec::new();
        let mut files: Vec<PathBuf> = fs::read_dir(dir.join("commands"))
            .map(|d| d.map(|e| e.unwrap().path()).collect())
            .unwrap_or_default();
        files.sort();
        for path in files {
            commands.push(canned(&fs::read(&path).unwrap(), &path));
        }

        Fixture {
            root: dir.join("root"),
            env,
            uname,
            statvfs,
            commands,
            ppid,
            uid,
            reads: Arc::default(),
        }
    }

    /// Where the absolute `path` is in the fixture's tree.
    fn path(&self, path: &Path) -> PathBuf {
        self.root.join(path.strip_prefix("/").unwrap_or(path))
    }
}

/// Parses a canned command: header lines (`program:`, then `arg:` for each
/// argument, and optional `cwd:` and `status:`), a `---` line, then stdout.
fn canned(data: &[u8], path: &Path) -> Canned {
    let split = data
        .windows(5)
        .position(|w| w == b"\n---\n")
        .unwrap_or_else(|| panic!("{}: no --- line", path.display()));
    let header = std::str::from_utf8(&data[..split]).expect("UTF-8 header");
    let mut c = Canned {
        argv: Vec::new(),
        cwd: None,
        status: 0,
        stdout: data[split + 5..].to_vec(),
    };
    for line in header.lines().filter(|l| !l.starts_with('#')) {
        let (key, value) = line
            .split_once(':')
            .unwrap_or_else(|| panic!("{}: expected key: value", path.display()));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match key {
            "program" => c.argv.insert(0, value.to_string()),
            "arg" => c.argv.push(value.to_string()),
            "cwd" => c.cwd = Some(value.into()),
            "status" => c.status = number(key, value),
            _ => panic!("{}: unknown key {key:?}", path.display()),
        }
    }
    c
}

impl System for Fixture {
    fn read(&self, path: &Path) -> Option<String> {
        self.reads.lock().unwrap().push(path.to_path_buf());
        fs::read_to_string(self.path(path)).ok()
    }

    fn read_dir(&self, path: &Path) -> Option<Vec<Entry>> {
        list_dir(&self.path(path))
    }

    fn exists(&self, path: &Path) -> bool {
        self.path(path).exists()
    }

    fn env(&self, key: &str) -> Option<String> {
        self.env.get(key).cloned()
    }

    fn uname(&self) -> Uname {
        self.uname.clone()
    }

    fn statvfs(&self, path: &Path) -> Option<Statvfs> {
        self.statvfs.get(path).copied()
    }

    /// The canned output with the same argv (and working directory, if the
    /// canned one names it); like a missing program otherwise.
    fn run(&self, cmd: Command) -> Option<Output> {
        let argv: Vec<_> = [cmd.get_program()]
            .into_iter()
            .chain(cmd.get_args())
            .collect();
        let c = self.commands.iter().find(|c| {
            c.argv
                .iter()
                .map(String::as_str)
                .eq(argv.iter().map(|a| a.to_str().unwrap()))
                && c.cwd
                    .as_deref()
                    .is_none_or(|cwd| cmd.get_current_dir() == Some(cwd))
        })?;
        Some(Output {
            status: ExitStatus::from_raw(c.status << 8),
            stdout: c.stdout.clone(),
            stderr: Vec::new(),
        })
    }

    fn ppid(&self) -> u32 {
        self.ppid
    }

    fn uid(&self) -> u32 {
        self.uid
    }

    /// Looked up in the fixture's `/etc/passwd`.
    fn user_name(&self, uid: u32) -> Option<String> {
        let passwd = fs::read_to_string(self.path(Path::new("/etc/passwd"))).ok()?;
        passwd.lines().find_map(|l| {
            let mut fields = l.split(':');
            let name = fields.next()?;
            (fields.nth(1)?.parse() == Ok(uid)).then(|| name.to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{env, process};

    use super::*;
    use crate::{
        info::{Ctx, Info, Report, Value, collect, default_modules, value::Field},
        palette::Roles,
        term::{ColorMode, Rgb},
    };

    fn ctx(fixture: Fixture) -> Ctx {
        Ctx::new(Box::new(fixture), None, false)
    }

    fn fetch(name: &str) -> Info {
        collect(&ctx(Fixture::load(name)), &default_modules())
    }

    /// The info column as `--no-color` prints it.
    fn plain_text(info: &Info) -> String {
        let gray = Rgb(128, 128, 128);
        let roles = Roles {
            accent: gray,
            secondary: gray,
            muted: gray,
        };
        let lines = crate::info_lines(info, &info.fields(), ColorMode::None, roles, &[], None);
        lines.join("\n") + "\n"
    }

    /// Compares `actual` with the fixture's `expected.txt`, or rewrites that
    /// file when `UPDATE_SNAPSHOTS=1`.
    fn assert_snapshot(name: &str, actual: &str) {
        let path = dir(name).join("expected.txt");
        if env::var("UPDATE_SNAPSHOTS").is_ok_and(|v| v == "1") {
            fs::write(&path, actual).unwrap();
            return;
        }
        let expected = fs::read_to_string(&path).unwrap_or_default();
        if actual == expected {
            return;
        }
        let (exp, act): (Vec<_>, Vec<_>) = (expected.lines().collect(), actual.lines().collect());
        let mut diff = String::new();
        for i in 0..exp.len().max(act.len()) {
            match (exp.get(i), act.get(i)) {
                (e, a) if e == a => diff += &format!("  {}\n", e.unwrap()),
                (e, a) => {
                    e.inspect(|e| diff += &format!("- {e}\n"));
                    a.inspect(|a| diff += &format!("+ {a}\n"));
                }
            }
        }
        panic!(
            "{} doesn't match (- expected, + actual); rerun with UPDATE_SNAPSHOTS=1 to accept:\n{diff}",
            path.display()
        );
    }

    fn field(info: &Info, id: &str, name: &str) -> Option<Field> {
        info.get(id)?.field(name)
    }

    fn int(n: u64) -> Option<Field> {
        Some(Field::Int(n))
    }

    fn text(s: &str) -> Option<Field> {
        Some(Field::Text(s.into()))
    }

    #[test]
    fn wsl2_snapshot() {
        assert_snapshot("wsl2", &plain_text(&fetch("wsl2")));
    }

    #[test]
    fn arch_desktop_snapshot() {
        assert_snapshot("arch-desktop", &plain_text(&fetch("arch-desktop")));
    }

    #[test]
    fn wsl2_fields() {
        let info = fetch("wsl2");
        assert_eq!((info.user.as_str(), info.host.as_str()), ("dev", "DEVBOX"));
        assert_eq!(field(&info, "os", "name"), text("Ubuntu 24.04.5 LTS"));
        assert_eq!(field(&info, "os", "arch"), text("x86_64"));
        assert_eq!(field(&info, "windows", "build"), int(26300));
        assert_eq!(field(&info, "uptime", "seconds"), int(21919));
        assert_eq!(field(&info, "uptime", "days"), int(0));
        assert_eq!(field(&info, "packages", "total"), int(6));
        assert_eq!(field(&info, "packages", "dpkg"), int(6));
        assert_eq!(field(&info, "packages", "pacman"), None);
        assert_eq!(field(&info, "shell", "version"), text("5.9"));
        assert_eq!(
            field(&info, "cpu", "name"),
            text("11th Gen Intel Core i7-11850H")
        );
        assert_eq!(field(&info, "cpu", "threads"), int(16));
        assert_eq!(field(&info, "cpu", "ghz"), Some(Field::Float(2.5)));
        assert_eq!(field(&info, "gpu", "name"), text("NVIDIA T1200 Laptop GPU"));
        // MemTotal 16234672 kB; used = total + Shmem - (MemFree + Buffers + Cached + SReclaimable).
        assert_eq!(field(&info, "memory", "total_bytes"), int(16234672 * 1024));
        assert_eq!(field(&info, "memory", "used_bytes"), int(2712428 * 1024));
        assert_eq!(field(&info, "memory", "pct"), int(16));
        assert_eq!(field(&info, "disk", "total_bytes"), int(263940717 * 4096));
        assert_eq!(field(&info, "disk", "pct"), int(8));
        assert_eq!(field(&info, "battery", "pct"), int(100));
        assert_eq!(field(&info, "battery", "status"), text("Full"));
        assert_eq!(field(&info, "locale", "name"), text("C.UTF-8"));
    }

    #[test]
    fn arch_desktop_fields() {
        let info = fetch("arch-desktop");
        let ids: Vec<&str> = info.modules.iter().map(|(def, _)| def.id).collect();
        assert!(
            !ids.contains(&"windows") && !ids.contains(&"battery"),
            "{ids:?}"
        );
        assert_eq!(field(&info, "packages", "pacman"), int(7));
        assert_eq!(field(&info, "packages", "flatpak"), int(3));
        assert_eq!(field(&info, "packages", "total"), int(10));
        assert_eq!(field(&info, "cpu", "threads"), int(32));
        assert_eq!(field(&info, "cpu", "ghz"), Some(Field::Float(5.881)));
        assert_eq!(field(&info, "gpu", "count"), int(2));
        assert_eq!(field(&info, "resolution", "count"), int(2));
        assert_eq!(field(&info, "memory", "pct"), int(16));
        assert_eq!(field(&info, "disk", "pct"), int(40));
        assert_eq!(field(&info, "wm", "name"), text("KWin"));
        assert_eq!(field(&info, "terminal", "name"), text("kitty"));
        assert_eq!(field(&info, "uptime", "days"), int(11));
        let Some(Value::Gpu(gpus)) = info.get("gpu") else {
            panic!("no GPU");
        };
        assert_eq!(gpus.names, ["NVIDIA GeForce RTX 4090", "AMD Raphael"]);
    }

    #[test]
    fn wsl2_without_interop_falls_back() {
        let mut fixture = Fixture::load("wsl2");
        fixture.commands.clear();
        let info = collect(&ctx(fixture), &default_modules());
        let fields = info.fields();
        let get = |label| {
            fields
                .iter()
                .find(|(l, _)| *l == label)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("Host"), Some("Windows Subsystem for Linux (WSL2)"));
        assert_eq!(get("Windows"), None);
        assert_eq!(get("GPU"), Some("Microsoft Basic Render Driver"));
        assert_eq!(get("Shell"), Some("zsh"));
    }

    #[test]
    fn cached_facts_are_used_for_this_boot() {
        let dir = env::temp_dir().join(format!("ffetch-test-{}-fixture-facts", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("facts.json");
        let boot_id =
            fs::read_to_string(super::dir("wsl2").join("root/proc/sys/kernel/random/boot_id"))
                .unwrap();
        fs::write(
            &file,
            format!(
                r#"{{"boot_id": "{}", "facts": {{"windows": "Windows 99 (build 1)", "gpu": null}}}}"#,
                boot_id.trim()
            ),
        )
        .unwrap();
        let ctx = Ctx::new(Box::new(Fixture::load("wsl2")), Some(file.clone()), false);
        let info = collect(&ctx, &["windows", "gpu"]);
        let fields = info.fields();
        assert_eq!(fields[0], ("Windows", "Windows 99 (build 1)".to_string()));
        // A cached "nothing found" falls back to the PCI scan without asking again.
        assert_eq!(
            fields[1],
            ("GPU", "Microsoft Basic Render Driver".to_string())
        );

        // With --refresh, they're looked up (in the fixture) again and saved.
        let ctx = Ctx::new(Box::new(Fixture::load("wsl2")), Some(file.clone()), true);
        collect(&ctx, &["windows", "gpu"]);
        let saved = fs::read_to_string(&file).unwrap();
        assert!(
            saved.contains("\"windows\": \"Windows 11 (build 26300)\""),
            "{saved}"
        );
        assert!(
            saved.contains("\"gpu\": \"NVIDIA T1200 Laptop GPU\""),
            "{saved}"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_the_requested_modules_run_in_the_order_asked() {
        let info = collect(
            &ctx(Fixture::load("arch-desktop")),
            &["memory", "kernel", "no-such-module", "battery", "os"],
        );
        let ids: Vec<&str> = info.modules.iter().map(|(def, _)| def.id).collect();
        assert_eq!(ids, ["memory", "kernel", "os"], "no battery on this one");
    }

    #[test]
    fn shared_files_are_read_once() {
        let fixture = Fixture::load("arch-desktop");
        let reads = Arc::clone(&fixture.reads);
        let ctx = ctx(fixture);
        collect(&ctx, &["memory", "cpu", "memory", "cpu"]);
        assert!(ctx.meminfo().is_some() && ctx.cpuinfo().is_some());
        let reads = reads.lock().unwrap();
        for file in ["/proc/meminfo", "/proc/cpuinfo"] {
            let n = reads.iter().filter(|p| p == &Path::new(file)).count();
            assert_eq!(n, 1, "{file} read {n} times");
        }
    }

    #[test]
    fn user_comes_from_passwd_without_user_variable() {
        let mut fixture = Fixture::load("arch-desktop");
        fixture.env.remove("USER");
        let ctx = ctx(fixture);
        assert_eq!(ctx.user.name, "ada");
        assert!(!ctx.user.is_root());

        let mut fixture = Fixture::load("arch-desktop");
        fixture.env.remove("USER");
        fixture.uid = 0;
        let ctx = self::ctx(fixture);
        assert_eq!(ctx.user.name, "root");
        assert!(ctx.user.is_root());

        let mut fixture = Fixture::load("arch-desktop");
        fixture.env.remove("USER");
        fixture.uid = 4242;
        assert_eq!(self::ctx(fixture).user.name, "user", "not in passwd");
    }

    #[test]
    fn terminal_walk_skips_shells() {
        // Started straight from fish this time: one shell fewer to skip.
        let mut fixture = Fixture::load("arch-desktop");
        fixture.ppid = 2150;
        let info = collect(&ctx(fixture), &["terminal"]);
        assert_eq!(info.fields(), [("Terminal", "kitty".to_string())]);

        // No parent to walk (e.g. started by init): $TERM is all there is.
        let mut fixture = Fixture::load("arch-desktop");
        fixture.ppid = 1;
        let info = collect(&ctx(fixture), &["terminal"]);
        assert_eq!(info.fields(), [("Terminal", "xterm-kitty".to_string())]);
    }
}

//! A `System` read from a fixture directory instead of the machine, and the
//! fixture snapshot tests. The layout is described in `tests/fixtures/README.md`.

use std::{
    collections::BTreeMap,
    env, fs,
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{self, Command, ExitStatus, Output},
    sync::{Arc, Mutex},
    time::Duration,
};

use super::ctx::{Ctx, Entry, IfAddr, Statvfs, System, Uname, list_dir};

/// A canned program run: its argv (and working directory, if it matters) and
/// what it printed.
pub struct Canned {
    argv: Vec<String>,
    cwd: Option<PathBuf>,
    status: i32,
    stdout: Vec<u8>,
}

/// A canned exchange over a Unix socket: the request's first line and the
/// reply.
pub struct CannedSocket {
    path: PathBuf,
    request: String,
    reply: Vec<u8>,
}

pub struct Fixture {
    root: PathBuf,
    pub env: BTreeMap<String, String>,
    uname: Uname,
    statvfs: BTreeMap<PathBuf, Statvfs>,
    pub commands: Vec<Canned>,
    pub sockets: Vec<CannedSocket>,
    pub interfaces: Vec<IfAddr>,
    pub cwd: Option<PathBuf>,
    pub ppid: u32,
    pub uid: u32,
    /// The hour of the local time.
    pub hour: Option<u8>,
    /// Every path read, in order (shared, so it can be checked after the
    /// fixture moves into a `Ctx`).
    pub reads: Arc<Mutex<Vec<PathBuf>>>,
    /// Every program run (or tried), in order, shared like `reads`.
    pub runs: Arc<Mutex<Vec<String>>>,
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
    /// A machine with the filesystem at `root` and nothing else: no
    /// environment, programs, sockets or network.
    pub fn bare(root: PathBuf) -> Fixture {
        Fixture {
            root,
            env: BTreeMap::new(),
            uname: Uname::default(),
            statvfs: BTreeMap::new(),
            commands: Vec::new(),
            sockets: Vec::new(),
            interfaces: Vec::new(),
            cwd: None,
            ppid: 1,
            uid: 1000,
            hour: None,
            reads: Arc::default(),
            runs: Arc::default(),
        }
    }

    pub fn load(name: &str) -> Fixture {
        let dir = dir(name);
        assert!(dir.is_dir(), "no fixture {}", dir.display());
        let mut fixture = Fixture::bare(dir.join("root"));
        let file = |f: &str| fs::read_to_string(dir.join(f)).unwrap_or_default();

        fixture.env = pairs(&file("env"))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        for (key, value) in pairs(&file("uname")) {
            let field = match key {
                "nodename" => &mut fixture.uname.nodename,
                "release" => &mut fixture.uname.release,
                "machine" => &mut fixture.uname.machine,
                _ => panic!("uname: unknown key {key:?}"),
            };
            *field = value.to_string();
        }

        for (key, value) in pairs(&file("process")) {
            match key {
                "ppid" => fixture.ppid = number(key, value),
                "uid" => fixture.uid = number(key, value),
                "cwd" => fixture.cwd = Some(value.into()),
                _ => panic!("process: unknown key {key:?}"),
            }
        }

        for (key, value) in pairs(&file("clock")) {
            match key {
                "hour" => fixture.hour = Some(number(key, value)),
                _ => panic!("clock: unknown key {key:?}"),
            }
        }

        for line in file("interfaces").lines() {
            let words: Vec<&str> = line.split_whitespace().collect();
            let &[name, addr, flags] = words.as_slice() else {
                assert!(
                    words.is_empty() || words[0].starts_with('#'),
                    "interfaces: {line:?}"
                );
                continue;
            };
            let flags: Vec<&str> = flags.split(',').collect();
            for flag in &flags {
                assert!(
                    ["up", "running", "loopback", "-"].contains(flag),
                    "interfaces: {flag:?}"
                );
            }
            fixture.interfaces.push(IfAddr {
                name: name.into(),
                addr: addr
                    .parse()
                    .unwrap_or_else(|_| panic!("interfaces: {addr:?}")),
                up: flags.contains(&"up"),
                running: flags.contains(&"running"),
                loopback: flags.contains(&"loopback"),
            });
        }

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
            fixture.statvfs.insert(PathBuf::from(path), st);
        }

        let files = |sub: &str| {
            let mut files: Vec<PathBuf> = fs::read_dir(dir.join(sub))
                .map(|d| d.map(|e| e.unwrap().path()).collect())
                .unwrap_or_default();
            files.sort();
            files
        };
        for path in files("commands") {
            fixture
                .commands
                .push(canned(&fs::read(&path).unwrap(), &path));
        }
        for path in files("sockets") {
            fixture
                .sockets
                .push(canned_socket(&fs::read(&path).unwrap(), &path));
        }
        fixture
    }

    /// Where the absolute `path` is in the fixture's tree.
    fn path(&self, path: &Path) -> PathBuf {
        self.root.join(path.strip_prefix("/").unwrap_or(path))
    }
}

/// Splits a canned file into its header, as `key: value` pairs (`#` comment
/// lines skipped), and the bytes after the `---` line.
fn canned_parts<'a>(data: &'a [u8], path: &Path) -> (Vec<(&'a str, &'a str)>, &'a [u8]) {
    let split = data
        .windows(5)
        .position(|w| w == b"\n---\n")
        .unwrap_or_else(|| panic!("{}: no --- line", path.display()));
    let header = std::str::from_utf8(&data[..split]).expect("UTF-8 header");
    let pairs = header
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(|line| {
            let (key, value) = line
                .split_once(':')
                .unwrap_or_else(|| panic!("{}: expected key: value", path.display()));
            (key, value.strip_prefix(' ').unwrap_or(value))
        })
        .collect();
    (pairs, &data[split + 5..])
}

/// Parses a canned command: header lines (`program:`, then `arg:` for each
/// argument, and optional `cwd:` and `status:`), a `---` line, then stdout.
fn canned(data: &[u8], path: &Path) -> Canned {
    let (header, stdout) = canned_parts(data, path);
    let mut c = Canned {
        argv: Vec::new(),
        cwd: None,
        status: 0,
        stdout: stdout.to_vec(),
    };
    for (key, value) in header {
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

/// Parses a canned socket exchange: `path:` and `request:` (the request's first
/// line) header lines, a `---` line, then the reply.
fn canned_socket(data: &[u8], path: &Path) -> CannedSocket {
    let (header, reply) = canned_parts(data, path);
    let get = |key: &str| {
        let (_, value) = header
            .iter()
            .find(|(k, _)| *k == key)
            .unwrap_or_else(|| panic!("{}: no {key}:", path.display()));
        value.to_string()
    };
    CannedSocket {
        path: get("path").into(),
        request: get("request"),
        reply: reply.to_vec(),
    }
}

/// A directory tree written for one test, and deleted when dropped.
pub struct Tree {
    root: PathBuf,
}

impl Tree {
    /// Writes `files` ((absolute path, contents) pairs) under a fresh directory
    /// named after the test.
    pub fn new(test: &str, files: &[(&str, &str)]) -> Tree {
        let root = env::temp_dir().join(format!("ffetch-test-{}-{test}", process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        for (path, contents) in files {
            let path = root.join(path.strip_prefix('/').unwrap_or(path));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        Tree { root }
    }

    /// A context for a machine with only these files (see `Fixture::bare`).
    pub fn ctx(&self) -> Ctx {
        Ctx::new(Box::new(Fixture::bare(self.root.clone())), None, false)
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
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

    fn cwd(&self) -> Option<PathBuf> {
        self.cwd.clone()
    }

    fn uname(&self) -> Uname {
        self.uname.clone()
    }

    fn statvfs(&self, path: &Path) -> Option<Statvfs> {
        self.statvfs.get(path).copied()
    }

    fn interfaces(&self) -> Vec<IfAddr> {
        self.interfaces.clone()
    }

    /// The canned output with the same argv (and working directory, if the
    /// canned one names it); like a missing program otherwise. Canned programs
    /// never time out.
    fn run(&self, cmd: Command, _timeout: Duration) -> Option<Output> {
        let program = cmd.get_program().to_string_lossy().into_owned();
        self.runs.lock().unwrap().push(program);
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

    /// The canned reply for this socket and request line; like a missing
    /// socket otherwise.
    fn unix_request(&self, path: &Path, request: &[u8], _timeout: Duration) -> Option<Vec<u8>> {
        let line = request.split(|&b| b == b'\r' || b == b'\n').next()?;
        let canned = self
            .sockets
            .iter()
            .find(|s| s.path == path && s.request.as_bytes() == line)?;
        Some(canned.reply.clone())
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

    fn local_hour(&self) -> Option<u8> {
        self.hour
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        info::{Info, MODULES, Report, Value, collect, default_modules, value::Field},
        layout::{self, Layout, Options},
        logo::LogoImage,
        output::{
            format::{self, Template},
            json,
        },
        palette::{self, Roles, Theme},
        quip::{self, State},
        term::{ColorMode, Rgb},
    };

    fn ctx(fixture: Fixture) -> Ctx {
        Ctx::new(Box::new(fixture), None, false)
    }

    fn fetch(name: &str) -> Info {
        collect(&ctx(Fixture::load(name)), &default_modules())
    }

    /// Picks the quip in snapshots, so that they don't change between runs:
    /// the first of three templates.
    const QUIP_SEED: u64 = 3;

    /// The info column of the fixture `name` as `--no-color` prints it
    /// without a logo.
    fn plain_text(name: &str) -> String {
        let ctx = ctx(Fixture::load(name));
        let info = collect(&ctx, &default_modules());
        let quip = quip::pick(&quip::built_in(), &State::new(&info, &ctx), QUIP_SEED);
        let gray = Rgb(128, 128, 128);
        let opts = Options {
            logo: None,
            size: None,
            layout: Layout::Auto,
            bars: true,
            mode: ColorMode::None,
            roles: Roles {
                accent: gray,
                secondary: gray,
                muted: gray,
            },
            swatches: Vec::new(),
            quip,
        };
        let lines = layout::info_lines(&info, &info.rows(), &opts, None, false);
        lines.join("\n") + "\n"
    }

    /// (label, text) for each line of the info column.
    fn lines(info: &Info) -> Vec<(&'static str, String)> {
        info.rows().into_iter().map(|r| (r.label, r.text)).collect()
    }

    /// Compares `actual` with the fixture's `file` (`expected.txt` or
    /// `expected.json`), or rewrites that file when `UPDATE_SNAPSHOTS=1`.
    fn assert_snapshot(name: &str, file: &str, actual: &str) {
        let path = dir(name).join(file);
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
        assert_snapshot("wsl2", "expected.txt", &plain_text("wsl2"));
    }

    #[test]
    fn arch_desktop_snapshot() {
        assert_snapshot("arch-desktop", "expected.txt", &plain_text("arch-desktop"));
    }

    /// What `--json` prints for the fixture `name`: the default modules, and
    /// the built-in logo's palette with the default theme.
    fn json_text(name: &str) -> String {
        let logo = LogoImage::embedded();
        let palette = palette::extract(&logo.pixels, logo.background);
        json::render(&fetch(name), &palette, Theme::default().roles(&palette))
    }

    #[test]
    fn wsl2_json_snapshot() {
        assert_snapshot("wsl2", "expected.json", &json_text("wsl2"));
    }

    #[test]
    fn arch_desktop_json_snapshot() {
        assert_snapshot("arch-desktop", "expected.json", &json_text("arch-desktop"));
    }

    #[test]
    fn json_document() {
        for (name, absent) in [
            ("wsl2", ["temp", "resolution", "de", "wm"]),
            ("arch-desktop", ["windows", "battery", "toolchains", "ip"]),
        ] {
            let text = json_text(name);
            let json: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(json["schema"], 1);
            let modules = json["modules"].as_object().unwrap();
            for id in absent {
                assert!(!modules.contains_key(id), "{name}: {id}");
            }
            assert!(!text.contains("null"), "{name}: nothing is null");
            // In the default order, the ones with nothing to report left out.
            let at = |id: &str| text.find(&format!("\n    \"{id}\": "));
            let order: Vec<&str> = default_modules()
                .into_iter()
                .filter(|id| at(id).is_some())
                .collect();
            let mut by_position = order.clone();
            by_position.sort_by_key(|id| at(id));
            assert_eq!(order, by_position, "{name}");
            assert_eq!(order.len(), modules.len(), "{name}");

            let palette = json["palette"].as_object().unwrap();
            let hex = |v: &serde_json::Value| {
                let s = v.as_str().unwrap();
                assert!(
                    s.len() == 7
                        && s.starts_with('#')
                        && s[1..]
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                    "{s:?}"
                );
            };
            for role in ["accent", "secondary", "muted"] {
                hex(&palette[role]);
            }
            let colors = palette["colors"].as_array().unwrap();
            assert!((1..=6).contains(&colors.len()));
            colors.iter().for_each(hex);
            assert_eq!(palette["accent"], "#c33e58", "the logo's background");
        }
    }

    #[test]
    fn json_values_of_the_opt_in_modules() {
        let ctx = ctx(Fixture::load("arch-desktop"));
        let info = collect(&ctx, &["ip", "toolchains", "docker", "git"]);
        let json: serde_json::Value = serde_json::from_str(&json::render(
            &info,
            &palette_for_tests(),
            roles_for_tests(),
        ))
        .unwrap();
        assert_eq!(
            json["modules"],
            serde_json::json!({
                "ip": {"address": "192.168.1.20", "interface": "enp5s0"},
                "toolchains": {"rust": "1.98.1", "node": "22.11.0", "python": "3.12.7"},
                "docker": {"running": 3},
                "git": {"branch": "main", "ahead": 1, "behind": 0, "changed": 3},
            })
        );
    }

    fn palette_for_tests() -> palette::Palette {
        let logo = LogoImage::embedded();
        palette::extract(&logo.pixels, logo.background)
    }

    fn roles_for_tests() -> Roles {
        Theme::default().roles(&palette_for_tests())
    }

    /// The `--format` line for `template` on the fixture `fixture`.
    fn line(fixture: Fixture, template: &str) -> String {
        let template: Template = template.parse().unwrap();
        template.line(&ctx(fixture))
    }

    #[test]
    fn format_lines() {
        assert_eq!(
            line(Fixture::load("wsl2"), format::ONELINE),
            "Ubuntu 24.04.5 LTS x86_64 · up 6 hours, 5 mins · mem 16% · disk 8%"
        );
        let arch = || Fixture::load("arch-desktop");
        // Several lines are joined with commas.
        assert_eq!(
            line(arch(), "{gpu} ({gpu.count})"),
            "NVIDIA GeForce RTX 4090, AMD Raphael (2)"
        );
        assert_eq!(
            line(arch(), "{cpu.ghz}GHz, load {load.load1}, {{{git.branch}}}"),
            "5.88GHz, load 2.31, {main}"
        );
        // Nothing to report: empty, as are fields the value doesn't have.
        assert_eq!(
            line(
                arch(),
                "[{battery}|{battery.pct}|{windows.build}|{os.name}]"
            ),
            "[|||Arch Linux]"
        );
        let mut detached = Fixture::load("wsl2");
        detached.cwd = Some("/elsewhere".into());
        assert_eq!(line(detached, "git:{git.branch}:{git.ahead}"), "git::");
    }

    #[test]
    fn format_runs_only_the_modules_it_shows() {
        let fixture = Fixture::load("wsl2");
        let (reads, runs) = (Arc::clone(&fixture.reads), Arc::clone(&fixture.runs));
        assert_eq!(line(fixture, "mem {memory.pct}%"), "mem 16%");
        let reads = reads.lock().unwrap();
        assert!(reads.contains(&PathBuf::from("/proc/meminfo")), "{reads:?}");
        // Besides the context's own (boot ID, WSL detection), only memory's.
        let others: Vec<&PathBuf> = reads
            .iter()
            .filter(|p| {
                ![
                    "/proc/meminfo",
                    "/proc/sys/kernel/random/boot_id",
                    "/proc/sys/kernel/osrelease",
                ]
                .contains(&p.to_str().unwrap())
            })
            .collect();
        assert!(others.is_empty(), "{others:?}");
        assert!(
            runs.lock().unwrap().is_empty(),
            "{:?}",
            runs.lock().unwrap()
        );

        // The full default run, for comparison, starts programs.
        let fixture = Fixture::load("wsl2");
        let runs = Arc::clone(&fixture.runs);
        collect(&ctx(fixture), &default_modules());
        let runs = runs.lock().unwrap();
        for program in ["git", "wslinfo", "cmd.exe"] {
            assert!(runs.iter().any(|r| r == program), "{program}: {runs:?}");
        }
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
        assert_eq!(field(&info, "gpu", "name"), text("T1200 Laptop GPU"));
        assert_eq!(field(&info, "gpu", "vendor"), text("NVIDIA"));
        assert_eq!(field(&info, "gpu", "source"), text("nvidia-smi"));
        // MemTotal 16234672 kB; used = total + Shmem - (MemFree + Buffers + Cached + SReclaimable).
        assert_eq!(field(&info, "memory", "total_bytes"), int(16234672 * 1024));
        assert_eq!(field(&info, "memory", "used_bytes"), int(2712428 * 1024));
        assert_eq!(field(&info, "memory", "pct"), int(16));
        assert_eq!(field(&info, "disk", "total_bytes"), int(263940717 * 4096));
        assert_eq!(field(&info, "disk", "pct"), int(8));
        assert_eq!(field(&info, "battery", "pct"), int(100));
        assert_eq!(field(&info, "battery", "status"), text("Full"));
        assert_eq!(field(&info, "locale", "name"), text("C.UTF-8"));
        assert_eq!(field(&info, "load", "load1"), Some(Field::Float(0.14)));
        assert_eq!(field(&info, "load", "load15"), Some(Field::Float(0.08)));
        assert_eq!(field(&info, "load", "threads"), int(16));
        assert!(info.get("temp").is_none(), "WSL has no CPU sensors");
        assert_eq!(field(&info, "git", "branch"), text("main"));
        assert_eq!(field(&info, "git", "ahead"), int(0));
        assert_eq!(field(&info, "git", "behind"), int(0));
        assert_eq!(field(&info, "git", "changed"), int(0));
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
        let names: Vec<String> = gpus.gpus.iter().map(|g| g.full_name()).collect();
        assert_eq!(names, ["NVIDIA GeForce RTX 4090", "AMD Raphael"]);
        assert_eq!(field(&info, "gpu", "source"), text("pci"));
        assert_eq!(field(&info, "temp", "celsius"), Some(Field::Float(54.125)));
        assert_eq!(field(&info, "load", "load1"), Some(Field::Float(2.31)));
        assert_eq!(field(&info, "load", "threads"), int(32));
        assert_eq!(field(&info, "git", "ahead"), int(1));
        assert_eq!(field(&info, "git", "behind"), int(0));
        assert_eq!(field(&info, "git", "changed"), int(3));
    }

    fn quip_state(fixture: Fixture, ids: &[&str]) -> State {
        let ctx = ctx(fixture);
        State::new(&collect(&ctx, ids), &ctx)
    }

    #[test]
    fn quip_states() {
        let wsl2 = quip_state(Fixture::load("wsl2"), &default_modules());
        assert_eq!(
            wsl2,
            State {
                battery: Some(100),
                battery_discharging: Some(false),
                mem_pct: Some(16),
                disk_pct: Some(8),
                load1: Some(0.14),
                threads: Some(16),
                uptime_days: Some(0),
                hour: Some(2),
                root: Some(false),
                packages: Some(6),
            }
        );
        let arch = quip_state(Fixture::load("arch-desktop"), &default_modules());
        assert_eq!(
            arch,
            State {
                battery: None,
                battery_discharging: None,
                mem_pct: Some(16),
                disk_pct: Some(40),
                load1: Some(2.31),
                threads: Some(32),
                uptime_days: Some(11),
                hour: Some(14),
                root: Some(false),
                packages: Some(10),
            }
        );

        // Root at noon: nothing else to remark on.
        let mut fixture = Fixture::load("wsl2");
        fixture.uid = 0;
        fixture.hour = Some(12);
        let root = quip_state(fixture, &default_modules());
        assert_eq!((root.root, root.hour), (Some(true), Some(12)));
        let quip = quip::pick(&quip::built_in(), &root, QUIP_SEED).unwrap();
        assert!(quip.contains("root"), "{quip}");

        // Modules that didn't run leave their names undefined. Without the
        // load module, the thread count comes from the CPU.
        let mut fixture = Fixture::load("wsl2");
        fixture.hour = None;
        assert_eq!(
            quip_state(fixture, &["os", "cpu"]),
            State {
                threads: Some(16),
                root: Some(false),
                ..State::default()
            }
        );
    }

    #[test]
    fn a_discharging_battery() {
        let tree = Tree::new(
            "discharging",
            &[
                ("/sys/class/power_supply/BAT0/type", "Battery\n"),
                ("/sys/class/power_supply/BAT0/capacity", "9\n"),
                ("/sys/class/power_supply/BAT0/status", "Discharging\n"),
            ],
        );
        let ctx = tree.ctx();
        let state = State::new(&collect(&ctx, &["battery"]), &ctx);
        assert_eq!(
            (state.battery, state.battery_discharging),
            (Some(9), Some(true))
        );
        assert_eq!(state.hour, None, "no clock in a bare fixture");
        let quip = quip::pick(&quip::built_in(), &state, QUIP_SEED).unwrap();
        assert_eq!(quip, "9% battery. living dangerously.");
    }

    const OPT_IN: [&str; 4] = ["toolchains", "docker", "ip", "updates"];

    #[test]
    fn opt_in_modules_on_wsl2() {
        let info = collect(&ctx(Fixture::load("wsl2")), &OPT_IN);
        assert_eq!(
            lines(&info),
            [
                ("Toolchains", "python 3.12.3".to_string()),
                ("Local IP", "172.20.254.26 (eth0)".to_string()),
                ("Updates", "15 (5 security)".to_string()),
            ],
            "no Docker socket"
        );
        assert_eq!(field(&info, "toolchains", "python"), text("3.12.3"));
        assert_eq!(field(&info, "ip", "interface"), text("eth0"));
        assert_eq!(field(&info, "updates", "total"), int(15));
        assert_eq!(field(&info, "updates", "security"), int(5));
    }

    #[test]
    fn opt_in_modules_on_arch_desktop() {
        let info = collect(&ctx(Fixture::load("arch-desktop")), &OPT_IN);
        assert_eq!(
            lines(&info),
            [
                (
                    "Toolchains",
                    "rust 1.98.1 · node 22.11.0 · python 3.12.7".to_string()
                ),
                ("Containers", "3 running".to_string()),
                ("Local IP", "192.168.1.20 (enp5s0)".to_string()),
            ],
            "no update-notifier on Arch"
        );
        assert_eq!(field(&info, "toolchains", "count"), int(3));
        assert_eq!(field(&info, "docker", "running"), int(3));
        assert_eq!(field(&info, "ip", "address"), text("192.168.1.20"));
    }

    #[test]
    fn docker_errors_hide_the_module() {
        let mut fixture = Fixture::load("arch-desktop");
        fixture.sockets[0].reply =
            b"HTTP/1.0 403 Forbidden\r\nContent-Type: application/json\r\n\r\n{}".to_vec();
        assert!(collect(&ctx(fixture), &["docker"]).modules.is_empty());

        let mut fixture = Fixture::load("arch-desktop");
        fixture.sockets.clear();
        assert!(collect(&ctx(fixture), &["docker"]).modules.is_empty());
    }

    #[test]
    fn git_is_only_shown_inside_a_repository() {
        // git fails outside a repository: here, no canned run for that directory.
        let mut fixture = Fixture::load("arch-desktop");
        fixture.cwd = Some("/home/ada".into());
        assert!(collect(&ctx(fixture), &["git"]).modules.is_empty());

        let mut fixture = Fixture::load("arch-desktop");
        fixture.cwd = None;
        assert!(collect(&ctx(fixture), &["git"]).modules.is_empty());

        // A failing run (not a repository after all) is hidden too.
        let mut fixture = Fixture::load("wsl2");
        let git = fixture
            .commands
            .iter_mut()
            .find(|c| c.argv[0] == "git")
            .unwrap();
        git.status = 128;
        assert!(collect(&ctx(fixture), &["git"]).modules.is_empty());
    }

    #[test]
    fn each_battery_gets_a_bar() {
        let tree = Tree::new(
            "two-batteries",
            &[
                ("/sys/class/power_supply/AC/type", "Mains\n"),
                ("/sys/class/power_supply/BAT0/type", "Battery\n"),
                ("/sys/class/power_supply/BAT0/capacity", "80\n"),
                ("/sys/class/power_supply/BAT0/status", "Charging\n"),
                ("/sys/class/power_supply/BAT1/type", "Battery\n"),
                ("/sys/class/power_supply/BAT1/capacity", "9\n"),
            ],
        );
        let info = collect(&tree.ctx(), &["battery", "kernel"]);
        let rows: Vec<(String, Option<u64>)> = info
            .rows()
            .into_iter()
            .map(|r| (r.text, r.meter.map(|m| m.pct)))
            .collect();
        assert_eq!(
            rows,
            [
                ("80% [Charging]".to_string(), Some(80)),
                ("9%".to_string(), Some(9)),
            ],
            "no kernel without uname"
        );
    }

    #[test]
    fn wsl2_without_interop_falls_back() {
        let mut fixture = Fixture::load("wsl2");
        fixture.commands.clear();
        let info = collect(&ctx(fixture), &default_modules());
        let fields = lines(&info);
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
                r#"{{"boot_id": "{}", "facts": {{"windows": "Windows 99 (build 1)", "gpus": null}}}}"#,
                boot_id.trim()
            ),
        )
        .unwrap();
        let ctx = Ctx::new(Box::new(Fixture::load("wsl2")), Some(file.clone()), false);
        let info = collect(&ctx, &["windows", "gpu"]);
        let fields = lines(&info);
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
            saved.contains(r#""gpus": "nvidia-smi\nNVIDIA T1200 Laptop GPU""#),
            "{saved}"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn gpu_facts_from_before_the_source_was_kept_are_looked_up_again() {
        let dir = env::temp_dir().join(format!("ffetch-test-{}-old-gpu-fact", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("facts.json");
        let boot_id =
            fs::read_to_string(super::dir("wsl2").join("root/proc/sys/kernel/random/boot_id"))
                .unwrap();
        let facts = |key: &str| {
            format!(
                r#"{{"boot_id": "{}", "facts": {{"{key}": "Some Old GPU"}}}}"#,
                boot_id.trim()
            )
        };
        let gpu = || {
            let ctx = Ctx::new(Box::new(Fixture::load("wsl2")), Some(file.clone()), false);
            let info = collect(&ctx, &["gpu"]);
            (field(&info, "gpu", "name"), field(&info, "gpu", "source"))
        };

        // The old "gpu" fact (names only) is a miss: looked up again, and saved.
        fs::write(&file, facts("gpu")).unwrap();
        assert_eq!(gpu(), (text("T1200 Laptop GPU"), text("nvidia-smi")));
        let saved = fs::read_to_string(&file).unwrap();
        assert!(saved.contains(r#""gpus": "nvidia-smi\n"#), "{saved}");

        // A "gpus" fact not in the new form holds until the next boot (or
        // --refresh), but never shows: the PCI scan answers instead.
        fs::write(&file, facts("gpus")).unwrap();
        assert_eq!(gpu(), (text("Basic Render Driver"), text("pci")));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_registry_lists_the_fields_of_each_module() {
        let all: Vec<&str> = MODULES.iter().map(|m| m.id).collect();
        let mut seen = std::collections::BTreeSet::new();
        for name in ["wsl2", "arch-desktop"] {
            let info = collect(&ctx(Fixture::load(name)), &all);
            for (def, value) in &info.modules {
                assert_eq!(def.fields, value.fields(), "{}", def.id);
                seen.insert(def.id);
            }
        }
        assert_eq!(seen.len(), MODULES.len(), "the fixtures cover every module");
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
        assert_eq!(lines(&info), [("Terminal", "kitty".to_string())]);

        // No parent to walk (e.g. started by init): $TERM is all there is.
        let mut fixture = Fixture::load("arch-desktop");
        fixture.ppid = 1;
        let info = collect(&ctx(fixture), &["terminal"]);
        assert_eq!(lines(&info), [("Terminal", "xterm-kitty".to_string())]);
    }
}

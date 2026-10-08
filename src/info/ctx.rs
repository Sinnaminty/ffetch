//! The module context. Everything a module learns about the machine goes
//! through a `Ctx`, and through its `System`, so tests can swap the machine
//! for a fixture directory (see `tests/fixtures/README.md`).

use std::{
    env,
    ffi::{CStr, CString, OsString},
    fs,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::OnceLock,
};

use crate::{
    cache::{self, Facts},
    command,
    wsl::{self, Wsl},
};

/// Changes on every boot, including a `wsl --shutdown` or a Windows reboot.
const BOOT_ID: &str = "/proc/sys/kernel/random/boot_id";

/// Everything ffetch asks the machine about. `Live` asks this one.
pub trait System: Send + Sync {
    /// The file at the absolute `path`, if it exists and is UTF-8.
    fn read(&self, path: &Path) -> Option<String>;
    /// The entries of the directory at `path`, in no particular order.
    fn read_dir(&self, path: &Path) -> Option<Vec<Entry>>;
    fn exists(&self, path: &Path) -> bool;
    /// The environment variable `key`. A value that isn't UTF-8 reads as empty.
    fn env(&self, key: &str) -> Option<String>;
    fn uname(&self) -> Uname;
    fn statvfs(&self, path: &Path) -> Option<Statvfs>;
    /// Runs `cmd` like `command::run`: stdout captured, never hanging.
    fn run(&self, cmd: Command) -> Option<Output>;
    /// The parent process of ffetch.
    fn ppid(&self) -> u32;
    /// ffetch's real user ID.
    fn uid(&self) -> u32;
    /// The login name of `uid` in the passwd database. Not thread-safe: only
    /// called before the module threads start.
    fn user_name(&self, uid: u32) -> Option<String>;
}

/// A directory entry.
pub struct Entry {
    pub name: OsString,
    /// Whether it is a directory itself (a symlink to one doesn't count).
    pub is_dir: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Uname {
    pub nodename: String,
    pub release: String,
    pub machine: String,
}

/// The `statvfs` numbers ffetch uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Statvfs {
    /// Fragment size: the unit of the block counts.
    pub frsize: u64,
    pub blocks: u64,
    pub bfree: u64,
    /// Free blocks available to unprivileged users.
    pub bavail: u64,
}

/// Lists a directory of the real filesystem.
pub fn list_dir(path: &Path) -> Option<Vec<Entry>> {
    let entries = fs::read_dir(path).ok()?.filter_map(Result::ok);
    Some(
        entries
            .map(|e| Entry {
                is_dir: e.file_type().is_ok_and(|t| t.is_dir()),
                name: e.file_name(),
            })
            .collect(),
    )
}

/// The machine ffetch runs on.
pub struct Live;

impl System for Live {
    fn read(&self, path: &Path) -> Option<String> {
        fs::read_to_string(path).ok()
    }

    fn read_dir(&self, path: &Path) -> Option<Vec<Entry>> {
        list_dir(path)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn env(&self, key: &str) -> Option<String> {
        env::var_os(key).map(|v| v.into_string().unwrap_or_default())
    }

    fn uname(&self) -> Uname {
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

    fn statvfs(&self, path: &Path) -> Option<Statvfs> {
        let path = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: statvfs only fills in the zeroed struct.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
            return None;
        }
        Some(Statvfs {
            frsize: st.f_frsize as u64,
            blocks: st.f_blocks as u64,
            bfree: st.f_bfree as u64,
            bavail: st.f_bavail as u64,
        })
    }

    fn run(&self, cmd: Command) -> Option<Output> {
        command::run(cmd)
    }

    fn ppid(&self) -> u32 {
        unsafe { libc::getppid() as u32 }
    }

    fn uid(&self) -> u32 {
        unsafe { libc::getuid() }
    }

    fn user_name(&self, uid: u32) -> Option<String> {
        // SAFETY: getpwuid returns null or a pointer to a valid passwd entry in
        // a static buffer, which is why it's only called from one thread.
        let pw = unsafe { libc::getpwuid(uid) };
        if pw.is_null() {
            return None;
        }
        Some(
            unsafe { CStr::from_ptr((*pw).pw_name) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// Who is running ffetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub name: String,
    pub uid: u32,
}

impl User {
    #[cfg_attr(not(test), expect(dead_code, reason = "for quips (M5)"))]
    pub fn is_root(&self) -> bool {
        self.uid == 0
    }
}

/// What modules run against: the system, facts about it that every module may
/// need, and the shared inputs read at most once per run. It is shared by the
/// module threads.
pub struct Ctx {
    sys: Box<dyn System>,
    pub uname: Uname,
    /// The WSL version, or `None` outside WSL.
    pub wsl: Option<Wsl>,
    pub user: User,
    /// Facts cached until the next boot. Loaded before the modules run; saved
    /// by `collect` once they have finished.
    pub facts: Facts,
    meminfo: OnceLock<Option<String>>,
    cpuinfo: OnceLock<Option<String>>,
}

impl Ctx {
    /// The context for this machine, with the facts cached in the user's cache
    /// directory. With `refresh`, cached facts are looked up again.
    pub fn live(refresh: bool) -> Ctx {
        let facts_file = cache::dir().map(|d| d.join("facts.json"));
        Ctx::new(Box::new(Live), facts_file, refresh)
    }

    /// A context for `sys`, with the per-boot facts kept in `facts_file`, or
    /// not kept at all.
    pub fn new(sys: Box<dyn System>, facts_file: Option<PathBuf>, refresh: bool) -> Ctx {
        let boot_id = sys.read(Path::new(BOOT_ID));
        let facts = Facts::open(facts_file, boot_id, refresh);
        let uid = sys.uid();
        let name = sys
            .env("USER")
            .filter(|u| !u.is_empty())
            .or_else(|| sys.user_name(uid))
            .unwrap_or_else(|| "user".into());
        Ctx {
            uname: sys.uname(),
            wsl: wsl::detect(sys.as_ref()),
            user: User { name, uid },
            facts,
            sys,
            meminfo: OnceLock::new(),
            cpuinfo: OnceLock::new(),
        }
    }

    /// The file at `path`, trimmed; `None` if it is missing or blank.
    pub fn read(&self, path: impl AsRef<Path>) -> Option<String> {
        let s = self.sys.read(path.as_ref())?;
        let s = s.trim();
        (!s.is_empty()).then(|| s.to_string())
    }

    /// The whole file at `path`, untrimmed.
    pub fn read_full(&self, path: impl AsRef<Path>) -> Option<String> {
        self.sys.read(path.as_ref())
    }

    /// The paths in the directory at `path`, sorted; empty if it can't be read.
    pub fn sorted_dir(&self, path: impl AsRef<Path>) -> Vec<PathBuf> {
        let path = path.as_ref();
        let mut paths: Vec<PathBuf> = self
            .sys
            .read_dir(path)
            .unwrap_or_default()
            .into_iter()
            .map(|e| path.join(e.name))
            .collect();
        paths.sort();
        paths
    }

    /// How many subdirectories `path` has, not counting the ones named in `skip`.
    pub fn count_dirs(&self, path: impl AsRef<Path>, skip: &[&str]) -> Option<usize> {
        let entries = self.sys.read_dir(path.as_ref())?;
        Some(
            entries
                .iter()
                .filter(|e| e.is_dir)
                .filter(|e| !e.name.to_str().is_some_and(|n| skip.contains(&n)))
                .count(),
        )
    }

    pub fn exists(&self, path: impl AsRef<Path>) -> bool {
        self.sys.exists(path.as_ref())
    }

    pub fn env(&self, key: &str) -> Option<String> {
        self.sys.env(key)
    }

    /// The environment variable `key`, unless it is unset or empty.
    pub fn env_nonempty(&self, key: &str) -> Option<String> {
        self.env(key).filter(|v| !v.is_empty())
    }

    pub fn statvfs(&self, path: impl AsRef<Path>) -> Option<Statvfs> {
        self.sys.statvfs(path.as_ref())
    }

    /// Runs `cmd`; see `command::run`.
    pub fn run(&self, cmd: Command) -> Option<Output> {
        self.sys.run(cmd)
    }

    pub fn ppid(&self) -> u32 {
        self.sys.ppid()
    }

    /// `/proc/meminfo`, trimmed. Read at most once per run.
    pub fn meminfo(&self) -> Option<&str> {
        self.meminfo
            .get_or_init(|| self.read("/proc/meminfo"))
            .as_deref()
    }

    /// `/proc/cpuinfo`, trimmed. Read at most once per run.
    pub fn cpuinfo(&self) -> Option<&str> {
        self.cpuinfo
            .get_or_init(|| self.read("/proc/cpuinfo"))
            .as_deref()
    }

    /// The per-boot fact `key`: cached, or else computed by `compute`.
    pub fn cached(&self, key: &str, compute: impl FnOnce() -> Option<String>) -> Option<String> {
        self.facts.get(key, compute)
    }
}

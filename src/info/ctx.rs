//! The module context. Everything a module learns about the machine goes
//! through a `Ctx`, and through its `System`, so tests can swap the machine
//! for a fixture directory (see `tests/fixtures/README.md`).

use std::{
    env,
    ffi::{CStr, CString, OsString},
    fs,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::OnceLock,
    time::Duration,
};

use crate::{
    cache::{self, Facts},
    command, socket,
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
    /// The directory ffetch runs in.
    fn cwd(&self) -> Option<PathBuf>;
    fn uname(&self) -> Uname;
    fn statvfs(&self, path: &Path) -> Option<Statvfs>;
    /// The IP addresses of the network interfaces, in `getifaddrs` order.
    fn interfaces(&self) -> Vec<IfAddr>;
    /// Runs `cmd` like `command::run`: stdout captured, killed after `timeout`.
    fn run(&self, cmd: Command, timeout: Duration) -> Option<Output>;
    /// Sends `request` to the Unix socket at `path` and returns the reply, like
    /// `socket::request`; `None` if that fails or takes longer than `timeout`.
    fn unix_request(&self, path: &Path, request: &[u8], timeout: Duration) -> Option<Vec<u8>>;
    /// The parent process of ffetch.
    fn ppid(&self) -> u32;
    /// ffetch's real user ID.
    fn uid(&self) -> u32;
    /// The login name of `uid` in the passwd database. Not thread-safe: only
    /// called before the module threads start.
    fn user_name(&self, uid: u32) -> Option<String>;
    /// The hour of the local time, 0-23.
    fn local_hour(&self) -> Option<u8>;
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

/// An IP address of a network interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IfAddr {
    /// The interface, e.g. "eth0".
    pub name: String,
    pub addr: IpAddr,
    /// Switched on (`IFF_UP`).
    pub up: bool,
    /// Connected (`IFF_RUNNING`): it has a carrier.
    pub running: bool,
    pub loopback: bool,
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

    fn cwd(&self) -> Option<PathBuf> {
        env::current_dir().ok()
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

    fn interfaces(&self) -> Vec<IfAddr> {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        // SAFETY: getifaddrs allocates the list, which is freed below and not
        // used after that.
        if unsafe { libc::getifaddrs(&mut list) } != 0 {
            return Vec::new();
        }
        let mut addrs = Vec::new();
        let mut next = list;
        // SAFETY: each entry is null or valid until freeifaddrs; the names are
        // NUL-terminated.
        while let Some(ifa) = unsafe { next.as_ref() } {
            next = ifa.ifa_next;
            let Some(addr) = (unsafe { ip_addr(ifa.ifa_addr) }) else {
                continue;
            };
            let flag = |f: libc::c_int| ifa.ifa_flags & f as libc::c_uint != 0;
            addrs.push(IfAddr {
                name: unsafe { CStr::from_ptr(ifa.ifa_name) }
                    .to_string_lossy()
                    .into_owned(),
                addr,
                up: flag(libc::IFF_UP),
                running: flag(libc::IFF_RUNNING),
                loopback: flag(libc::IFF_LOOPBACK),
            });
        }
        unsafe { libc::freeifaddrs(list) };
        addrs
    }

    fn run(&self, cmd: Command, timeout: Duration) -> Option<Output> {
        command::run(cmd, timeout)
    }

    fn unix_request(&self, path: &Path, request: &[u8], timeout: Duration) -> Option<Vec<u8>> {
        socket::request(path, request, timeout)
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

    fn local_hour(&self) -> Option<u8> {
        // SAFETY: time with a null pointer only returns the time, and
        // localtime_r only fills in the zeroed struct (reading the time zone
        // from TZ or /etc/localtime the first time).
        let now = unsafe { libc::time(std::ptr::null_mut()) };
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
            return None;
        }
        u8::try_from(tm.tm_hour).ok()
    }
}

/// The IPv4 or IPv6 address in `sa`; `None` for other families.
///
/// SAFETY: `sa` is null or points to a socket address as big as its family's.
unsafe fn ip_addr(sa: *const libc::sockaddr) -> Option<IpAddr> {
    let family = unsafe { sa.as_ref() }?.sa_family as libc::c_int;
    match family {
        libc::AF_INET => {
            let sin = unsafe { &*sa.cast::<libc::sockaddr_in>() };
            // In network byte order, which is the order of the octets.
            let octets = sin.sin_addr.s_addr.to_ne_bytes();
            Some(IpAddr::V4(Ipv4Addr::from(octets)))
        }
        libc::AF_INET6 => {
            let sin6 = unsafe { &*sa.cast::<libc::sockaddr_in6>() };
            Some(IpAddr::V6(Ipv6Addr::from(sin6.sin6_addr.s6_addr)))
        }
        _ => None,
    }
}

/// Who is running ffetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub name: String,
    pub uid: u32,
}

impl User {
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

    pub fn cwd(&self) -> Option<PathBuf> {
        self.sys.cwd()
    }

    pub fn interfaces(&self) -> Vec<IfAddr> {
        self.sys.interfaces()
    }

    /// Runs `cmd` with the usual timeout; see `command::run`.
    pub fn run(&self, cmd: Command) -> Option<Output> {
        self.sys.run(cmd, command::TIMEOUT)
    }

    /// Runs `cmd`, killing it after `timeout`.
    pub fn run_within(&self, cmd: Command, timeout: Duration) -> Option<Output> {
        self.sys.run(cmd, timeout)
    }

    /// Sends `request` to the Unix socket at `path`; see `socket::request`.
    pub fn unix_request(
        &self,
        path: impl AsRef<Path>,
        request: &[u8],
        timeout: Duration,
    ) -> Option<Vec<u8>> {
        self.sys.unix_request(path.as_ref(), request, timeout)
    }

    pub fn ppid(&self) -> u32 {
        self.sys.ppid()
    }

    /// The hour of the local time, 0-23.
    pub fn local_hour(&self) -> Option<u8> {
        self.sys.local_hour()
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

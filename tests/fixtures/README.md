# Fixtures

Each directory here is a fake machine. The fixture tests in
`src/info/fixture.rs` run the info modules against it in place of the real
system (through `info::System`). Then they compare the plain `--no-color` info
column with `expected.txt`, quip included (picked with a fixed seed).

```
<name>/
  root/          the machine's filesystem: root/proc/meminfo is /proc/meminfo
  env            environment variables, KEY=VALUE per line (only these are set)
  uname          nodename=, release=, machine=
  process        ppid= (where the Terminal walk starts), uid=, cwd= (for Git)
  clock          hour= (the local time's hour, 0-23, for quips)
  statvfs        one line per path: / frsize=4096 blocks=… bfree=… bavail=…
  interfaces     getifaddrs, one address per line: eth0 192.168.1.20 up,running
  commands/      canned program runs, one file each (any name)
  sockets/       canned Unix socket exchanges, one file each (any name)
  expected.txt   the snapshot
```

In `env`, `uname`, `process`, `clock`, `statvfs` and `interfaces`, blank
lines and lines starting with `#` are ignored. The flags in `interfaces` are
any of `up`, `running` and `loopback`, comma-separated (`-` for none); only
IPv4 and IPv6 addresses are listed, in `getifaddrs` order.

Under `root/`, use plain files and directories, never symlinks. A directory
that has to exist but would be empty, such as a pacman package directory,
holds a small file so that git keeps it. ffetch only counts the directory
itself. Files that matter:

| Path | Read by |
|------|---------|
| `etc/os-release` | OS |
| `etc/passwd` | the user name when `USER` is unset |
| `proc/meminfo`, `proc/cpuinfo`, `proc/uptime` | Memory, CPU, Uptime |
| `proc/loadavg` | Load |
| `sys/class/hwmon/*/{name,temp1_input}`, `sys/class/thermal/thermal_zone*/{type,temp}` | CPU Temp |
| `proc/sys/kernel/osrelease` | WSL detection (with `WSL_DISTRO_NAME`) |
| `proc/sys/kernel/random/boot_id` | the per-boot fact cache |
| `proc/<pid>/comm`, `proc/<pid>/stat` | WM scan, Terminal parent walk |
| `sys/devices/virtual/dmi/id/*` | Host |
| `sys/bus/pci/devices/*/{class,vendor,device}`, `usr/share/{hwdata,misc}/pci.ids` | GPU |
| `sys/class/drm/*/{status,modes}` | Resolution |
| `sys/class/power_supply/*/{type,capacity,status}` | Battery |
| `sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq` | CPU clock |
| `var/lib/dpkg/status`, `var/lib/pacman/local/*`, `var/lib/flatpak/app/*`, … | Packages |
| `var/lib/update-notifier/updates-available` | Updates |

## Canned commands

A program run is matched by its full argv. If the file names a `cwd`, the
working directory must match too. A run with no matching file behaves like a
missing program. The header comes first, then a `---` line, then stdout
byte for byte (CRLF included, so `.gitattributes` turns off line-ending
conversion here):

```
program: cmd.exe
arg: /c
arg: ver
cwd: /mnt/c
---
Microsoft Windows [Version 10.0.26300.9457]
```

`status: N` sets the exit code (default 0).

## Canned sockets

A request to a Unix socket is matched by the socket's path and the request's
first line. Without a matching file, the socket behaves as if it were missing.
The reply follows the `---` line byte for byte, HTTP headers with CRLF
included:

```
path: /var/run/docker.sock
request: GET /containers/json HTTP/1.0
---
HTTP/1.0 200 OK
Content-Type: application/json
…
```

## Snapshots

`cargo test` fails when the output differs from `expected.txt` and prints a
diff. After an intended change, run `UPDATE_SNAPSHOTS=1 cargo test` and review
the rewritten files with `git diff`.

## The fixtures

- **wsl2**: modeled on the dev machine, an i7-11850H laptop running WSL2
  Ubuntu 24.04 inside tmux. `meminfo`, `cpuinfo`, `os-release` and the PCI
  devices are copied from it. The user and host names, `pci.ids` (trimmed),
  the dpkg database (6 packages) and the boot ID are synthetic. `wslinfo`,
  `cmd.exe`, `nvidia-smi`, `zsh --version` and `python3 --version` are canned,
  and so is `git status` for a clean checkout level with its upstream. Like
  WSL, it has no CPU temperature sensors (only the AC adapter and battery
  in hwmon), and no Docker socket. Its interfaces include the private
  address WSL's DNS tunnel puts on `lo`, and update-notifier reports 15
  updates, 5 of them security updates. Its clock says 2am, so the quip
  tells you to go to sleep.
- **arch-desktop**: synthetic Arch Linux with KDE Plasma on Wayland. It has
  pacman and flatpak packages, a DMI model, two connected DRM outputs, an
  NVIDIA and an AMD GPU, a Ryzen 7950X with a cpufreq maximum, and no
  battery. Its process tree is `kwin_wayland` plus a kitty → fish → bash
  parent chain. hwmon has an NVMe and an amdgpu sensor around `k10temp`.
  The working directory is a git checkout one commit ahead of its upstream
  with 3 changed files. `rustc`, `node` and `python3` are canned, and the
  Docker socket lists 3 running containers. It's 2pm, 11 days after the
  boot, which is what the quip is about.

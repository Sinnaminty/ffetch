# Fixtures

Each directory here is a fake machine. The fixture tests in
`src/info/fixture.rs` run the info modules against it in place of the real
system (through `info::System`). Then they compare the plain `--no-color` info
column with `expected.txt`.

```
<name>/
  root/          the machine's filesystem: root/proc/meminfo is /proc/meminfo
  env            environment variables, KEY=VALUE per line (only these are set)
  uname          nodename=, release=, machine=
  process        ppid= (where the Terminal walk starts), uid=
  statvfs        one line per path: / frsize=4096 blocks=… bfree=… bavail=…
  commands/      canned program runs, one file each (any name)
  expected.txt   the snapshot
```

In `env`, `uname`, `process` and `statvfs`, blank lines and lines starting with
`#` are ignored.

Under `root/`, use plain files and directories, never symlinks. A directory
that has to exist but would be empty, such as a pacman package directory,
holds a small file so that git keeps it. ffetch only counts the directory
itself. Files that matter:

| Path | Read by |
|------|---------|
| `etc/os-release` | OS |
| `etc/passwd` | the user name when `USER` is unset |
| `proc/meminfo`, `proc/cpuinfo`, `proc/uptime` | Memory, CPU, Uptime |
| `proc/sys/kernel/osrelease` | WSL detection (with `WSL_DISTRO_NAME`) |
| `proc/sys/kernel/random/boot_id` | the per-boot fact cache |
| `proc/<pid>/comm`, `proc/<pid>/stat` | WM scan, Terminal parent walk |
| `sys/devices/virtual/dmi/id/*` | Host |
| `sys/bus/pci/devices/*/{class,vendor,device}`, `usr/share/{hwdata,misc}/pci.ids` | GPU |
| `sys/class/drm/*/{status,modes}` | Resolution |
| `sys/class/power_supply/*/{type,capacity,status}` | Battery |
| `sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq` | CPU clock |
| `var/lib/dpkg/status`, `var/lib/pacman/local/*`, `var/lib/flatpak/app/*`, … | Packages |

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

## Snapshots

`cargo test` fails when the output differs from `expected.txt` and prints a
diff. After an intended change, run `UPDATE_SNAPSHOTS=1 cargo test` and review
the rewritten files with `git diff`.

## The fixtures

- **wsl2**: modeled on the dev machine, an i7-11850H laptop running WSL2
  Ubuntu 24.04 inside tmux. `meminfo`, `cpuinfo`, `os-release` and the PCI
  devices are copied from it. The user and host names, `pci.ids` (trimmed),
  the dpkg database (6 packages) and the boot ID are synthetic. `wslinfo`,
  `cmd.exe`, `nvidia-smi` and `zsh --version` are canned.
- **arch-desktop**: synthetic Arch Linux with KDE Plasma on Wayland. It has
  pacman and flatpak packages, a DMI model, two connected DRM outputs, an
  NVIDIA and an AMD GPU, a Ryzen 7950X with a cpufreq maximum, and no
  battery. Its process tree is `kwin_wayland` plus a kitty → fish → bash
  parent chain.

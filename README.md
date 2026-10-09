# ffetch

A neofetch-style system info tool for Linux, built around *your* image.
ffetch turns any PNG into colored ASCII art on its own, takes the color theme
for the whole output from that image, treats WSL as a first-class platform,
and has an attitude.

```
            |%#%_||     _;;:       fizz@KUS-ITPROG2
       ;| |#@@@#@#x#%;ooooo;       ----------------
      :o#%#@@@@##%%%xoooxooo%%x    OS: Ubuntu 24.04.5 LTS x86_64
   __ x%#@@@@@###o;ooo;xxoo#@#x;   Host: Windows Subsystem for Linux 2.6.3.0 (Ubuntu)
  ;o;o#@@@@@@#%xooooo;oooox#@@%%   Windows: Windows 11 (build 26300)
  ";o%%@@@@@@@#xoooox%xxooox##@#   Kernel: 6.6.87.2-microsoft-standard-WSL2
  )^;xx%@###@#xoo;;x#@@@@#o%#%##   Uptime: 2 hours, 32 mins
    ;ooo%##%oxx%xooo%@@@@@###%##   Packages: 1140 (dpkg)
  _,|_;oooooooooooox#@@###@#o###   Shell: zsh 5.9
  x%o%#xo;ooooxoo#%xx#@@####%x%#   Terminal: tmux
 ;|_%##xoooooooooo%#@@@@@#%;x;;x   CPU: 11th Gen Intel Core i7-11850H (16) @ 2.50GHz
 _%@@@@##%oxxxo%##%#@@@###x;;;oo   Load: 0.21, 0.06, 0.01 (16 threads)
x@@@@@@%@@#@@@#@#######o%%;;oooo   GPU: NVIDIA T1200 Laptop GPU
xo%######@@#@#@#@@##%%o;;;;;;ooo   Memory: [#---------] 1.07 GiB / 15.48 GiB (6%)
oo;;##########%%######%%x;;;;;oo   Disk (/): [#---------] 79.46 GiB / 1006.85 GiB (8%)
oo;x%x%#######ox%######%%x;;;;;;   Battery: [##########] 100% [Full]
                                   Git: spec-v0.2, clean
                                   Locale: C.UTF-8

                                    +-------+
                                   -| what. |
                                    +-------+
```

That's `ffetch --no-color --size 32`. In a color terminal the logo, labels and
title are painted in colors taken from the image, the usage bars are drawn as
`██░░░░░░░░` in green, yellow or red, and the speech bubble has rounded
corners.

## Features

- **Any image as the logo.** `--image photo.png` removes the background,
  downsamples and renders the picture as ASCII (`--logo ascii`) or half-block
  pixels (`--logo blocks`), scaled to fit the terminal.
- **A theme from the image.** A palette is extracted from the logo (k-means in
  Oklab, deterministic), and its colors are used for the user and host name,
  the labels, the separator and the color swatches.
- **WSL done properly.** The real GPU (via `nvidia-smi` or PowerShell)
  instead of "Microsoft Basic Render Driver", the Windows version, and the WSL
  version and distro. The slow lookups are cached until the next boot.
- **Useful info.** Usage bars for memory, disk and battery, load average, CPU
  temperature, git status for the current directory, and opt-in modules for
  toolchains, Docker containers, local IP and pending updates.
- **Quips.** The character comments on your machine from a speech bubble:
  low battery, full disk, high load, a long uptime, or being run at 2am.
- **Scriptable.** `--json` for structured values and the palette,
  `--format`/`--oneline` for prompts, MOTDs and status bars, and a TOML config
  file.
- **Fast and quiet.** Modules run in parallel and read `/proc` and `/sys`
  directly. A module that can't find its value is left out, and a broken image
  or config file costs a warning, never your shell's startup.

## Installing

ffetch runs on Linux (including WSL). Building it needs Rust 1.88 or newer.

```sh
cargo install --path .
```

The built-in logo is `assets/logo.png`. To bake in a different one, set
`FFETCH_LOGO` when building:

```sh
FFETCH_LOGO=~/Pictures/avatar.png cargo install --path .
```

To run ffetch whenever a shell starts, add `ffetch` to the end of your
`~/.bashrc` or `~/.zshrc`.

## Usage

```sh
ffetch                               # logo and info
ffetch --image ~/Pictures/cat.png    # another logo, and a theme to match
ffetch --logo blocks --size 40       # half-block pixels, 40 columns wide
ffetch --layout stacked              # logo above the info
ffetch --modules os,uptime,load,git  # just these, in this order
ffetch --oneline                     # Ubuntu 24.04.5 LTS x86_64 · up 2 hours, 32 mins · mem 6% · disk 8%
ffetch --format '{os} · {memory.pct}% ram'
ffetch --json | jq .palette
```

| Option | |
|--------|-|
| `-l`, `--logo <style>` | `ascii` (default), `blocks` or `none` |
| `-s`, `--size <cols>` | Logo width in columns (default 48; shrinks to fit the terminal) |
| `--image <path>` | Use a PNG as the logo and take the colors from it |
| `--keep-background` | With `--image`, keep the image's background instead of removing it |
| `--layout <kind>` | `auto` (default: beside the info if it fits, else above it), `side` or `stacked` |
| `--modules <ids>` | Show these modules, in this order |
| `--swatches <kind>` | `palette` (default: the image's colors), `ansi` (the 16 terminal colors) or `none` |
| `--no-bars` | Hide the usage bars |
| `--no-quip` | Hide the speech bubble |
| `--no-color` | Disable colors (`NO_COLOR` is honored too) |
| `--refresh` | Recompute the facts cached until the next boot |
| `--json` | Print the info and the palette as JSON |
| `--format <text>` | Print one line: `{module}` is a module's value, `{module.field}` one of its fields, `{{` and `}}` are braces |
| `--oneline` | OS, uptime, memory and disk on one line |
| `--config <path>` | Read this config file instead of the default one |
| `--print-config` | Print the settings in effect as a config file |

Options override the config file, which overrides the defaults.

### Modules

| id | Label | Default | Notes |
|----|-------|---------|-------|
| `os` | OS | on | |
| `host` | Host | on | On WSL: the WSL version and distro |
| `windows` | Windows | on | WSL only |
| `kernel` | Kernel | on | |
| `uptime` | Uptime | on | |
| `packages` | Packages | on | dpkg, pacman, rpm, apk, emerge, brew, flatpak and snap |
| `shell` | Shell | on | |
| `resolution` | Resolution | on | |
| `de` | DE | on | |
| `wm` | WM | on | |
| `terminal` | Terminal | on | Shows the multiplexer and Windows Terminal together |
| `cpu` | CPU | on | |
| `temp` | CPU Temp | on | Not available on WSL |
| `load` | Load | on | |
| `gpu` | GPU | on | On WSL: the physical GPU |
| `memory` | Memory | on | With a usage bar |
| `disk` | Disk | on | With a usage bar |
| `battery` | Battery | on | With a charge bar per battery |
| `git` | Git | on | Branch, ahead/behind and changes; only inside a repo |
| `locale` | Locale | on | |
| `toolchains` | Toolchains | off | rust, node and python versions |
| `docker` | Containers | off | Running containers, from the Docker socket |
| `ip` | Local IP | off | Off so screenshots don't leak it |
| `updates` | Updates | off | Ubuntu's update-notifier count |

`ffetch --help` lists the ids too.

## Configuration

ffetch reads `${XDG_CONFIG_HOME:-~/.config}/ffetch/config.toml` if it exists.
`ffetch --print-config` prints the settings in effect in the same format, so
it's a good starting point:

```sh
mkdir -p ~/.config/ffetch && ffetch --print-config > ~/.config/ffetch/config.toml
```

```toml
layout = "auto"          # auto | side | stacked
swatches = "palette"     # palette | ansi | none
bars = true
modules = ["os", "host", "windows", "kernel", "uptime", "packages", "shell",
           "terminal", "cpu", "gpu", "memory", "disk", "battery", "git"]

[logo]
style = "ascii"          # ascii | blocks | none
size = 48
image = "~/Pictures/avatar.png"   # relative paths are relative to this file
keep_background = false

[theme]
background = "dark"      # dark | light
# accent = "#c33e58"     # override any of accent, secondary, muted

[quip]                   # or just `quip = false` at the top level
enabled = true

[[quip.rule]]
when = "uptime_days >= 30"
say = ["a month. impressive. concerning."]
```

Mistakes in the file (a syntax error, an unknown key, a bad value) each print
one warning and fall back to the default.

### Quip rules

Your rules are checked before the built-in ones, and the first match wins.
`when` is a list of comparisons joined by `and`, using `<`, `<=`, `>`, `>=`,
`==` and `!=` with these names:

`battery`, `battery_discharging` (0/1), `mem_pct`, `disk_pct`, `load1`,
`threads`, `load_ratio` (load1 / threads), `uptime_days`, `hour` (0–23),
`root` (0/1), `packages`

A rule without `when` always matches. One of the `say` lines is picked at
random, and it can use the same names as `{placeholders}`, plus `{clock}` for
the time (`2am`). Keep them under 40 characters.

## Caching

ffetch keeps two caches in `${XDG_CACHE_HOME:-~/.cache}/ffetch/`:

- `facts.json` holds slow facts such as the Windows version and the WSL GPU
  name. It's valid until the next boot (on WSL, until Windows reboots or
  `wsl --shutdown`). `--refresh` recomputes it.
- `images/` holds processed `--image` logos, keyed by path, size and
  modification time.

Deleting either is always safe.

## Development

```sh
cargo test
```

Besides the unit tests, the fixture tests in `src/info/fixture.rs` run every
module against the fake machines in `tests/fixtures/` (a WSL2 laptop and an
Arch desktop) and compare the text and JSON output with snapshots. After an
intended output change, update them with `UPDATE_SNAPSHOTS=1 cargo test` and
review the diff. `tests/fixtures/README.md` explains how a fixture is laid out.

To look at colored output as an image (needs Pillow):

```sh
ffetch | python3 tools/preview.py preview.png
```

`SPEC.md` describes the design and the reasoning behind it.

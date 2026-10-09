# ffetch — Differentiation Spec

Status: Implemented (M1–M6) on branch `spec-v0.2` · 2026-10-08 · Takes ffetch from 0.1.0 to 0.2.0

Decisions made during implementation are folded into the sections below; §8 notes the current behavior for each open question.

## 1. Context

### 1.1 Where ffetch is today (v0.1)

- Neofetch-style output: logo on the left; user@host, 16 info modules and the ANSI color swatches on the right.
- The logo comes from `assets/logo.png`. `build.rs` removes the background region that touches the image border, downsamples to at most 256 px, and embeds the RGBA buffer. `src/logo.rs` renders it at runtime as colored ASCII (luminance ramp `:;ox%#@` plus edge glyphs matched to the outline) or as half-blocks (`--logo blocks`).
- The accent color for labels is the removed background color (`#C33E58` for the current image).
- The logo shrinks to fit the terminal; colors fall back from truecolor to 256-color to none.
- Linux only. Modules read `/proc` and `/sys` directly and run in parallel threads. About 10 ms wall time on the dev machine (WSL2, i7-11850H).

### 1.2 Landscape

- **neofetch** was archived in 2024. Being faster than it is the minimum, not a selling point.
- **fastfetch** is the real benchmark: fast (C), a very large module set, image logos through terminal graphics protocols or external converters, and JSON output.

### 1.3 Positioning

> ffetch is the fetch tool built around *your* image. It converts any picture into ASCII art itself, themes the whole output from that picture, treats WSL as a first-class platform, and has an attitude.

## 2. Goals and non-goals

**Goals**

| ID | Goal |
|----|------|
| G1 | Image-first identity: any image becomes the logo **and** the color theme, at runtime. |
| G2 | WSL is a first-class platform: show the real GPU, the Windows version and the WSL version. |
| G3 | Info that is useful at a glance, not just decoration. |
| G4 | Personality: the character comments on the system. |
| G5 | Scriptable and configurable: JSON, one-line output, a config file. |

**Non-goals for this spec**

- macOS, BSD and native Windows.
- Terminal graphics protocols (kitty, sixel, iTerm2). Doing the image conversion in-house is the point; revisit later.
- A library of distro logos.
- Matching fastfetch's module count.

## 3. Cross-cutting requirements

| ID | Requirement |
|----|-------------|
| R1 | **Performance budget.** A default run (warm cache) takes ≤ 25 ms wall time on the dev machine, measured with `hyperfine -N --warmup 3 ffetch`. Every module has a cost class (§5.2). A module in the `slow` class (> 50 ms) must be either cached (F2.5) or opt-in. |
| R2 | **Silent degradation.** A module that can't determine its value is omitted. It never prints an error and never fails the run. |
| R3 | **Color modes.** Every feature works in truecolor, 256-color and no-color modes. No-color output stays readable. Where a glyph's meaning depends on color (the usage bars), it gets an ASCII fallback. Other Unicode (`↑↓`, `°`, `·`, box drawing) is used as is. |
| R4 | **Determinism.** The same image always produces the same logo and palette. Output is stable between runs except for live values (and the quip, F4). |
| R5 | **Dependencies.** Each new crate needs a stated reason in its PR. Expected additions: `png` (moves from a build-only dependency to a runtime one, for F1.1), plus `serde`, `serde_json` and `toml` (F5). |
| R6 | **Precedence.** CLI flags override the config file, and the config file overrides built-in defaults. |

## 4. Features

### F1 — Image-derived theme

#### F1.1 Runtime image (`--image`)

**Behavior**

- `ffetch --image PATH` uses PATH as the logo instead of the embedded image. Config key: `logo.image`.
- It runs the same pipeline as `build.rs` today: decode, remove the background, downsample to 256 px. Images larger than 2048 px are first shrunk to at most 1024 px **before** background removal, so large photos stay fast. (Smaller images skip this step so the default logo's output doesn't change.)
- `--keep-background` (config `logo.keep_background`) skips background removal, for images where the background is part of the picture.
- The processed buffer is cached at `${XDG_CACHE_HOME:-~/.cache}/ffetch/images/<key>.rgba`. The key is an FNV-1a hash of the canonical path, file size, mtime, `--keep-background`, the crate version and a format marker. The cache file has a small header with width, height and the removed background color.
- Images over 64 megapixels are refused with the usual single warning, so a crafted file can't exhaust memory.
- If the image fails to load, ffetch prints one warning line to stderr, uses the embedded logo, and exits 0. A broken image must never break someone's shell startup.
- Only PNG is supported in this phase.

**Implementation notes**

- Move `decode`, `key_out_background` and `downsample` from `build.rs` into `src/image.rs`. `build.rs` reuses the file with `#[path = "src/image.rs"] mod image;`, so there is one implementation.
- `logo.rs` currently reads the `WIDTH`, `HEIGHT` and `PIXELS` constants. Change it to take a `&LogoImage { width, height, pixels, background }` built from either the embedded data or the runtime image.

**Acceptance**

- `ffetch --image assets/logo.png` produces byte-for-byte the same output as the default run.
- Uncached run of a 1254×1254 PNG: ≤ 80 ms. Cached run: within R1.
- A missing file, a non-PNG file and a truncated PNG each produce one warning, the embedded logo, and exit code 0.

#### F1.2 Palette extraction

**Behavior**

- Extract a palette of up to 6 colors from the logo's opaque pixels (alpha ≥ 0.5 in the 256 px buffer).
- Algorithm: k-means in Oklab, k = 6, k-means++ initialization with a fixed seed, 10 iterations. Drop clusters holding less than 2% of the pixels. The fixed seed satisfies R4.
- Assign three theme roles:
  - `accent`: labels and the user name. Use the removed background color if there was one (this is today's behavior); otherwise the cluster with the highest chroma × weight among clusters with lightness between 0.35 and 0.80.
  - `muted`: the separator line and the `@`. The cluster with the lowest chroma among clusters with lightness between 0.4 and 0.8.
  - `secondary`: the host name. The **most common** cluster with lightness ≥ 0.35 that is more than ΔE_ok 0.15 from `accent` and more than 0.05 from `muted`; if none qualifies, the accent. (The first draft picked the most chromatic cluster, but on the reference image that was a near-black, which made the host name unreadable.)
- Role colors are adjusted for the terminal background (`theme.background = "dark" | "light"`, default dark). On a dark background, a color with HSL lightness below 0.45 is raised to 0.45; on a light background, one above 0.55 is lowered to 0.55. Every other color stays exactly as it is in the image.
- Overrides: `theme.accent`, `theme.secondary` and `theme.muted` accept `#rrggbb`.

**Acceptance**

- For `assets/logo.png`, `accent` is `#C33E58`; the palette contains colors close to cream `#F2D7C5`, rose-beige `#C9A69D` and dark mauve `#473D46` (ΔE_ok < 0.05); `secondary` is the cream and `muted` the rose-beige. (Maroon `#633540` was also listed originally, but k-means merges it into dark mauve for every seed and sample size tried, so it was dropped.)
- Running twice on the same image gives an identical palette.

#### F1.3 Themed output and swatches

- Title: user in `accent`, `@` in `muted`, host in `secondary`. Separator line in `muted`. Labels in `accent`. Values keep the terminal's default foreground color.
- `--swatches palette|ansi|none` (config `swatches`), default `palette`:
  - `palette`: one row with each palette color as a 3-column block.
  - `ansi`: today's two rows of the 16 ANSI colors.
- 256-color mode quantizes role and palette colors with `term::to_ansi256`.

### F2 — First-class WSL

Detection: `/proc/sys/kernel/osrelease` contains `microsoft` (case-insensitive), or `WSL_DISTRO_NAME` is set. It's WSL2 if the release string contains `WSL2`.

Measured on the dev machine:

| Source | Output | Time |
|--------|--------|------|
| `wslinfo --version` | `2.6.3.0` | ~1 ms |
| `$WSL_DISTRO_NAME` | `Ubuntu` | 0 |
| `/usr/lib/wsl/lib/nvidia-smi --query-gpu=name --format=csv,noheader` | `NVIDIA T1200 Laptop GPU` | ~120 ms |
| `cmd.exe /c ver` | `Microsoft Windows [Version 10.0.26300.9457]` | ~130 ms |
| PCI scan (current GPU module) | `Microsoft Basic Render Driver` (×2, virtual adapter) | < 1 ms |

#### F2.1 Host

`Host: Windows Subsystem for Linux 2.6.3.0 (Ubuntu)`: the version comes from `wslinfo --version` and the distro from `WSL_DISTRO_NAME`. If `wslinfo` is missing, fall back to today's text.

#### F2.2 Windows version (new module `windows`, label `Windows`)

- `Windows 11 (build 26300)`. The build number comes from `cmd.exe /c ver`. Build ≥ 22000 means Windows 11; anything lower is Windows 10.
- Run `cmd.exe` with its working directory set to `/mnt/c` and stderr discarded. Otherwise it prints a "UNC paths are not supported" warning when started from a Linux path.
- Cost class `slow`, so it uses the per-boot cache (F2.5). The module only exists on WSL.

#### F2.3 Real GPU

- On WSL, replace `Microsoft Basic Render Driver` with the physical GPU name:
  1. NVIDIA: `nvidia-smi` at the path above, one line per GPU.
  2. Others (AMD, Intel): `powershell.exe -NoProfile -NonInteractive -Command "(Get-CimInstance Win32_VideoController).Name"`. Expected to take hundreds of ms (not yet measured).
- Results go in the per-boot cache. Show `Microsoft Basic Render Driver` only when both lookups fail.

#### F2.4 Terminal

- When `WT_SESSION` is set and a multiplexer is detected, show both: `tmux (Windows Terminal)`.
- **Stretch:** resolve the Windows Terminal profile name. `WT_PROFILE_ID` is a GUID, which has to be looked up in WT's `settings.json` on the Windows side (`/mnt/c/Users/<user>/AppData/Local/Packages/Microsoft.WindowsTerminal_8wekyb3d8bbwe/LocalState/settings.json`). Finding `<user>` reliably is unsolved, and `WT_*` variables don't pass into tmux by default. Only do this if it's cheap.

#### F2.5 Per-boot cache

- File: `${XDG_CACHE_HOME:-~/.cache}/ffetch/facts.json` with `{ "boot_id": "...", "facts": { "<key>": "<value>" } }`.
- Valid while `boot_id` matches `/proc/sys/kernel/random/boot_id`. A Windows reboot or `wsl --shutdown` changes the boot ID, which also covers Windows updates and GPU changes.
- Writes are atomic: write a temp file, then rename it.
- `--refresh` recomputes all cached facts.
- The first run after each boot pays for the slow modules. They run in parallel, so expect about 130 ms. Every later run must meet R1.

**Acceptance (dev machine)**

- `GPU: NVIDIA T1200 Laptop GPU`
- `Windows: Windows 11 (build 26300)`
- `Host: Windows Subsystem for Linux 2.6.3.0 (Ubuntu)`
- A cached run meets R1. Deleting the cache file makes the next run repopulate it.

### F3 — Useful info

#### F3.1 Usage bars

- Memory, Disk and Battery show a 10-cell bar right after the label: `Memory: ██░░░░░░░░ 1.86 GiB / 15.48 GiB (12%)`. Filled cells are `ceil(pct / 10)`. Empty cells use the theme's `muted` color. Each battery gets its own bar.
- Bar color uses these thresholds:

  | Level | Usage | Battery charge |
  |-------|-------|----------------|
  | ok | < 60% | > 40% |
  | warn | 60–85% | 15–40% |
  | crit | > 85% | < 15% |

  The filled cells use ANSI green, yellow and red (SGR 32/33/31), so they follow the user's terminal theme.
- In no-color mode the bar is drawn in ASCII: `[##--------]`.
- Toggle with `--no-bars` or config `bars = false`.

#### F3.2 New modules

| id | Label | Source | Cost | Default |
|----|-------|--------|------|---------|
| `load` | Load | `/proc/loadavg`, shown with the thread count: `0.14, 0.15, 0.08 (16 threads)` | fast | on |
| `temp` | CPU Temp | `/sys/class/hwmon/*` where the name is `coretemp`, `k10temp` or `zenpower`; otherwise `/sys/class/thermal/thermal_zone*` with type `x86_pkg_temp`. Hidden on WSL, which exposes neither. | fast | on |
| `git` | Git | `git --no-optional-locks status --porcelain=v2 --branch` in the current directory: `main ↑1 ↓0, 3 changed` or `main, clean`. Arrows appear only when ahead/behind isn't 0:0. A detached HEAD shows the short oid. Only shown inside a repo. 50 ms timeout, then hidden. `--no-optional-locks` matters: a timed-out run must not leave an `index.lock` behind. | spawn | on |
| `toolchains` | Toolchains | `rustc --version`, `node --version`, `python3 --version`, all in parallel: `rust 1.98.1 · node 22.x · python 3.12`. Shows only the ones installed. | spawn (20–30 ms each, through rustup proxies) | off |
| `docker` | Containers | `GET /containers/json` (HTTP/1.0) over `/var/run/docker.sock` using a std `UnixStream` (no `docker` process): `3 running`. 100 ms timeout. Hidden if the socket is missing or not accessible, or the reply isn't a complete 200. | fast | off |
| `ip` | Local IP | `getifaddrs`, first IPv4 address that isn't loopback: `192.168.1.20 (eth0)` | fast | off (people post screenshots) |
| `updates` | Updates | Ubuntu: parse `/var/lib/update-notifier/updates-available`. Never run a package-manager query. | fast | off |

Default order for the info column: OS, Host, Windows, Kernel, Uptime, Packages, Shell, Resolution, DE, WM, Terminal, CPU, CPU Temp, Load, GPU, Memory, Disk, Battery, Git, Locale.

`--modules id,id,...` replaces the module list, in the given order; it's how opt-in modules are enabled from the command line, and it overrides the config file's `modules` (R6). Each unknown id produces one warning and is skipped.

#### F3.3 Responsive layout

- `--layout auto|side|stacked` (config `layout`), default `auto`.
- `auto`:
  1. **Side by side** if the logo can be at least 16 columns wide with the info column at least 32 (this is today's logic).
  2. Otherwise **stacked**: the logo goes above the info, left-aligned, with a blank row between them. Its width is `min(48, terminal width, the widest whose rows fit in terminal height − info rows − 3)`, and it must be at least 16 columns.
  3. Otherwise no logo (this is today's fallback).
- `side` and `stacked` force that arrangement, with no logo if it doesn't fit. An explicit `--size` skips the height-based shrinking.

**Acceptance:** table-driven tests for terminal sizes 200×50, 120×30, 80×24, 50×40, 40×20 and 30×10 (and the boundaries around them) that check the chosen layout and logo width.

### F4 — Personality: quips

**Behavior**

- A one-line remark from the character in a speech bubble at the bottom of the info column, pointing toward the logo:

  ```
   ╭──────────────────────────────────╮
  ─┤ 11 days without a reboot. bold.  │
   ╰──────────────────────────────────╯
  ```

  In no-color mode the bubble is drawn with `+ - |`. Without a logo beside it (stacked or no logo) there is no tail. The border uses the `muted` role. On a narrow column the text is truncated with `…`, and the bubble is dropped if it would be narrower than 12 columns.
- Rules are checked in priority order and the first match wins (user rules first). Each rule has several templates; one is picked at random per run, seeded from the clock. Placeholders are filled from module data.

| Priority | Condition (`when`) | Templates |
|----------|-----------|-------------------|
| 1 | `battery < 15 and battery_discharging == 1` | `{battery}% battery. living dangerously.` · `plug me in.` · `{battery}% left. find a charger.` |
| 2 | `mem_pct >= 90` | `{mem_pct}% ram. i'm drowning.` · `memory's at {mem_pct}%. close a tab.` · `{mem_pct}% memory used. i can't think.` |
| 3 | `disk_pct >= 90` | `disk's {disk_pct}% full. delete something.` · `disk at {disk_pct}%. no room for anything.` |
| 4 | `load_ratio > 1` | `load {load1} on {threads} threads. breathe.` · `load {load1}. one thing at a time.` · `everything at once? load {load1}.` |
| 5 | `uptime_days >= 7` | `{uptime_days} days without a reboot. bold.` · `up {uptime_days} days. i could use a nap.` · `{uptime_days} days up. reboot me already.` |
| 6 | `hour < 5` | `it's {clock}. go to sleep.` · `it's {clock}. let me sleep.` · `{clock}. nothing good happens now.` |
| 7 | `root == 1` | `running a fetch as root. sure.` · `root? for a fetch? fine.` |
| 8 | `packages > 3000` | `{packages} packages. i'm stuffed.` · `{packages} packages. i'm full.` · `{packages} packages and counting. ugh.` |
| 9 | always | `what.` · `fine. here are your stats.` · `don't screenshot me.` |

Conditions use a small language: `<name> <op> <number>` joined by `and`, with ops `< <= > >= == !=`. Built-in and user rules share it. Names: `battery`, `battery_discharging` (0/1), `mem_pct`, `disk_pct`, `load1`, `threads`, `load_ratio` (load1 / threads), `uptime_days`, `hour` (0–23), `root` (0/1), `packages`. A name whose module didn't run or had no value is undefined, and any condition using it is false. Unknown names and placeholders are parse errors. In templates, `{clock}` renders the hour as `2am`/`12pm`, `{packages}` gets thousands separators, and `{load1}` one decimal.

- Tone: grumpy, at most 40 characters, lowercase. No profanity, and never about the user as a person — only about the machine.
- On by default. Disable with `--no-quip` or config `quip = false`. Users can override or extend rules with `[[quip.rule]]` tables in the config (they take priority over the built-ins).
- Local time comes from `libc::localtime_r`.

**Acceptance:** unit tests feed fake system states to the rule engine and assert which rule wins; templates are tested to always fit within 40 characters after placeholders are filled.

### F5 — Scripting and configuration

#### F5.1 JSON output (`--json`)

- Prints one JSON object with **structured** values (not display strings), then exits. No logo and no ANSI codes.

  ```json
  {
    "schema": 1,
    "user": "fizz",
    "host": "KUS-ITPROG2",
    "palette": { "accent": "#c33e58", "secondary": "#…", "muted": "#…", "colors": ["#f2d7c5", "…"] },
    "modules": {
      "os": { "name": "Ubuntu 24.04.5 LTS", "arch": "x86_64" },
      "uptime": { "seconds": 11580 },
      "memory": { "used_bytes": 1997000000, "total_bytes": 16620000000 },
      "gpu": [{ "vendor": "NVIDIA", "name": "T1200 Laptop GPU", "source": "nvidia-smi" }]
    }
  }
  ```

- `schema` is incremented on any breaking change. Modules with no value, and unknown fields within a value, are left out; they are never `null`.
- `gpu` entries are `{name, source, vendor?}`: `source` is `pci`, `nvidia-smi` or `powershell`, and `vendor` appears only when known (PCI, nvidia-smi). Other shapes: `packages` is `{total, managers: {dpkg: N, …}}`, `resolution` a list of `{width, height}`, `battery` a list of `{pct, status}`, `toolchains` a map of name to version, `git` has separate `ahead`/`behind`.
- The palette's role colors are the ones actually painted (after background adjustment and overrides); `colors` are the image's own.
- Including `palette` means other tools (status bars, terminal themes) can take their colors from the same image.

#### F5.2 One-line output (`--format`, `--oneline`)

- `--format '<template>'` with `{module}` placeholders, e.g. `--format '{os} · up {uptime} · mem {memory.pct}%'`.
- `--oneline` is a preset: `{os} · up {uptime} · mem {memory.pct}% · disk {disk.pct}%`.
- Only the modules named in the template are run. Intended for MOTDs, prompts and status bars, so it must meet R1 easily (measured: ~1.3 ms for `--oneline`).
- An unknown module or field is a usage error (exit 2), reported before anything runs. A module with no value at runtime renders as an empty string. `{{` and `}}` are literal braces. Multi-line values are joined with `, `.
- `--format` and `--oneline` don't read the config file. `--json`, `--format` and `--oneline` are mutually exclusive.

#### F5.3 Config file

- Location: `${XDG_CONFIG_HOME:-~/.config}/ffetch/config.toml`. Override with `--config PATH`.
- `--print-config` prints the effective config: defaults merged with the file and flags.
- Unknown keys produce a warning, not an error.

```toml
layout = "auto"          # auto | side | stacked
swatches = "palette"     # palette | ansi | none
bars = true
modules = ["os", "host", "windows", "kernel", "uptime", "packages", "shell",
           "terminal", "cpu", "gpu", "memory", "disk", "battery", "git"]

[logo]
style = "ascii"          # ascii | blocks | none
size = 48
image = "~/Pictures/avatar.png"
keep_background = false

[theme]
background = "dark"      # dark | light
# accent = "#c33e58"

[quip]                   # or just `quip = false` at the top level
enabled = true

[[quip.rule]]
when = "uptime_days >= 30"
say = ["a month. impressive. concerning."]
```

The `when` expressions in user quip rules use the condition language from F4. A rule without `when` always matches, which replaces the fallback. Bad rules are skipped with a warning naming the rule number; a `say` string over 40 characters warns but is kept.

Error handling (a fetch tool runs at shell startup and must never break it):
- A missing default config is silent. A missing `--config` path warns once.
- A TOML syntax error warns once, with `file:line:column`, and the file is ignored.
- Unknown keys, invalid values and unknown module ids each warn once and fall back to the default for that key.
- A relative `logo.image` is resolved against the config file's directory; `~` is expanded.
- `--print-config` writes unset keys as comments, since TOML has no null.

## 5. Architecture changes

### 5.1 Module context and structured data

- Change modules from `fn() -> Option<String>` to `fn(&Ctx) -> Option<Box<dyn Report>>`.
  - `Ctx` holds the filesystem root (`/` normally, a fixture directory in tests), the `uname` result, WSL detection, the fact cache, and lazily read shared files (`/proc/meminfo`, `/proc/cpuinfo`) so modules don't read them twice.
  - `Report` is `Serialize` (for F5.1), has `display(&Theme) -> String` (text plus optional bar), and exposes numeric fields for quips and `--format`.
- All file reads go through `ctx.path("/proc/…")`, so tests can use fixture trees like `tests/fixtures/wsl2/`, `tests/fixtures/arch-desktop/` and so on.

### 5.2 Module registry

```rust
struct ModuleDef {
    id: &'static str,       // config + JSON key
    label: &'static str,    // display label
    cost: Cost,             // Fast | Spawn | Slow (Slow requires caching or opt-in)
    default_on: bool,
    run: fn(&Ctx) -> Option<Box<dyn Report>>,
}
```

### 5.3 New source files

| File | Responsibility |
|------|----------------|
| `src/image.rs` | Decode, background removal and downsampling; shared with `build.rs` |
| `src/palette.rs` | Oklab k-means and role assignment |
| `src/config.rs` | TOML config, merged with CLI flags |
| `src/cache.rs` | Per-boot fact cache and image cache |
| `src/wsl.rs` | WSL detection and WSL-specific facts |
| `src/quip.rs` | Rule engine, templates, bubble rendering |
| `src/layout.rs` | Side, stacked and no-logo layout (moved out of `main.rs`) |
| `src/output/{text,json,format}.rs` | The three output modes |

## 6. Milestones

Each milestone ships on its own and keeps R1–R6.

| # | Scope | Depends on |
|---|-------|------------|
| M1 | F1: runtime image, palette, themed output, swatches; extract `src/image.rs` | — |
| M2 | F2: WSL modules plus the per-boot cache | — |
| M3 | §5.1–5.2 refactor (context, structured reports, fixture testing) | — |
| M4 | F3: bars, new modules, stacked layout | M3 |
| M5 | F4: quips | M3 (needs numeric fields) |
| M6 | F5: JSON, `--format`/`--oneline`, config file | M3 |

M1 and M2 are the most distinctive and the most visible, so they come first. They can be built against today's module signature and ported during M3.

## 7. Testing

- **Unit tests:** palette determinism and role assignment, `to_ansi256`, `pci_lookup`, CPU name cleanup, uptime formatting, layout selection (F3.3 table), the quip rule engine, `--format` parsing, config merging.
- **Fixture tests:** for each fixture tree, a snapshot of `--json` output and of the plain (`--no-color`) text output.
- **Visual check:** a dev script that renders ANSI output to PNG, so logo and theme changes can be reviewed as images (a prototype exists from v0.1 work; move it to `tools/preview.py`).
- **Performance:** `hyperfine` numbers recorded in each milestone PR for the default run, `--image` (cold and warm cache), and `--oneline`.

## 8. Open questions

1. **Quips on by default?** The spec says yes because it's the most memorable feature. It could be off by default for a quieter tool. *Currently: on; `--no-quip` or `quip = false`.*
2. **Light terminals:** auto-detect the background with an OSC 11 query (adds a terminal round trip and is complicated when output isn't a TTY), or rely on `theme.background` only? *Currently: config only. The light theme adjusts the title, labels and borders, but the ASCII logo is still tuned for dark backgrounds, so its cream areas look pale on white.*
3. **Value color:** values currently stay the terminal's default color. Should they be themed too? *Currently: default color.*
4. **Non-NVIDIA GPUs on WSL:** is a `powershell.exe` call (hundreds of ms, once per boot) acceptable, or should they stay as "Basic Render Driver"? *Currently: PowerShell fallback, measured at 0.6–0.75 s on the first run after a boot.*
5. **Image formats:** add JPEG/WebP (the `image` crate adds compile time; `zune-jpeg` is lighter) or keep PNG only? *Currently: PNG only.*
6. **JSON stability:** commit to `schema: 1` as public at 0.2.0, or keep it experimental until 1.0? *Currently: `schema: 1`, no stability promise written down.*
7. **Windows Terminal profile name (F2.4 stretch):** worth the complexity? *Currently: not implemented.*
8. **Interop without `appendWindowsPath`:** `cmd.exe` and `powershell.exe` are found through `PATH`, so they're missing when WSL's `appendWindowsPath=false`. Fall back to `/mnt/c/Windows/System32/...`? *Currently: no fallback; the Windows line is hidden and the GPU falls back to the PCI name.*
9. **`--format` extras:** `{user}` and `{hostname}` placeholders for prompts? *Currently: module placeholders only.*

//! End-to-end tests of the `ffetch` binary.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const RESET: &str = "\x1b[0m";

/// A fresh, empty directory for one test, used as `XDG_CACHE_HOME`.
fn temp_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// `XDG_CONFIG_HOME` for a test whose cache is `cache`: empty until the test
/// writes a config there, so the user's own config never gets in.
fn config_home(cache: &Path) -> PathBuf {
    cache.join("config-home")
}

/// Runs ffetch in truecolor mode from the repository root.
fn ffetch(cache: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ffetch"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("XDG_CACHE_HOME", cache)
        .env("XDG_CONFIG_HOME", config_home(cache))
        .env("COLORTERM", "truecolor")
        .env_remove("NO_COLOR")
        .output()
        .expect("ffetch runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr_lines(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .map(str::to_string)
        .collect()
}

/// The parts of the output that don't change between runs: each line up to its
/// first reset (the whole logo row; piped output uses the 48-column logo, which
/// is usually taller than the info column) and the themed title. The info
/// values are live (uptime, memory) and can differ from one run to the next,
/// and so can the quip: the edges of its bubble are left out, in case they
/// are below the logo.
fn stable_parts(out: &Output) -> Vec<String> {
    let text = stdout(out);
    let mut parts: Vec<String> = text
        .lines()
        .filter(|l| !l.contains('╭') && !l.contains('╰'))
        .filter_map(|l| l.split_once(RESET).map(|(logo, _)| logo.to_string()))
        .collect();
    parts.extend(
        text.lines()
            .next()
            .map(|title| title.trim_start().to_string()),
    );
    parts
}

/// Lines with background colours: the palette row, or the two ANSI rows.
fn swatch_lines(out: &Output) -> Vec<String> {
    let is_swatch = |l: &&str| {
        ["\x1b[48;", "\x1b[40m", "\x1b[100m"]
            .iter()
            .any(|code| l.contains(code))
    };
    stdout(out)
        .lines()
        .filter(is_swatch)
        .map(str::to_string)
        .collect()
}

#[test]
fn image_option_matches_the_embedded_logo() {
    let cache = temp_dir("same-logo");
    for style in ["ascii", "blocks"] {
        let default = ffetch(&cache, &["--logo", style]);
        let cold = ffetch(&cache, &["--logo", style, "--image", "assets/logo.png"]);
        let warm = ffetch(&cache, &["--logo", style, "--image", "assets/logo.png"]);
        for out in [&default, &cold, &warm] {
            assert!(out.status.success());
            assert!(out.stderr.is_empty(), "{:?}", stderr_lines(out));
        }
        assert!(stable_parts(&default).len() > 20);
        assert_eq!(
            stable_parts(&cold),
            stable_parts(&default),
            "{style}, cold cache"
        );
        assert_eq!(
            stable_parts(&warm),
            stable_parts(&default),
            "{style}, warm cache"
        );
    }
    let entries = fs::read_dir(cache.join("ffetch/images")).expect("cache directory exists");
    assert_eq!(entries.count(), 1, "one entry, reused by later runs");
}

#[test]
fn keep_background_changes_the_logo() {
    let cache = temp_dir("keep-background");
    let default = ffetch(&cache, &[]);
    let kept = ffetch(&cache, &["--image", "assets/logo.png", "--keep-background"]);
    assert!(kept.status.success());
    assert!(kept.stderr.is_empty(), "{:?}", stderr_lines(&kept));
    assert_ne!(stable_parts(&kept), stable_parts(&default));
}

#[test]
fn broken_images_warn_once_and_fall_back() {
    let cache = temp_dir("broken");
    let not_png = cache.join("notes.png");
    fs::write(&not_png, "just some text\n").unwrap();
    let logo = fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/logo.png")).unwrap();
    let truncated = cache.join("truncated.png");
    fs::write(&truncated, &logo[..logo.len() / 2]).unwrap();

    let default = ffetch(&cache, &[]);
    for path in [Path::new("/nonexistent.png"), &not_png, &truncated, &cache] {
        let out = ffetch(&cache, &["--image", path.to_str().unwrap()]);
        assert_eq!(out.status.code(), Some(0), "{path:?}");
        let warnings = stderr_lines(&out);
        assert_eq!(warnings.len(), 1, "{path:?}: {warnings:?}");
        assert!(
            warnings[0].starts_with("ffetch: ") && warnings[0].contains("built-in logo"),
            "{warnings:?}"
        );
        assert_eq!(stable_parts(&out), stable_parts(&default), "{path:?}");
    }
}

#[test]
fn swatch_styles() {
    let cache = temp_dir("swatches");
    let palette = ffetch(&cache, &["--logo", "none"]);
    let explicit = ffetch(&cache, &["--logo", "none", "--swatches", "palette"]);
    let ansi = ffetch(&cache, &["--logo", "none", "--swatches=ansi"]);
    let none = ffetch(&cache, &["--logo", "none", "--swatches", "none"]);
    for out in [&palette, &explicit, &ansi, &none] {
        assert!(out.status.success());
    }

    let rows = swatch_lines(&palette);
    assert_eq!(rows.len(), 1, "palette is a single row");
    let blocks = rows[0].matches("\x1b[48;2;").count();
    assert!((1..=6).contains(&blocks), "{blocks} palette colours");
    assert_eq!(swatch_lines(&explicit), rows);

    let rows = swatch_lines(&ansi);
    assert_eq!(rows.len(), 2);
    assert!(rows[0].starts_with("\x1b[40m   \x1b[41m"));
    assert!(rows[1].starts_with("\x1b[100m   \x1b[101m"));

    assert!(swatch_lines(&none).is_empty());
    assert!(
        !stdout(&none).contains("\x1b[4"),
        "no background colours at all"
    );
}

#[test]
fn no_color_has_no_escapes_or_swatches() {
    let cache = temp_dir("no-color");
    let out = ffetch(&cache, &["--no-color", "--swatches", "ansi"]);
    assert!(out.status.success());
    assert!(!stdout(&out).contains('\x1b'));
}

#[test]
fn bad_arguments_exit_2() {
    let cache = temp_dir("bad-args");
    for args in [
        &["--swatches", "rainbow"][..],
        &["--swatches"],
        &["--image"],
        &["--layout", "diagonal"],
        &["--layout"],
        &["--modules"],
        &["--bogus"],
        &["--format"],
        &["--config"],
    ] {
        let out = ffetch(&cache, args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8_lossy(&out.stderr).contains("ffetch --help"));
    }
}

#[test]
fn help_lists_the_new_options() {
    let cache = temp_dir("help");
    let help = stdout(&ffetch(&cache, &["--help"]));
    for option in [
        "--image",
        "--keep-background",
        "--swatches",
        "--refresh",
        "--layout",
        "--modules",
        "--no-bars",
        "--no-quip",
        "--json",
        "--format",
        "--oneline",
        "--config",
        "--print-config",
    ] {
        assert!(help.contains(option), "{option} missing from --help");
    }
    // The module ids, opt-in ones included.
    for id in [
        "os,",
        "temp,",
        "load,",
        "git,",
        "toolchains,",
        "docker,",
        "ip,",
        "updates",
    ] {
        assert!(help.contains(id), "{id} missing from --help");
    }
    assert!(help.lines().all(|l| l.chars().count() <= 90), "{help}");
}

/// The info lines of a `--no-color --logo none` run with `args`.
fn plain_lines(cache: &Path, args: &[&str]) -> (Output, Vec<String>) {
    let mut all = vec!["--no-color", "--logo", "none"];
    all.extend(args);
    let out = ffetch(cache, &all);
    let lines = stdout(&out).lines().map(str::to_string).collect();
    (out, lines)
}

#[test]
fn usage_bars_and_no_bars() {
    let cache = temp_dir("bars");
    let memory = |lines: &[String]| {
        let line = lines.iter().find(|l| l.starts_with("Memory: ")).cloned();
        line.expect("a Memory line")["Memory: ".len()..].to_string()
    };
    let (_, lines) = plain_lines(&cache, &[]);
    let with_bar = memory(&lines);
    let (bar, rest) = with_bar.split_at(12);
    assert!(
        bar.starts_with('[') && bar.ends_with(']') && bar[1..11].chars().all(|c| "#-".contains(c)),
        "{with_bar:?}"
    );
    assert!(
        rest.starts_with(' ') && rest.ends_with("%)"),
        "{with_bar:?}"
    );

    let (out, lines) = plain_lines(&cache, &["--no-bars"]);
    assert!(out.status.success());
    assert!(memory(&lines).ends_with("%)") && !memory(&lines).contains('['));
}

#[test]
fn quip_bubble_and_no_quip() {
    let cache = temp_dir("quip");
    // No colour and no logo: an ASCII bubble without a tail, after a blank row
    // (and before the output's trailing blank line).
    let (out, lines) = plain_lines(&cache, &[]);
    assert!(out.status.success());
    let [blank, top, middle, bottom, end] = &lines[lines.len() - 5..] else {
        panic!("{lines:?}");
    };
    assert_eq!((blank.as_str(), end.as_str()), ("", ""));
    assert!(top.starts_with("+-") && top.ends_with("-+"), "{lines:?}");
    assert_eq!(top, bottom);
    assert!(
        middle.starts_with("| ") && middle.ends_with(" |"),
        "{lines:?}"
    );
    assert_eq!(middle.chars().count(), top.chars().count());
    let quip = &middle[2..middle.len() - 2];
    assert!(!quip.is_empty() && quip.chars().count() <= 40, "{quip:?}");
    assert_eq!(quip, quip.to_lowercase());

    let (out, lines) = plain_lines(&cache, &["--no-quip"]);
    assert!(out.status.success());
    assert!(
        !lines
            .iter()
            .any(|l| l.starts_with('+') || l.starts_with('|')),
        "{lines:?}"
    );

    // In colour beside the logo (auto's choice in a pipe), the bubble is
    // rounded and its tail points at the logo.
    let text = stdout(&ffetch(&cache, &[]));
    for piece in ["╭", "─┤", "╯"] {
        assert!(text.contains(piece), "{piece} missing:\n{text}");
    }
    let text = stdout(&ffetch(&cache, &["--no-quip"]));
    assert!(!text.contains('╭') && !text.contains("─┤"), "{text}");
}

#[test]
fn modules_option_replaces_the_list_and_skips_unknown_ids() {
    let cache = temp_dir("modules");
    let (out, lines) = plain_lines(&cache, &["--modules", "kernel,bogus,os,nah,bogus,kernel"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        stderr_lines(&out),
        [
            "ffetch: unknown module 'bogus'; skipping it",
            "ffetch: unknown module 'nah'; skipping it",
        ]
    );
    let labels: Vec<&str> = lines[2..]
        .iter()
        .take_while(|l| !l.is_empty())
        .map(|l| l.split(':').next().unwrap())
        .collect();
    assert_eq!(labels, ["Kernel", "OS"]);

    // Opt-in modules can be asked for; ones with nothing to show are left out.
    let (out, _) = plain_lines(&cache, &["--modules=updates,docker,load"]);
    assert!(out.status.success());
    assert!(out.stderr.is_empty(), "{:?}", stderr_lines(&out));
}

#[test]
fn stacked_layout_puts_the_logo_above_the_info() {
    let cache = temp_dir("stacked");
    let text = stdout(&ffetch(&cache, &["--no-color", "--layout", "stacked"]));
    let lines: Vec<&str> = text.lines().collect();
    let title = lines
        .iter()
        .position(|l| l.starts_with("OS: "))
        .expect("an OS line at the left edge")
        - 2;
    // Not a terminal, so the logo keeps its default width: 48 columns, 24 rows,
    // then a blank row.
    assert_eq!(title, 25, "{text}");
    assert!(lines[..24].iter().all(|l| l.chars().count() == 48));
    assert!(lines[24].is_empty());
    assert!(lines[title + 1].chars().all(|c| c == '-'));

    // Side by side, auto's choice in a pipe. Only the logo columns are stable.
    let logo_columns = |text: String| -> Vec<String> {
        text.lines().map(|l| l.chars().take(48).collect()).collect()
    };
    let side = stdout(&ffetch(&cache, &["--no-color", "--layout", "side"]));
    assert!(!side.lines().any(|l| l.starts_with("OS: ")));
    let auto = stdout(&ffetch(&cache, &["--no-color", "--layout=auto"]));
    assert_eq!(logo_columns(side), logo_columns(auto));
}

/// Whether this is WSL, by the same rule ffetch uses.
fn on_wsl() -> bool {
    let release = fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    release.to_lowercase().contains("microsoft")
        || std::env::var("WSL_DISTRO_NAME").is_ok_and(|d| !d.is_empty())
}

#[test]
fn facts_are_cached_per_boot_until_refreshed() {
    let cache = temp_dir("facts");
    let facts = cache.join("ffetch/facts.json");
    let plain = ["--no-color", "--logo", "none"];
    let first = ffetch(&cache, &plain);
    assert!(first.status.success());
    assert!(first.stderr.is_empty(), "{:?}", stderr_lines(&first));

    if !on_wsl() {
        // Off WSL nothing is cached and the output has no WSL lines.
        assert!(!stdout(&first).contains("Windows:"));
        let refreshed = ffetch(&cache, &["--refresh"]);
        assert!(refreshed.status.success());
        assert!(!facts.exists(), "no facts file off WSL");
        return;
    }

    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
    let boot_id = boot_id.trim();
    let text = fs::read_to_string(&facts).expect("the first run on WSL writes the facts");
    assert!(
        text.contains(&format!("\"boot_id\": \"{boot_id}\"")),
        "{text}"
    );

    // Later runs use the cached facts instead of looking them up again...
    let doctored = format!(
        r#"{{"boot_id": "{boot_id}", "facts": {{"windows": "Windows 99 (build 1)", "gpus": null}}}}"#
    );
    fs::write(&facts, &doctored).unwrap();
    let cached = stdout(&ffetch(&cache, &plain));
    assert!(cached.contains("Windows: Windows 99 (build 1)"), "{cached}");

    // ...until --refresh looks them up again...
    let refreshed = ffetch(&cache, &["--no-color", "--logo", "none", "--refresh"]);
    assert!(refreshed.status.success());
    assert!(!stdout(&refreshed).contains("Windows 99"));
    assert!(!fs::read_to_string(&facts).unwrap().contains("Windows 99"));

    // ...or the machine reboots.
    fs::write(&facts, doctored.replace(boot_id, "another-boot")).unwrap();
    assert!(!stdout(&ffetch(&cache, &plain)).contains("Windows 99"));
    assert!(fs::read_to_string(&facts).unwrap().contains(boot_id));
}

/// The JSON `--json` prints with `args`, checking that it is all there is.
fn json(cache: &Path, args: &[&str]) -> serde_json::Value {
    let mut all = vec!["--json"];
    all.extend(args);
    let out = ffetch(cache, &all);
    assert!(out.status.success(), "{:?}", stderr_lines(&out));
    assert!(out.stderr.is_empty(), "{:?}", stderr_lines(&out));
    let text = stdout(&out);
    assert!(!text.contains('\x1b'), "no ANSI codes");
    serde_json::from_str(&text).expect("--json prints JSON")
}

#[test]
fn json_output() {
    let cache = temp_dir("json");
    let json = json(&cache, &[]);
    assert_eq!(json["schema"], 1);
    assert!(json["user"].is_string() && json["host"].is_string());
    let is_hex = |v: &serde_json::Value| {
        v.as_str().is_some_and(|s| {
            s.len() == 7
                && s.starts_with('#')
                && s[1..].bytes().all(|b| b"0123456789abcdef".contains(&b))
        })
    };
    let palette = &json["palette"];
    assert!(
        ["accent", "secondary", "muted"]
            .iter()
            .all(|r| is_hex(&palette[r]))
    );
    let colors = palette["colors"].as_array().unwrap();
    assert!(!colors.is_empty() && colors.iter().all(is_hex), "{palette}");
    let modules = json["modules"].as_object().unwrap();
    assert!(modules["os"]["name"].is_string(), "{json}");
    assert!(modules["uptime"]["seconds"].is_u64(), "{json}");
    assert!(modules["memory"]["total_bytes"].is_u64(), "{json}");
    assert!(modules.values().all(|v| !v.is_null()), "nothing is null");

    // --modules picks them, in its order.
    let text = stdout(&ffetch(&cache, &["--json", "--modules", "kernel,os"]));
    let (kernel, os) = (text.find("\"kernel\": {"), text.find("\"os\": {"));
    assert!(kernel.unwrap() < os.unwrap(), "{text}");
    let picked = serde_json::from_str::<serde_json::Value>(&text).unwrap();
    assert_eq!(picked["modules"].as_object().unwrap().len(), 2);
}

#[test]
fn format_and_oneline() {
    let cache = temp_dir("format");
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease").unwrap();
    let out = ffetch(
        &cache,
        &["--format", "{kernel} {{{kernel.release}}} [{temp}]"],
    );
    let text = stdout(&out);
    assert!(out.status.success() && out.stderr.is_empty(), "{out:?}");
    assert!(
        text.starts_with(&format!("{0} {{{0}}} [", kernel.trim())),
        "{text:?}"
    );
    assert!(
        text.ends_with("]\n") && text.lines().count() == 1,
        "{text:?}"
    );
    if on_wsl() {
        // No CPU temperature there: the placeholder is empty.
        assert!(text.ends_with(" []\n"), "{text:?}");
    }

    let out = ffetch(&cache, &["--oneline"]);
    let text = stdout(&out);
    assert!(out.status.success() && out.stderr.is_empty(), "{out:?}");
    assert!(!text.contains('\x1b'), "{text:?}");
    let parts: Vec<&str> = text.trim_end().split(" · ").collect();
    assert_eq!(parts.len(), 4, "{text:?}");
    assert!(parts[1].starts_with("up ") && parts[2].starts_with("mem "));
    assert!(parts[3].starts_with("disk ") && parts[3].ends_with('%'));
    // Same as the preset spelled out.
    let spelled = ffetch(
        &cache,
        &[
            "--format",
            "{os} · up {uptime} · mem {memory.pct}% · disk {disk.pct}%",
        ],
    );
    assert_eq!(stdout(&spelled).split(" · ").next(), Some(parts[0]));
}

#[test]
fn bad_templates_and_conflicting_outputs_exit_2() {
    let cache = temp_dir("bad-format");
    for (args, message) in [
        (&["--format", "{bogus}"][..], "unknown module 'bogus'"),
        (&["--format", "{memory.percent}"], "no field 'percent'"),
        (&["--format", "{os"], "unclosed '{'"),
        (&["--format", "}"], "unmatched '}'"),
        (&["--json", "--oneline"], "don't go together"),
        (&["--format", "{os}", "--json"], "don't go together"),
    ] {
        let out = ffetch(&cache, args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.starts_with("ffetch: "), "{stderr}");
        assert!(stderr.contains(message), "{args:?}: {stderr}");
        assert!(stderr.contains("ffetch --help"), "{stderr}");
    }
}

/// Writes `text` as the config file at `path`.
fn write_config(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn config_file_changes_the_output() {
    let cache = temp_dir("config");
    let file = cache.join("my-config.toml");
    write_config(
        &file,
        "quip = false\nmodules = [\"os\", \"kernel\"]\nswatches = \"none\"\n",
    );
    let config = file.to_str().unwrap();
    let (out, lines) = plain_lines(&cache, &["--config", config]);
    assert!(out.status.success() && out.stderr.is_empty(), "{out:?}");
    let labels: Vec<&str> = lines[2..]
        .iter()
        .map(|l| l.split(':').next().unwrap())
        .collect();
    assert_eq!(
        labels,
        ["OS", "Kernel", ""],
        "no swatches, no quip: {lines:?}"
    );

    // Options win over the file.
    let (_, lines) = plain_lines(&cache, &["--config", config, "--modules", "uptime"]);
    assert!(
        lines[2].starts_with("Uptime: ") && lines.len() == 4,
        "{lines:?}"
    );

    // The file in the default place is read too...
    write_config(
        &config_home(&cache).join("ffetch/config.toml"),
        "modules = [\"kernel\"]\n",
    );
    let (_, lines) = plain_lines(&cache, &[]);
    assert!(
        lines[2].starts_with("Kernel: ") && lines[3].is_empty(),
        "{lines:?}"
    );
    // ...unless another one is named.
    let (_, lines) = plain_lines(&cache, &["--config", config]);
    assert!(lines[2].starts_with("OS: "), "{lines:?}");
}

#[test]
fn config_problems_are_warnings() {
    let cache = temp_dir("config-warnings");
    let file = cache.join("config.toml");
    write_config(
        &file,
        "layout = \"diagonal\"\ncolour = \"red\"\nmodules = [\"os\", \"bogus\"]\n\
         [theme]\naccent = \"red\"\n[logo]\nsize = 3\n",
    );
    let path = file.to_str().unwrap();
    let (out, lines) = plain_lines(&cache, &["--config", path]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        lines[2].starts_with("OS: "),
        "the rest still applies: {lines:?}"
    );
    let warnings = stderr_lines(&out);
    let at = |line: u32| format!("ffetch: {path}:{line}:");
    assert_eq!(warnings.len(), 5, "{warnings:?}");
    for (warning, line) in warnings.iter().zip([1, 2, 3, 5, 7]) {
        assert!(warning.starts_with(&at(line)), "{warnings:?}");
    }

    // Not TOML at all: one warning, and the defaults.
    write_config(&file, "layout = \"auto\nquip = false\n");
    let (out, lines) = plain_lines(&cache, &["--config", path]);
    assert_eq!(out.status.code(), Some(0));
    let warnings = stderr_lines(&out);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].starts_with(&format!("ffetch: {path}:1:15: ")),
        "{warnings:?}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("| ")),
        "quip still on: {lines:?}"
    );

    // A missing file named on the command line: one warning.
    let missing = cache.join("nope.toml");
    let out = ffetch(&cache, &["--config", missing.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stderr_lines(&out).len(), 1, "{:?}", stderr_lines(&out));
    assert!(!out.stdout.is_empty());
}

#[test]
fn print_config_reads_back_the_same() {
    let cache = temp_dir("print-config");
    let file = cache.join("config.toml");
    write_config(
        &file,
        "bars = false\nmodules = [\"os\", \"git\"]\n[logo]\nsize = 30\n\
         [theme]\nbackground = \"light\"\naccent = \"#00FF00\"\n\
         [[quip.rule]]\nwhen = \"hour < 24\"\nsay = [\"always \\\"quoted\\\".\"]\n",
    );
    let args = [
        "--config",
        file.to_str().unwrap(),
        "--layout",
        "side",
        "--image",
        "assets/logo.png",
        "--print-config",
    ];
    let first = ffetch(&cache, &args);
    assert!(
        first.status.success() && first.stderr.is_empty(),
        "{first:?}"
    );
    let printed = stdout(&first);
    for line in [
        "layout = \"side\"",
        "bars = false",
        "modules = [\"os\", \"git\"]",
        "size = 30",
        "background = \"light\"",
        "accent = \"#00ff00\"",
        "[[quip.rule]]",
    ] {
        assert!(printed.lines().any(|l| l == line), "{line:?}:\n{printed}");
    }
    let image = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/logo.png");
    assert!(printed.contains(&format!("image = {:?}", image.to_str().unwrap())));

    let again = cache.join("printed.toml");
    fs::write(&again, &printed).unwrap();
    let second = ffetch(
        &cache,
        &["--config", again.to_str().unwrap(), "--print-config"],
    );
    assert!(second.stderr.is_empty(), "{:?}", stderr_lines(&second));
    assert_eq!(stdout(&second), printed);
    // The defaults, with no config file.
    let defaults = stdout(&ffetch(&cache, &["--print-config"]));
    assert!(defaults.contains("layout = \"auto\"\n"), "{defaults}");
}

/// The truecolor title line (user@host) of a run with `args`.
fn title(cache: &Path, args: &[&str]) -> String {
    let mut all = vec!["--logo", "none"];
    all.extend(args);
    stdout(&ffetch(cache, &all))
        .lines()
        .next()
        .unwrap()
        .to_string()
}

#[test]
fn theme_from_the_config_file() {
    let cache = temp_dir("theme");
    let file = cache.join("config.toml");
    let path = file.to_str().unwrap();
    let dark = title(&cache, &[]);
    // The host name is in the cream of the logo, too light for a light terminal.
    write_config(&file, "[theme]\nbackground = \"light\"\n");
    let light = title(&cache, &["--config", path]);
    assert_ne!(light, dark);
    assert!(dark.contains("\x1b[38;2;242;215;197m"), "{dark:?}");
    assert!(!light.contains("\x1b[38;2;242;215;197m"), "{light:?}");
    // Role overrides are used exactly.
    write_config(
        &file,
        "[theme]\naccent = \"#00ff00\"\nsecondary = \"#0000ff\"\n",
    );
    let custom = title(&cache, &["--config", path]);
    assert!(custom.contains("\x1b[38;2;0;255;0m") && custom.contains("\x1b[38;2;0;0;255m"));
    // And they reach the JSON palette.
    let json = json(&cache, &["--config", path]);
    assert_eq!(json["palette"]["accent"], "#00ff00");
}

#[test]
fn quip_rules_from_the_config_file() {
    let cache = temp_dir("quip-rules");
    let file = cache.join("config.toml");
    write_config(
        &file,
        "[[quip.rule]]\nwhen = \"bogus > 1\"\nsay = [\"never.\"]\n\
         [[quip.rule]]\nwhen = \"hour >= 0\"\nsay = [\"mine, always.\"]\n",
    );
    let (out, lines) = plain_lines(&cache, &["--config", file.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(
        lines.contains(&"| mine, always. |".to_string()),
        "{lines:?}"
    );
    let warnings = stderr_lines(&out);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("quip rule 1: unknown name 'bogus'")
            && warnings[0].ends_with("; skipping it"),
        "{warnings:?}"
    );
}

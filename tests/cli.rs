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

/// Runs ffetch in truecolor mode from the repository root.
fn ffetch(cache: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ffetch"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("XDG_CACHE_HOME", cache)
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
/// is taller than the info column) and the themed title. The info values are
/// live (uptime, memory) and can differ from one run to the next.
fn stable_parts(out: &Output) -> Vec<String> {
    let text = stdout(out);
    let mut parts: Vec<String> = text
        .lines()
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
        &["--bogus"],
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
    for option in ["--image", "--keep-background", "--swatches", "--refresh"] {
        assert!(help.contains(option), "{option} missing from --help");
    }
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
        r#"{{"boot_id": "{boot_id}", "facts": {{"windows": "Windows 99 (build 1)", "gpu": null}}}}"#
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

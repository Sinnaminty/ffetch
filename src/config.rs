//! The config file, `${XDG_CONFIG_HOME:-~/.config}/ffetch/config.toml` or the
//! one named with `--config`, and how it combines with the command line:
//! flags override the file, and the file overrides the defaults (R6).
//!
//! ffetch often runs at shell startup, so a bad config never stops it. Each
//! problem costs one warning, and only what it is about falls back to its
//! default; a file that isn't TOML at all is ignored as a whole. A missing
//! file at the default location is fine and says nothing.
//!
//! `quip` is either a boolean or a table: TOML can't have `quip = true` and
//! `[[quip.rule]]` in one file, so with rules it is written
//! `[quip]` / `enabled = true` / `[[quip.rule]]`.

use std::{
    env,
    fmt::Display,
    fs, io,
    path::{self, Path, PathBuf},
};

use toml::{
    Spanned,
    de::{DeString, DeTable, DeValue},
};

use crate::{
    info,
    layout::{DEFAULT_LOGO_COLS, Layout, MIN_LOGO_COLS, Swatches},
    logo::Style,
    palette::{Background, Theme},
    quip,
    term::Rgb,
};

/// The words a setting takes, shared with the command line.
pub const LAYOUTS: &[(&str, Layout)] = &[
    ("auto", Layout::Auto),
    ("side", Layout::Side),
    ("stacked", Layout::Stacked),
];
pub const SWATCHES: &[(&str, Swatches)] = &[
    ("palette", Swatches::Palette),
    ("ansi", Swatches::Ansi),
    ("none", Swatches::None),
];
/// `None` is no logo.
pub const STYLES: &[(&str, Option<Style>)] = &[
    ("ascii", Some(Style::Ascii)),
    ("blocks", Some(Style::Blocks)),
    ("none", None),
];
const BACKGROUNDS: &[(&str, Background)] =
    &[("dark", Background::Dark), ("light", Background::Light)];

/// The value `word` stands for in `table`.
pub fn lookup<T: Copy>(table: &[(&str, T)], word: &str) -> Option<T> {
    table.iter().find(|(w, _)| *w == word).map(|&(_, v)| v)
}

/// The words of `table` for a message, each wrapped in `quote`: "auto, side
/// or stacked".
pub fn choices<T>(table: &[(&str, T)], quote: &str) -> String {
    let words: Vec<String> = table
        .iter()
        .map(|(w, _)| format!("{quote}{w}{quote}"))
        .collect();
    match words.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// The word for `value` in `table`.
fn word<T: PartialEq>(table: &[(&'static str, T)], value: &T) -> &'static str {
    table
        .iter()
        .find(|(_, v)| v == value)
        .map_or("", |(w, _)| w)
}

/// Everything the config file can set.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub layout: Layout,
    pub swatches: Swatches,
    /// Usage bars for memory, disk and battery.
    pub bars: bool,
    /// Whether to show a quip.
    pub quip: bool,
    /// The `[[quip.rule]]`s that parse, in order. They go before the built-in
    /// rules.
    pub quip_rules: Vec<QuipRule>,
    /// The modules to show, in order.
    pub modules: Vec<&'static str>,
    pub logo: Logo,
    pub theme: Theme,
}

/// `[logo]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Logo {
    /// `None` for no logo.
    pub style: Option<Style>,
    /// The width asked for. `None` is the default width, which shrinks to fit
    /// the terminal.
    pub size: Option<usize>,
    /// A PNG to use instead of the built-in logo.
    pub image: Option<PathBuf>,
    pub keep_background: bool,
}

/// A `[[quip.rule]]`, as written.
#[derive(Clone, Debug, PartialEq)]
pub struct QuipRule {
    /// The condition; `None` for a rule that always applies.
    pub when: Option<String>,
    pub say: Vec<String>,
}

impl QuipRule {
    fn rule(&self) -> Result<quip::Rule, quip::Error> {
        match &self.when {
            Some(when) => quip::Rule::new(when, &self.say),
            None => quip::Rule::always(&self.say),
        }
    }
}

impl Default for Config {
    fn default() -> Config {
        Config {
            layout: Layout::Auto,
            swatches: Swatches::Palette,
            bars: true,
            quip: true,
            quip_rules: Vec::new(),
            modules: info::default_modules(),
            logo: Logo {
                style: Some(Style::Blocks),
                size: None,
                image: None,
                keep_background: false,
            },
            theme: Theme::default(),
        }
    }
}

/// What the command line says about the settings in `Config`; `None` for
/// what it doesn't mention.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Flags {
    pub layout: Option<Layout>,
    pub swatches: Option<Swatches>,
    /// `Some(false)` for `--no-bars`.
    pub bars: Option<bool>,
    /// `Some(false)` for `--no-quip`.
    pub quip: Option<bool>,
    pub modules: Option<Vec<&'static str>>,
    /// `Some(None)` for `--logo none`.
    pub style: Option<Option<Style>>,
    pub size: Option<usize>,
    pub image: Option<PathBuf>,
    /// `Some(true)` for `--keep-background`.
    pub keep_background: Option<bool>,
}

impl Config {
    /// Overrides the settings the command line mentions.
    pub fn apply(&mut self, flags: Flags) {
        // Taken apart, so that a new flag can't be forgotten here.
        let Flags {
            layout,
            swatches,
            bars,
            quip,
            modules,
            style,
            size,
            image,
            keep_background,
        } = flags;
        self.layout = layout.unwrap_or(self.layout);
        self.swatches = swatches.unwrap_or(self.swatches);
        self.bars = bars.unwrap_or(self.bars);
        self.quip = quip.unwrap_or(self.quip);
        if let Some(modules) = modules {
            self.modules = modules;
        }
        self.logo.style = style.unwrap_or(self.logo.style);
        self.logo.size = size.or(self.logo.size);
        if image.is_some() {
            self.logo.image = image;
        }
        self.logo.keep_background = keep_background.unwrap_or(self.logo.keep_background);
    }

    /// The quip rules to go by: the user's, then the built-in ones.
    pub fn rules(&self) -> Vec<quip::Rule> {
        // The user's parsed when the file was read; the ones that didn't are gone.
        let user = self.quip_rules.iter().filter_map(|r| r.rule().ok());
        user.chain(quip::built_in()).collect()
    }

    /// The settings as a config file, which reads back as the same settings.
    /// Settings that are unset (and so not the same as any value) are
    /// commented out.
    pub fn to_toml(&self) -> String {
        let mut out = String::from(
            "# ffetch's settings in effect: the defaults, then the config file, then the\n\
             # command line. ffetch reads this back with --config.\n\n",
        );
        out += &format!("layout = {}\n", string(word(LAYOUTS, &self.layout)));
        out += &format!("swatches = {}\n", string(word(SWATCHES, &self.swatches)));
        out += &format!("bars = {}\n", self.bars);
        if self.quip_rules.is_empty() {
            out += &format!("quip = {}\n", self.quip);
        }
        out += &list("modules", &self.modules);

        out += "\n[logo]\n";
        out += &format!("style = {}\n", string(word(STYLES, &self.logo.style)));
        out += &match self.logo.size {
            Some(cols) => format!("size = {cols}\n"),
            None => format!("# size = {DEFAULT_LOGO_COLS}  # unset: shrinks to fit the terminal\n"),
        };
        out += &match &self.logo.image {
            // Absolute, so that it means the same wherever the file goes.
            Some(image) => {
                let image = path::absolute(image).unwrap_or_else(|_| image.clone());
                format!("image = {}\n", string(&image.to_string_lossy()))
            }
            None => "# image = \"~/Pictures/avatar.png\"  # unset: the built-in logo\n".into(),
        };
        out += &format!("keep_background = {}\n", self.logo.keep_background);

        out += "\n[theme]\n";
        out += &format!(
            "background = {}\n",
            string(word(BACKGROUNDS, &self.theme.background))
        );
        for (role, color) in [
            ("accent", self.theme.accent),
            ("secondary", self.theme.secondary),
            ("muted", self.theme.muted),
        ] {
            out += &match color {
                Some(color) => format!("{role} = {}\n", string(&color.hex())),
                None => format!("# {role} = \"#rrggbb\"  # unset: from the image\n"),
            };
        }

        if !self.quip_rules.is_empty() {
            out += &format!("\n[quip]\nenabled = {}\n", self.quip);
            for rule in &self.quip_rules {
                out += "\n[[quip.rule]]\n";
                if let Some(when) = &rule.when {
                    out += &format!("when = {}\n", string(when));
                }
                out += &list("say", &rule.say);
            }
        }
        out
    }
}

/// `s` as a TOML basic string.
fn string(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\n' => out += "\\n",
            '\r' => out += "\\r",
            '\t' => out += "\\t",
            c if c.is_control() => out += &format!("\\u{:04X}", c as u32),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `key = ["a", "b"]`, wrapped to 80 columns under the first item.
fn list(key: &str, items: &[impl AsRef<str>]) -> String {
    const WIDTH: usize = 80;
    let mut out = format!("{key} = [");
    let indent = out.len();
    let mut cols = indent;
    for (i, item) in items.iter().enumerate() {
        let mut item = string(item.as_ref());
        if i + 1 < items.len() {
            item.push(',');
        }
        let len = item.chars().count();
        if i > 0 {
            // Room for the space, the item and a closing bracket.
            if cols + 1 + len + 1 > WIDTH {
                out += &format!("\n{}", " ".repeat(indent));
                cols = indent;
            } else {
                out.push(' ');
                cols += 1;
            }
        }
        out += &item;
        cols += len;
    }
    out + "]\n"
}

/// `${XDG_CONFIG_HOME:-$HOME/.config}/ffetch/config.toml`, or `None` without
/// a usable base.
pub fn default_path() -> Option<PathBuf> {
    // The XDG spec says to ignore relative paths.
    let xdg = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    let base = xdg.or_else(|| Some(home()?.join(".config")))?;
    Some(base.join("ffetch").join("config.toml"))
}

/// `$HOME`, unless it is unset or empty.
fn home() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// The settings from the file named with `--config`, or else the one at
/// `default_path()`, over the defaults; and the warnings about it.
pub fn load(path: Option<&Path>) -> (Config, Vec<String>) {
    match (path, default_path()) {
        (Some(path), _) => read(path, true),
        (None, Some(path)) => read(&path, false),
        (None, None) => (Config::default(), Vec::new()),
    }
}

/// The settings in the file at `path`; it may be missing unless `named`
/// (with `--config`).
fn read(path: &Path, named: bool) -> (Config, Vec<String>) {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text, path, home().as_deref()),
        Err(e) if e.kind() == io::ErrorKind::NotFound && !named => (Config::default(), Vec::new()),
        Err(e) => (
            Config::default(),
            vec![format!(
                "can't read the config file {}: {e}; using the defaults",
                path.display()
            )],
        ),
    }
}

/// The settings in `text`, read from the file at `path`, and the warnings
/// about it in the order of the file. `~` in paths stands for `home`.
fn parse(text: &str, path: &Path, home: Option<&Path>) -> (Config, Vec<String>) {
    let mut config = Config::default();
    let mut reader = Reader {
        text,
        path,
        dir: path.parent().unwrap_or(Path::new("")),
        home,
        warnings: Vec::new(),
    };
    match DeTable::parse(text) {
        Ok(table) => reader.file(table.get_ref(), &mut config),
        Err(e) => {
            let message = e.message().replace('\n', " ");
            let span = e.span().unwrap_or(0..0);
            // SPEC.md's example has `quip = true` next to [[quip.rule]],
            // which TOML forbids; say what to write instead.
            let hint = match text.get(span.clone()) {
                Some("quip") if message.contains("boolean") => {
                    " (with [[quip.rule]], write [quip] and enabled = true instead of quip = true)"
                }
                _ => "",
            };
            reader.warn(span.start, format!("{message}{hint}; ignoring the file"));
        }
    }
    // The tables are sorted by key; the file's order reads better.
    reader.warnings.sort_by_key(|&(at, _)| at);
    let warnings = reader.warnings.into_iter().map(|(_, w)| w).collect();
    (config, warnings)
}

/// Goes through a parsed config file, taking what is valid and warning about
/// the rest.
struct Reader<'a> {
    text: &'a str,
    path: &'a Path,
    /// Where the file is: relative image paths start there.
    dir: &'a Path,
    home: Option<&'a Path>,
    /// Where in `text` (a byte offset), and what.
    warnings: Vec<(usize, String)>,
}

/// A value in the file, with where it is.
type Value<'a> = Spanned<DeValue<'a>>;

impl Reader<'_> {
    /// A warning about what is at the byte offset `at`: "path:line:column:
    /// message".
    fn warn(&mut self, at: usize, message: impl Display) {
        let before = self.text.get(..at).unwrap_or(self.text);
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        let warning = format!("{}:{line}:{column}: {message}", self.path.display());
        self.warnings.push((at, warning));
    }

    fn unknown(&mut self, key: &Spanned<DeString>, name: &str) {
        self.warn(
            key.span().start,
            format!("unknown key '{name}'; ignoring it"),
        );
    }

    /// Warns that `value`, the value of `key`, isn't what `expected` says.
    fn invalid(&mut self, key: &str, value: &Value, expected: &str) {
        let got = describe(value.get_ref());
        self.warn(
            value.span().start,
            format!("{key}: expected {expected}, got {got}; using the default"),
        );
    }

    /// Sets `slot` to the value `table` has for the word in `value`.
    fn choose<T: Copy>(&mut self, key: &str, value: &Value, table: &[(&str, T)], slot: &mut T) {
        match value.get_ref().as_str().and_then(|w| lookup(table, w)) {
            Some(v) => *slot = v,
            None => self.invalid(key, value, &choices(table, "\"")),
        }
    }

    fn boolean(&mut self, key: &str, value: &Value, slot: &mut bool) {
        match value.get_ref() {
            DeValue::Boolean(b) => *slot = *b,
            _ => self.invalid(key, value, "true or false"),
        }
    }

    fn table<'v>(&mut self, key: &str, value: &'v Value<'v>) -> Option<&'v DeTable<'v>> {
        match value.get_ref() {
            DeValue::Table(table) => Some(table),
            other => {
                let got = describe(other);
                let message = format!("{key}: expected a table, got {got}; using the defaults");
                self.warn(value.span().start, message);
                None
            }
        }
    }

    fn file(&mut self, file: &DeTable, config: &mut Config) {
        for (key, value) in file {
            match key.get_ref().as_ref() {
                "layout" => self.choose("layout", value, LAYOUTS, &mut config.layout),
                "swatches" => self.choose("swatches", value, SWATCHES, &mut config.swatches),
                "bars" => self.boolean("bars", value, &mut config.bars),
                "quip" => self.quip(value, config),
                "modules" => self.modules(value, &mut config.modules),
                "logo" => self.logo(value, &mut config.logo),
                "theme" => self.theme(value, &mut config.theme),
                name => self.unknown(key, name),
            }
        }
    }

    /// The module list. Unknown ids are skipped, as with `--modules`.
    fn modules(&mut self, value: &Value, modules: &mut Vec<&'static str>) {
        let DeValue::Array(items) = value.get_ref() else {
            return self.invalid("modules", value, "a list of module ids");
        };
        let mut ids = Vec::new();
        for item in items.iter() {
            let at = item.span().start;
            match item.get_ref() {
                DeValue::String(name) => match info::find(name) {
                    Some(def) if !ids.contains(&def.id) => ids.push(def.id),
                    Some(_) => {}
                    None => self.warn(at, format!("modules: unknown module '{name}'; skipping it")),
                },
                other => {
                    let got = describe(other);
                    self.warn(
                        at,
                        format!("modules: expected a module id, got {got}; skipping it"),
                    );
                }
            }
        }
        *modules = ids;
    }

    fn logo(&mut self, value: &Value, logo: &mut Logo) {
        let Some(table) = self.table("logo", value) else {
            return;
        };
        for (key, value) in table {
            match key.get_ref().as_ref() {
                "style" => self.choose("logo.style", value, STYLES, &mut logo.style),
                "size" => match columns(value.get_ref()) {
                    Some(cols) => logo.size = Some(cols),
                    None => {
                        let expected = format!("a width of at least {MIN_LOGO_COLS} columns");
                        self.invalid("logo.size", value, &expected);
                    }
                },
                "image" => match value.get_ref().as_str().filter(|p| !p.is_empty()) {
                    Some(image) => logo.image = Some(self.image_path(image)),
                    None => self.invalid("logo.image", value, "the path of a PNG image"),
                },
                "keep_background" => {
                    self.boolean("logo.keep_background", value, &mut logo.keep_background)
                }
                name => self.unknown(key, &format!("logo.{name}")),
            }
        }
    }

    /// An image path from the file: `~` is the home directory, and a relative
    /// path is relative to the config file.
    fn image_path(&self, path: &str) -> PathBuf {
        self.dir.join(expand_home(path, self.home))
    }

    fn theme(&mut self, value: &Value, theme: &mut Theme) {
        let Some(table) = self.table("theme", value) else {
            return;
        };
        for (key, value) in table {
            let name = key.get_ref().as_ref();
            let role = match name {
                "background" => {
                    self.choose(
                        "theme.background",
                        value,
                        BACKGROUNDS,
                        &mut theme.background,
                    );
                    continue;
                }
                "accent" => &mut theme.accent,
                "secondary" => &mut theme.secondary,
                "muted" => &mut theme.muted,
                name => {
                    self.unknown(key, &format!("theme.{name}"));
                    continue;
                }
            };
            match value.get_ref().as_str().and_then(Rgb::from_hex) {
                Some(color) => *role = Some(color),
                None => {
                    let key = format!("theme.{name}");
                    self.invalid(&key, value, "a color like \"#c33e58\"");
                }
            }
        }
    }

    /// `quip = false`, or a `[quip]` table with `enabled` and the rules.
    fn quip(&mut self, value: &Value, config: &mut Config) {
        let table = match value.get_ref() {
            DeValue::Boolean(on) => {
                config.quip = *on;
                return;
            }
            DeValue::Table(table) => table,
            _ => return self.invalid("quip", value, "true, false or a table"),
        };
        for (key, value) in table {
            match key.get_ref().as_ref() {
                "enabled" => self.boolean("quip.enabled", value, &mut config.quip),
                "rule" => {
                    let DeValue::Array(rules) = value.get_ref() else {
                        self.invalid("quip.rule", value, "[[quip.rule]] tables");
                        continue;
                    };
                    for (i, rule) in rules.iter().enumerate() {
                        config.quip_rules.extend(self.quip_rule(i + 1, rule));
                    }
                }
                name => self.unknown(key, &format!("quip.{name}")),
            }
        }
    }

    /// Rule number `n`, unless something is wrong with it.
    fn quip_rule(&mut self, n: usize, value: &Value) -> Option<QuipRule> {
        let rule = match self.read_rule(n, value) {
            Ok(rule) => rule,
            Err((at, problem)) => {
                self.warn(at, format!("quip rule {n}: {problem}; skipping it"));
                return None;
            }
        };
        for say in &rule.say {
            let (len, max) = (say.chars().count(), quip::MAX_CHARS);
            if len > max {
                let message = format!("{say:?} is {len} characters, more than the {max} that fit");
                self.warn(value.span().start, format!("quip rule {n}: {message}"));
            }
        }
        Some(rule)
    }

    /// The rule in `value`, or where (a byte offset) and what the problem is.
    fn read_rule(&mut self, n: usize, value: &Value) -> Result<QuipRule, (usize, String)> {
        let wrong = |value: &Value, expected: &str| {
            let got = describe(value.get_ref());
            (value.span().start, format!("{expected}, got {got}"))
        };
        let DeValue::Table(table) = value.get_ref() else {
            return Err(wrong(value, "expected a table"));
        };
        let mut rule = QuipRule {
            when: None,
            say: Vec::new(),
        };
        for (key, value) in table {
            match key.get_ref().as_ref() {
                "when" => {
                    let when = value.get_ref().as_str();
                    let when = when.ok_or_else(|| wrong(value, "when: expected a condition"))?;
                    rule.when = Some(when.into());
                }
                "say" => {
                    let say = strings(value.get_ref());
                    rule.say = say.ok_or_else(|| wrong(value, "say: expected a list of quips"))?;
                }
                name => self.warn(
                    key.span().start,
                    format!("quip rule {n}: unknown key '{name}'; ignoring it"),
                ),
            }
        }
        // At the rule; the error names the condition or quip it is about.
        rule.rule()
            .map_err(|e| (value.span().start, e.to_string()))?;
        Ok(rule)
    }
}

/// A logo width from the file.
fn columns(value: &DeValue) -> Option<usize> {
    let DeValue::Integer(n) = value else {
        return None;
    };
    let cols = u64::from_str_radix(n.as_str(), n.radix()).ok()?;
    usize::try_from(cols).ok().filter(|&c| c >= MIN_LOGO_COLS)
}

/// A list of strings from the file.
fn strings(value: &DeValue) -> Option<Vec<String>> {
    let DeValue::Array(items) = value else {
        return None;
    };
    items
        .iter()
        .map(|item| item.get_ref().as_str().map(str::to_string))
        .collect()
}

/// A value from the file, for a warning: strings quoted, numbers as written,
/// and the kind of anything bigger.
fn describe(value: &DeValue) -> String {
    match value {
        DeValue::String(s) => format!("{s:?}"),
        DeValue::Integer(n) => n.to_string(),
        DeValue::Float(x) => x.to_string(),
        DeValue::Boolean(b) => b.to_string(),
        DeValue::Datetime(_) => "a date".into(),
        DeValue::Array(_) => "a list".into(),
        DeValue::Table(_) => "a table".into(),
    }
}

/// `path` with a leading `~` (alone or before a `/`) standing for `home`.
fn expand_home(path: &str, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_path_buf(),
        (Some(rest), Some(home)) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quip::State;

    const PATH: &str = "/home/me/.config/ffetch/config.toml";
    const HOME: &str = "/home/me";

    /// The settings and warnings in `text`, as if read from `PATH`.
    fn read_str(text: &str) -> (Config, Vec<String>) {
        parse(text, Path::new(PATH), Some(Path::new(HOME)))
    }

    /// The settings in `text`, which must have no warnings.
    fn clean(text: &str) -> Config {
        let (config, warnings) = read_str(text);
        assert_eq!(warnings, Vec::<String>::new(), "{text}");
        config
    }

    /// The warnings for `text`, without the path.
    fn warnings(text: &str) -> Vec<String> {
        let prefix = format!("{PATH}:");
        read_str(text)
            .1
            .into_iter()
            .map(|w| w.strip_prefix(&prefix).unwrap().to_string())
            .collect()
    }

    fn rule(when: Option<&str>, say: &[&str]) -> QuipRule {
        QuipRule {
            when: when.map(str::to_string),
            say: say.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Everything set to something other than its default.
    const FULL: &str = r##"
layout = "stacked"
swatches = "ansi"
bars = false
modules = ["os", "gpu", "ip"]

[logo]
style = "blocks"
size = 30
image = "~/Pictures/avatar.png"
keep_background = true

[theme]
background = "light"
accent = "#C33E58"
secondary = "#00ff00"
muted = "#123456"

[quip]
enabled = false

[[quip.rule]]
when = "uptime_days >= 30"
say = ["a month. impressive. concerning."]

[[quip.rule]]
say = ["hi.", "hello."]
"##;

    fn full() -> Config {
        Config {
            layout: Layout::Stacked,
            swatches: Swatches::Ansi,
            bars: false,
            quip: false,
            quip_rules: vec![
                rule(
                    Some("uptime_days >= 30"),
                    &["a month. impressive. concerning."],
                ),
                rule(None, &["hi.", "hello."]),
            ],
            modules: vec!["os", "gpu", "ip"],
            logo: Logo {
                style: Some(Style::Blocks),
                size: Some(30),
                image: Some("/home/me/Pictures/avatar.png".into()),
                keep_background: true,
            },
            theme: Theme {
                background: Background::Light,
                accent: Some(Rgb(0xc3, 0x3e, 0x58)),
                secondary: Some(Rgb(0, 255, 0)),
                muted: Some(Rgb(0x12, 0x34, 0x56)),
            },
        }
    }

    #[test]
    fn an_empty_file_is_the_defaults() {
        assert_eq!(clean(""), Config::default());
        assert_eq!(clean("# just a comment\n\n"), Config::default());
        let defaults = Config::default();
        assert_eq!(
            (
                defaults.layout,
                defaults.swatches,
                defaults.bars,
                defaults.quip
            ),
            (Layout::Auto, Swatches::Palette, true, true)
        );
        assert_eq!(defaults.modules, info::default_modules());
        assert_eq!(defaults.logo.style, Some(Style::Blocks));
        assert_eq!(defaults.theme, Theme::default());
    }

    #[test]
    fn every_key() {
        assert_eq!(clean(FULL), full());
    }

    #[test]
    fn each_choice() {
        for (word, layout) in LAYOUTS {
            assert_eq!(clean(&format!("layout = {word:?}")).layout, *layout);
        }
        for (word, swatches) in SWATCHES {
            assert_eq!(clean(&format!("swatches = {word:?}")).swatches, *swatches);
        }
        for (word, style) in STYLES {
            assert_eq!(
                clean(&format!("[logo]\nstyle = {word:?}")).logo.style,
                *style
            );
        }
        for (word, background) in BACKGROUNDS {
            let text = format!("[theme]\nbackground = {word:?}");
            assert_eq!(clean(&text).theme.background, *background);
        }
        assert!(!clean("quip = false").quip);
        assert!(clean("quip = true").quip);
        assert!(clean("[quip]\nenabled = true").quip);
        assert!(!clean("quip = { enabled = false }").quip);
        assert!(clean("[logo]\nkeep_background = true").logo.keep_background);
        assert_eq!(clean("[logo]\nsize = 16").logo.size, Some(16));
        assert_eq!(clean("[logo]\nsize = 0x40").logo.size, Some(64));
        assert_eq!(clean("modules = []").modules, Vec::<&str>::new());
    }

    #[test]
    fn the_spec_example() {
        // As in SPEC.md F5.3, except `quip = true`: TOML doesn't allow it
        // next to [[quip.rule]], so `[quip] enabled = true` stands in.
        let text = r##"
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

[quip]
enabled = true

[[quip.rule]]
when = "uptime_days >= 30"
say = ["a month. impressive. concerning."]
"##;
        let config = clean(text);
        assert_eq!(config.modules.len(), 14);
        assert_eq!(config.logo.size, Some(48));
        assert!(config.quip);
        assert_eq!(config.quip_rules.len(), 1);

        // Verbatim, it isn't TOML: one warning, and the defaults.
        let verbatim = text
            .replace("[quip]\nenabled = true\n", "")
            .replace("bars = true\n", "bars = true\nquip = true\n");
        let (config, warnings) = read_str(&verbatim);
        assert_eq!(config, Config::default());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].ends_with(
                " (with [[quip.rule]], write [quip] and enabled = true instead of quip = true); \
                 ignoring the file"
            ),
            "{warnings:?}"
        );
    }

    #[test]
    fn home_and_relative_image_paths() {
        let image = |path: &str| clean(&format!("[logo]\nimage = {path:?}")).logo.image;
        assert_eq!(image("~/a.png"), Some("/home/me/a.png".into()));
        assert_eq!(image("~"), Some("/home/me".into()));
        assert_eq!(image("/abs/a.png"), Some("/abs/a.png".into()));
        // Next to the config file.
        assert_eq!(image("a.png"), Some("/home/me/.config/ffetch/a.png".into()));
        // Only the user's own home.
        assert_eq!(
            image("~bob/a.png"),
            Some("/home/me/.config/ffetch/~bob/a.png".into())
        );
        // Without a home directory, `~` is just a name.
        let (config, _) = parse(
            "[logo]\nimage = \"~/a.png\"",
            Path::new("/etc/ffetch.toml"),
            None,
        );
        assert_eq!(config.logo.image, Some("/etc/~/a.png".into()));
        assert_eq!(expand_home("~/x", None), PathBuf::from("~/x"));
        assert_eq!(
            expand_home("~/x", Some(Path::new("/h"))),
            PathBuf::from("/h/x")
        );
    }

    #[test]
    fn command_line_flags_win() {
        // For each key with a flag: the default, the file's value, and the
        // flag's over the file's.
        fn check<T: PartialEq + std::fmt::Debug>(
            file: &str,
            flags: Flags,
            get: fn(&Config) -> T,
            (default, from_file, from_flag): (T, T, T),
        ) {
            assert_eq!(get(&Config::default()), default, "default");
            let mut config = clean(file);
            assert_eq!(get(&config), from_file, "{file}");
            config.apply(Flags::default());
            assert_eq!(get(&config), from_file, "{file}, no flags");
            config.apply(flags.clone());
            assert_eq!(get(&config), from_flag, "{file}, {flags:?}");
            // A flag over the defaults, too.
            let mut config = Config::default();
            config.apply(flags);
            assert_eq!(get(&config), from_flag, "{file}");
        }
        check(
            "layout = \"side\"",
            Flags {
                layout: Some(Layout::Stacked),
                ..Flags::default()
            },
            |c| c.layout,
            (Layout::Auto, Layout::Side, Layout::Stacked),
        );
        check(
            "swatches = \"ansi\"",
            Flags {
                swatches: Some(Swatches::None),
                ..Flags::default()
            },
            |c| c.swatches,
            (Swatches::Palette, Swatches::Ansi, Swatches::None),
        );
        // --no-bars and --no-quip can only turn things off.
        check(
            "bars = true",
            Flags {
                bars: Some(false),
                ..Flags::default()
            },
            |c| c.bars,
            (true, true, false),
        );
        check(
            "quip = true",
            Flags {
                quip: Some(false),
                ..Flags::default()
            },
            |c| c.quip,
            (true, true, false),
        );
        check(
            "modules = [\"os\", \"kernel\"]",
            Flags {
                modules: Some(vec!["ip"]),
                ..Flags::default()
            },
            |c| c.modules.clone(),
            (info::default_modules(), vec!["os", "kernel"], vec!["ip"]),
        );
        check(
            "[logo]\nstyle = \"ascii\"",
            Flags {
                style: Some(None),
                ..Flags::default()
            },
            |c| c.logo.style,
            (Some(Style::Blocks), Some(Style::Ascii), None),
        );
        check(
            "[logo]\nsize = 30",
            Flags {
                size: Some(20),
                ..Flags::default()
            },
            |c| c.logo.size,
            (None, Some(30), Some(20)),
        );
        check(
            "[logo]\nimage = \"/a.png\"",
            Flags {
                image: Some("b.png".into()),
                ..Flags::default()
            },
            |c| c.logo.image.clone(),
            (None, Some("/a.png".into()), Some("b.png".into())),
        );
        check(
            "[logo]\nkeep_background = false",
            Flags {
                keep_background: Some(true),
                ..Flags::default()
            },
            |c| c.logo.keep_background,
            (false, false, true),
        );
        // What the command line doesn't mention stays as the file says.
        let mut config = clean(FULL);
        config.apply(Flags {
            layout: Some(Layout::Auto),
            ..Flags::default()
        });
        assert_eq!(
            config,
            Config {
                layout: Layout::Auto,
                ..full()
            }
        );
    }

    #[test]
    fn syntax_errors_ignore_the_file() {
        for (text, warning) in [
            (
                "layout = \"auto\nbars = true\n",
                "1:15: invalid basic string, expected `\"`; ignoring the file",
            ),
            (
                "bars = true\nbars = false\n",
                "2:1: duplicate key; ignoring the file",
            ),
            (
                "layout = \"side\"\nx = \n",
                "2:5: string values must be quoted, expected literal string; ignoring the file",
            ),
        ] {
            let (config, _) = read_str(text);
            assert_eq!(config, Config::default(), "{text:?}");
            assert_eq!(warnings(text), [warning], "{text:?}");
        }
    }

    #[test]
    fn unknown_keys_warn_once_each() {
        let text = r##"
colour = "red"
layout = "side"

[logo]
style = "ascii"
colour = "red"
width = 3

[theme]
foreground = "#ffffff"

[quip]
enabled = true
often = true

[[quip.rule]]
say = ["hi."]
weight = 2

[extra]
x = 1
"##;
        assert_eq!(
            warnings(text),
            [
                "2:1: unknown key 'colour'; ignoring it",
                "7:1: unknown key 'logo.colour'; ignoring it",
                "8:1: unknown key 'logo.width'; ignoring it",
                "11:1: unknown key 'theme.foreground'; ignoring it",
                "15:1: unknown key 'quip.often'; ignoring it",
                "19:1: quip rule 1: unknown key 'weight'; ignoring it",
                "21:2: unknown key 'extra'; ignoring it",
            ]
        );
        // The rest still counts.
        let (config, _) = read_str(text);
        assert_eq!(config.layout, Layout::Side);
        assert_eq!(config.quip_rules, [rule(None, &["hi."])]);
    }

    #[test]
    fn invalid_values_fall_back_to_their_defaults() {
        let text = r##"
layout = "diagonal"
swatches = 3
bars = "yes"
modules = ["os", "bogus", 7, "kernel", "os"]

[logo]
style = "fancy"
size = 3
image = ""
keep_background = 1

[theme]
background = "grey"
accent = "red"
secondary = "#12345"
muted = "#00ff00"
"##;
        let expected = "; using the default";
        assert_eq!(
            warnings(text),
            [
                format!(
                    "2:10: layout: expected \"auto\", \"side\" or \"stacked\", got \"diagonal\"{expected}"
                ),
                format!(
                    "3:12: swatches: expected \"palette\", \"ansi\" or \"none\", got 3{expected}"
                ),
                format!("4:8: bars: expected true or false, got \"yes\"{expected}"),
                "5:18: modules: unknown module 'bogus'; skipping it".into(),
                "5:27: modules: expected a module id, got 7; skipping it".into(),
                format!(
                    "8:9: logo.style: expected \"ascii\", \"blocks\" or \"none\", got \"fancy\"{expected}"
                ),
                format!("9:8: logo.size: expected a width of at least 16 columns, got 3{expected}"),
                format!("10:9: logo.image: expected the path of a PNG image, got \"\"{expected}"),
                format!("11:19: logo.keep_background: expected true or false, got 1{expected}"),
                format!(
                    "14:14: theme.background: expected \"dark\" or \"light\", got \"grey\"{expected}"
                ),
                format!(
                    "15:10: theme.accent: expected a color like \"#c33e58\", got \"red\"{expected}"
                ),
                format!(
                    "16:13: theme.secondary: expected a color like \"#c33e58\", got \"#12345\"{expected}"
                ),
            ]
        );
        let (config, _) = read_str(text);
        assert_eq!(
            config,
            Config {
                modules: vec!["os", "kernel"],
                theme: Theme {
                    muted: Some(Rgb(0, 255, 0)),
                    ..Theme::default()
                },
                ..Config::default()
            }
        );
        // Whole tables of the wrong kind.
        assert_eq!(
            warnings("logo = \"big\"\ntheme = [1]\nquip = \"sometimes\"\nmodules = \"os\"\n"),
            [
                "1:8: logo: expected a table, got \"big\"; using the defaults",
                "2:9: theme: expected a table, got a list; using the defaults",
                "3:8: quip: expected true, false or a table, got \"sometimes\"; using the default",
                "4:11: modules: expected a list of module ids, got \"os\"; using the default",
            ]
        );
        for size in ["15", "-48", "48.0", "\"48\"", "99999999999999999999999"] {
            let text = format!("[logo]\nsize = {size}");
            let (config, warnings) = read_str(&text);
            assert_eq!(config.logo.size, None, "{size}");
            assert_eq!(warnings.len(), 1, "{size}");
        }
    }

    #[test]
    fn bad_quip_rules_are_skipped() {
        let long = "x".repeat(41);
        let text = format!(
            r#"
[[quip.rule]]
when = "uptime >= 30"
say = ["up a while."]

[[quip.rule]]
when = "hour < 5"
say = ["it's {{clok}}."]

[[quip.rule]]
when = "hour < 5"

[[quip.rule]]
when = 5
say = ["five."]

[[quip.rule]]
say = "just one."

[[quip.rule]]
say = ["{long}", "fine."]

[[quip.rule]]
when = "root == 1"
say = ["kept."]
"#
        );
        assert_eq!(
            warnings(&text),
            [
                "2:1: quip rule 1: unknown name 'uptime' at column 1 of \"uptime >= 30\"; skipping it",
                "6:1: quip rule 2: unknown placeholder '{clok}' at column 6 of \"it's {clok}.\"; skipping it",
                "10:1: quip rule 3: no templates; skipping it",
                "14:8: quip rule 4: when: expected a condition, got 5; skipping it",
                "18:7: quip rule 5: say: expected a list of quips, got \"just one.\"; skipping it",
                &format!(
                    "20:1: quip rule 6: \"{long}\" is 41 characters, more than the 40 that fit"
                ),
            ]
        );
        let (config, _) = read_str(&text);
        assert_eq!(
            config.quip_rules,
            [
                rule(None, &[&long, "fine."]),
                rule(Some("root == 1"), &["kept."])
            ]
        );
        assert!(config.quip, "rules don't turn quips off");
        assert_eq!(
            warnings("quip = { rule = \"often\" }"),
            ["1:17: quip.rule: expected [[quip.rule]] tables, got \"often\"; using the default"]
        );
        assert_eq!(
            warnings("quip = { rule = [1] }"),
            ["1:18: quip rule 1: expected a table, got 1; skipping it"]
        );
    }

    #[test]
    fn user_quip_rules_come_first() {
        let config = clean(
            "[[quip.rule]]\nwhen = \"uptime_days >= 30\"\nsay = [\"a month. impressive. concerning.\"]\n",
        );
        let rules = config.rules();
        assert_eq!(rules.len(), quip::built_in().len() + 1);
        let month = State {
            uptime_days: Some(45),
            ..State::default()
        };
        assert_eq!(
            quip::pick(&rules, &month, 0).as_deref(),
            Some("a month. impressive. concerning.")
        );
        // A week: the user's rule doesn't apply, so the built-in one does.
        let week = State {
            uptime_days: Some(8),
            ..State::default()
        };
        let quip = quip::pick(&rules, &week, 0).unwrap();
        assert!(quip.contains('8'), "{quip}");
        assert_eq!(Config::default().rules(), quip::built_in());
    }

    #[test]
    fn printed_config_reads_back_the_same() {
        let tricky = rule(
            Some("hour < 5 and root == 1"),
            &[
                "quote \" backslash \\ tab \t",
                "ünïcödé ™ \u{7f}\u{1}",
                "{packages} pkgs.",
            ],
        );
        let mut many_modules = full();
        many_modules.modules = info::MODULES.iter().map(|m| m.id).collect();
        many_modules.quip = true;
        many_modules.quip_rules.push(tricky);
        let no_logo = Config {
            logo: Logo {
                style: None,
                ..Config::default().logo
            },
            modules: Vec::new(),
            quip: false,
            ..Config::default()
        };
        for config in [Config::default(), full(), many_modules, no_logo] {
            let text = config.to_toml();
            assert_eq!(clean(&text), config, "{text}");
            assert!(text.lines().all(|l| l.chars().count() <= 80), "{text}");
        }
    }

    #[test]
    fn printed_defaults() {
        let text = Config::default().to_toml();
        for line in [
            "layout = \"auto\"",
            "swatches = \"palette\"",
            "bars = true",
            "quip = true",
            "modules = [\"os\", \"host\", \"windows\", \"kernel\", \"uptime\", \"packages\", \"shell\",",
            "[logo]",
            "style = \"blocks\"",
            "# size = 48  # unset: shrinks to fit the terminal",
            "keep_background = false",
            "[theme]",
            "background = \"dark\"",
            "# accent = \"#rrggbb\"  # unset: from the image",
        ] {
            assert!(text.lines().any(|l| l == line), "{line:?} missing:\n{text}");
        }
        assert!(!text.contains("[quip]"), "{text}");
        // A relative image from the command line is printed absolute.
        let mut config = Config::default();
        config.apply(Flags {
            image: Some("assets/logo.png".into()),
            ..Flags::default()
        });
        let cwd = env::current_dir().unwrap();
        let printed = format!(
            "image = {}",
            string(&cwd.join("assets/logo.png").to_string_lossy())
        );
        assert!(config.to_toml().contains(&printed), "{}", config.to_toml());
    }

    #[test]
    fn toml_strings() {
        assert_eq!(string("plain"), "\"plain\"");
        assert_eq!(string("a\"b\\c\nd\te"), r#""a\"b\\c\nd\te""#);
        assert_eq!(string("\u{1}\u{7f}é"), "\"\\u0001\\u007Fé\"");
        assert_eq!(list("say", &["a"]), "say = [\"a\"]\n");
        assert_eq!(list("modules", &[] as &[&str]), "modules = []\n");
    }

    /// A fresh, empty directory for one test.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("ffetch-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_and_unreadable_files() {
        let dir = temp_dir("config-files");
        let missing = dir.join("config.toml");
        // At the default location: silent.
        assert_eq!(read(&missing, false), (Config::default(), Vec::new()));
        // Named with --config: one warning.
        let (config, warnings) = read(&missing, true);
        assert_eq!(config, Config::default());
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].starts_with(&format!(
                "can't read the config file {}: ",
                missing.display()
            )) && warnings[0].ends_with("; using the defaults"),
            "{warnings:?}"
        );
        // A directory, or not UTF-8: one warning either way.
        for (named, path) in [(false, dir.clone()), (true, dir.clone())] {
            assert_eq!(read(&path, named).1.len(), 1, "{path:?}");
        }
        let binary = dir.join("binary.toml");
        fs::write(&binary, b"layout = \"\xff\"").unwrap();
        assert_eq!(read(&binary, false).1.len(), 1);
        // A real file.
        fs::write(&missing, "layout = \"side\"\nimage = 1\n").unwrap();
        let (config, warnings) = read(&missing, false);
        assert_eq!(config.layout, Layout::Side);
        assert_eq!(
            warnings,
            [format!(
                "{}:2:1: unknown key 'image'; ignoring it",
                missing.display()
            )]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn choice_lists() {
        assert_eq!(choices(LAYOUTS, ""), "auto, side or stacked");
        assert_eq!(choices(BACKGROUNDS, "\""), "\"dark\" or \"light\"");
        assert_eq!(choices(&[("one", 1)], ""), "one");
        assert_eq!(lookup(STYLES, "none"), Some(None));
        assert_eq!(lookup(STYLES, "None"), None);
    }
}

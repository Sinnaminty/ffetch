//! `--format` and `--oneline`: one line of plain text from a template, with
//! `{module}` placeholders for a module's text and `{module.field}` ones for
//! a field of its value (`Report::field`). Only the modules the template
//! names run, so a prompt or status bar pays for nothing else.

use std::{fmt, str::FromStr};

use crate::info::{self, Ctx, Info, Report};

/// What `--oneline` stands for.
pub const ONELINE: &str = "{os} · up {uptime} · mem {memory.pct}% · disk {disk.pct}%";

/// A parsed template. Every module and field in it exists.
#[derive(Clone, Debug, PartialEq)]
pub struct Template(Vec<Piece>);

#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Text(String),
    /// A module's text, its lines joined with ", ".
    Module(&'static str),
    /// A field of a module's value: (module, field).
    Field(&'static str, &'static str),
}

/// Why a template doesn't parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    /// Where in the template, in characters from 1.
    pub column: usize,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} at column {}", self.message, self.column)
    }
}

impl FromStr for Template {
    type Err = Error;

    /// Parses text with `{module}` and `{module.field}` placeholders; `{{`
    /// and `}}` stand for literal braces. Unknown modules and fields are
    /// errors, so a typo shows up before anything runs.
    fn from_str(src: &str) -> Result<Template, Error> {
        // At the byte offset `at`.
        let error = |message: String, at: usize| Error {
            message,
            column: src[..at].chars().count() + 1,
        };
        let mut pieces = Vec::new();
        let mut text = String::new();
        let mut rest = src.char_indices().peekable();
        while let Some((at, c)) = rest.next() {
            let doubled = matches!(c, '{' | '}') && rest.next_if(|&(_, next)| next == c).is_some();
            match c {
                '{' | '}' if doubled => text.push(c),
                '}' => {
                    return Err(error("unmatched '}' (write '}}' for a brace)".into(), at));
                }
                '{' => {
                    let inner = &src[at + 1..];
                    let len = inner
                        .find(['{', '}'])
                        .filter(|&i| inner[i..].starts_with('}'))
                        .ok_or_else(|| error("unclosed '{'".into(), at))?;
                    if !text.is_empty() {
                        pieces.push(Piece::Text(std::mem::take(&mut text)));
                    }
                    pieces.push(placeholder(&inner[..len]).map_err(|m| error(m, at))?);
                    // Past the placeholder and its closing brace.
                    while rest.next_if(|&(i, _)| i <= at + 1 + len).is_some() {}
                }
                c => text.push(c),
            }
        }
        if !text.is_empty() {
            pieces.push(Piece::Text(text));
        }
        Ok(Template(pieces))
    }
}

/// The placeholder `name` (between the braces), or what is wrong with it.
fn placeholder(name: &str) -> Result<Piece, String> {
    if name.is_empty() {
        return Err("empty placeholder '{}'".into());
    }
    let (id, field) = match name.split_once('.') {
        Some((id, field)) => (id, Some(field)),
        None => (name, None),
    };
    let def = info::find(id).ok_or_else(|| format!("unknown module '{id}' in '{{{name}}}'"))?;
    let Some(field) = field else {
        return Ok(Piece::Module(def.id));
    };
    match def.fields.iter().find(|&&f| f == field) {
        Some(field) => Ok(Piece::Field(def.id, field)),
        None => Err(format!(
            "module '{id}' has no field '{field}' (it has {})",
            def.fields.join(", ")
        )),
    }
}

impl Template {
    /// The modules the template shows, in the order they first appear.
    pub fn modules(&self) -> Vec<&'static str> {
        let mut ids = Vec::new();
        for piece in &self.0 {
            if let Piece::Module(id) | Piece::Field(id, _) = piece
                && !ids.contains(id)
            {
                ids.push(*id);
            }
        }
        ids
    }

    /// The template filled in from `info`. A placeholder whose module had
    /// nothing to report (or whose field is unknown for this value) is left
    /// empty.
    pub fn render(&self, info: &Info) -> String {
        let mut out = String::new();
        for piece in &self.0 {
            match piece {
                Piece::Text(text) => out += text,
                Piece::Module(id) => {
                    if let Some(value) = info.get(id) {
                        out += &value.display().lines().collect::<Vec<_>>().join(", ");
                    }
                }
                Piece::Field(id, name) => {
                    if let Some(field) = info.get(id).and_then(|v| v.field(name)) {
                        out += &field.to_string();
                    }
                }
            }
        }
        out
    }

    /// Runs the template's modules, and only those, on `ctx` and fills the
    /// template in.
    pub fn line(&self, ctx: &Ctx) -> String {
        self.render(&info::collect(ctx, &self.modules()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Template {
        src.parse()
            .unwrap_or_else(|e| panic!("{src:?} doesn't parse: {e}"))
    }

    fn text(s: &str) -> Piece {
        Piece::Text(s.into())
    }

    #[test]
    fn placeholders_and_text() {
        assert_eq!(
            parse("{os} · up {uptime} · mem {memory.pct}%").0,
            [
                Piece::Module("os"),
                text(" · up "),
                Piece::Module("uptime"),
                text(" · mem "),
                Piece::Field("memory", "pct"),
                text("%"),
            ]
        );
        assert_eq!(parse("{git.branch}").0, [Piece::Field("git", "branch")]);
        assert_eq!(parse("plain text").0, [text("plain text")]);
        assert_eq!(parse("aa  bb..").0, [text("aa  bb..")], "repeats stay");
        assert_eq!(parse("").0, []);
        assert_eq!(
            parse("{os}{kernel}").0,
            [Piece::Module("os"), Piece::Module("kernel")]
        );
        // Not Unicode-shy.
        assert_eq!(
            parse("☕ {cpu.ghz}GHz").0,
            [text("☕ "), Piece::Field("cpu", "ghz"), text("GHz")]
        );
    }

    #[test]
    fn doubled_braces_are_literal() {
        assert_eq!(
            parse("{{os}} is {os}, {{}}").0,
            [text("{os} is "), Piece::Module("os"), text(", {}")]
        );
        assert_eq!(
            parse("{{{os}}}").0,
            [text("{"), Piece::Module("os"), text("}")]
        );
        assert_eq!(parse("}}{{").0, [text("}{")]);
    }

    #[test]
    fn the_oneline_preset_parses() {
        assert_eq!(parse(ONELINE).modules(), ["os", "uptime", "memory", "disk"]);
    }

    #[test]
    fn modules_in_order_of_first_use() {
        assert_eq!(
            parse("{git.branch} {disk} {git.changed} {os} {disk.pct}").modules(),
            ["git", "disk", "os"]
        );
        assert!(parse("no placeholders").modules().is_empty());
    }

    #[test]
    fn errors() {
        for (src, message, column) in [
            ("{bogus}", "unknown module 'bogus' in '{bogus}'", 1),
            ("up {uptme}", "unknown module 'uptme' in '{uptme}'", 4),
            ("é {Os}", "unknown module 'Os' in '{Os}'", 3),
            (
                "{memory.percent}",
                "module 'memory' has no field 'percent' (it has used_bytes, total_bytes, pct)",
                1,
            ),
            (
                "{git.}",
                "module 'git' has no field '' (it has branch, ahead, behind, changed)",
                1,
            ),
            (
                "{memory.pct.x}",
                "module 'memory' has no field 'pct.x' (it has used_bytes, total_bytes, pct)",
                1,
            ),
            ("{.pct}", "unknown module '' in '{.pct}'", 1),
            ("{}", "empty placeholder '{}'", 1),
            ("{ os }", "unknown module ' os ' in '{ os }'", 1),
            ("mem {memory.pct", "unclosed '{'", 5),
            ("{", "unclosed '{'", 1),
            ("{os {kernel}", "unclosed '{'", 1),
            ("{os}}", "unmatched '}' (write '}}' for a brace)", 5),
            ("a } b", "unmatched '}' (write '}}' for a brace)", 3),
        ] {
            let e = src.parse::<Template>().expect_err(src);
            assert_eq!((e.message.as_str(), e.column), (message, column), "{src:?}");
        }
        let e = "{bogus}".parse::<Template>().unwrap_err();
        assert_eq!(
            e.to_string(),
            "unknown module 'bogus' in '{bogus}' at column 1"
        );
    }
}

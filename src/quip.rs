//! Quips: a one-line remark from the character about the machine, in a speech
//! bubble under the info column.
//!
//! A rule is a condition and a few templates. Conditions are written in a tiny
//! language: `<name> <op> <number>` clauses joined with `and`, such as
//! `battery < 15 and battery_discharging == 1`, over a flat set of names
//! backed by the modules' data (`State`). The built-in rules are written in it
//! too, so rules from the config file just go in front of them. The first rule
//! that applies wins, and one of its templates is picked at random.

use std::{
    fmt, iter,
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    info::{Ctx, Field, Info, Report},
    layout,
    rng::SplitMix64,
    term::{ColorMode, Rgb},
};

/// The names conditions can test and templates can show; see `State`.
const NAMES: &[&str] = &[
    "battery",
    "battery_discharging",
    "mem_pct",
    "disk_pct",
    "load1",
    "threads",
    "load_ratio",
    "uptime_days",
    "hour",
    "root",
    "packages",
];

/// A placeholder for templates only: `hour` on a 12-hour clock, e.g. "2am".
const CLOCK: &str = "clock";

/// The built-in rules, highest priority first: a condition and what to say.
/// Each quip is at most 40 characters with its placeholders filled in.
const BUILT_IN: &[(&str, &[&str])] = &[
    (
        "battery < 15 and battery_discharging == 1",
        &[
            "{battery}% battery. living dangerously.",
            "plug me in.",
            "{battery}% left. find a charger.",
        ],
    ),
    (
        "mem_pct >= 90",
        &[
            "{mem_pct}% ram. i'm drowning.",
            "memory's at {mem_pct}%. close a tab.",
            "{mem_pct}% memory used. i can't think.",
        ],
    ),
    (
        "disk_pct >= 90",
        &[
            "disk's {disk_pct}% full. delete something.",
            "disk at {disk_pct}%. no room for anything.",
        ],
    ),
    (
        // More waiting work than threads to run it.
        "load_ratio > 1",
        &[
            "load {load1} on {threads} threads. breathe.",
            "load {load1}. one thing at a time.",
            "everything at once? load {load1}.",
        ],
    ),
    (
        "uptime_days >= 7",
        &[
            "{uptime_days} days without a reboot. bold.",
            "up {uptime_days} days. i could use a nap.",
            "{uptime_days} days up. reboot me already.",
        ],
    ),
    (
        // 00:00-04:59.
        "hour < 5",
        &[
            "it's {clock}. go to sleep.",
            "it's {clock}. let me sleep.",
            "{clock}. nothing good happens now.",
        ],
    ),
    (
        "root == 1",
        &["running a fetch as root. sure.", "root? for a fetch? fine."],
    ),
    (
        "packages > 3000",
        &[
            "{packages} packages. i'm stuffed.",
            "{packages} packages. i'm full.",
            "{packages} packages and counting. ugh.",
        ],
    ),
];

/// The longest a quip should be, in characters, placeholders filled in.
pub const MAX_CHARS: usize = 40;

/// What to say when no other rule applies.
const FALLBACK: &[&str] = &[
    "what.",
    "fine. here are your stats.",
    "don't screenshot me.",
];

/// What quips know about the machine. Each field is a name in conditions and
/// templates. `None` means undefined: its module didn't run or had nothing to
/// report. A condition that tests an undefined name is false, and a template
/// that shows one isn't used.
///
/// One more name is derived: `load_ratio`, `load1 / threads`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// The first battery's charge, in percent.
    pub battery: Option<u64>,
    /// Whether that battery is discharging (1 or 0 in conditions).
    pub battery_discharging: Option<bool>,
    /// Memory in use, in percent.
    pub mem_pct: Option<u64>,
    /// Space used on `/`, in percent.
    pub disk_pct: Option<u64>,
    /// The 1-minute load average.
    pub load1: Option<f64>,
    /// Logical CPUs.
    pub threads: Option<u64>,
    /// Whole days since the boot.
    pub uptime_days: Option<u64>,
    /// The hour of the local time, 0-23.
    pub hour: Option<u8>,
    /// Whether ffetch runs as root (1 or 0 in conditions).
    pub root: Option<bool>,
    /// Installed packages, all package managers together.
    pub packages: Option<u64>,
}

impl State {
    /// What the modules in `info` found, the local hour and whether the user
    /// is root.
    pub fn new(info: &Info, ctx: &Ctx) -> State {
        let field = |id: &str, name: &str| info.get(id)?.field(name);
        let int = |id: &str, name: &str| match field(id, name)? {
            Field::Int(n) => Some(n),
            _ => None,
        };
        State {
            battery: int("battery", "pct"),
            battery_discharging: field("battery", "status")
                .map(|s| matches!(s, Field::Text(s) if s == "Discharging")),
            mem_pct: int("memory", "pct"),
            disk_pct: int("disk", "pct"),
            load1: match field("load", "load1") {
                Some(Field::Float(x)) => Some(x),
                _ => None,
            },
            threads: int("load", "threads").or_else(|| int("cpu", "threads")),
            uptime_days: int("uptime", "days"),
            hour: ctx.local_hour(),
            root: Some(ctx.user.is_root()),
            packages: int("packages", "total"),
        }
    }

    /// The number `name` stands for in conditions; `None` if it is undefined.
    fn value(&self, name: &str) -> Option<f64> {
        let int = |n: Option<u64>| n.map(|n| n as f64);
        let flag = |b: Option<bool>| b.map(|b| f64::from(u8::from(b)));
        match name {
            "battery" => int(self.battery),
            "battery_discharging" => flag(self.battery_discharging),
            "mem_pct" => int(self.mem_pct),
            "disk_pct" => int(self.disk_pct),
            "load1" => self.load1,
            "threads" => int(self.threads),
            "load_ratio" => Some(self.load1? / self.threads.filter(|&t| t > 0)? as f64),
            "uptime_days" => int(self.uptime_days),
            "hour" => self.hour.map(f64::from),
            "root" => flag(self.root),
            "packages" => int(self.packages),
            _ => None,
        }
    }

    /// How the placeholder `name` reads in a quip; `None` if it is undefined.
    /// Package counts get thousands separators and loads one decimal; other
    /// numbers are whole.
    fn show(&self, name: &str) -> Option<String> {
        match name {
            CLOCK => self.hour.map(clock),
            "packages" => self.packages.map(thousands),
            "load1" | "load_ratio" => Some(format!("{:.1}", self.value(name)?)),
            _ => Some((self.value(name)? as u64).to_string()),
        }
    }
}

/// `hour` (0-23) on a 12-hour clock: "12am", "2am", "12pm", "11pm".
fn clock(hour: u8) -> String {
    let suffix = if hour < 12 { "am" } else { "pm" };
    match hour % 12 {
        0 => format!("12{suffix}"),
        h => format!("{h}{suffix}"),
    }
}

/// `n` with commas between the thousands: "3,412".
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Why a rule doesn't parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// What is wrong, e.g. "unknown operator '=>'".
    pub message: String,
    /// The condition or template it is in; empty for a rule without templates.
    pub text: String,
    /// Where in `text`, in characters from 1.
    pub column: usize,
}

impl Error {
    fn new(message: impl Into<String>, text: &str, column: usize) -> Error {
        Error {
            message: message.into(),
            text: text.to_string(),
            column,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.message)?;
        if !self.text.is_empty() {
            write!(f, " at column {} of {:?}", self.column, self.text)?;
        }
        Ok(())
    }
}

/// How a clause compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl Op {
    fn parse(s: &str) -> Option<Op> {
        Some(match s {
            "<" => Op::Lt,
            "<=" => Op::Le,
            ">" => Op::Gt,
            ">=" => Op::Ge,
            "==" => Op::Eq,
            "!=" => Op::Ne,
            _ => return None,
        })
    }

    fn holds(self, a: f64, b: f64) -> bool {
        match self {
            Op::Lt => a < b,
            Op::Le => a <= b,
            Op::Gt => a > b,
            Op::Ge => a >= b,
            Op::Eq => a == b,
            Op::Ne => a != b,
        }
    }
}

/// `<name> <op> <number>`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Clause {
    name: &'static str,
    op: Op,
    number: f64,
}

/// Clauses that must all hold. With none, it always holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Condition(Vec<Clause>);

impl Condition {
    /// Whether every clause holds; one with an undefined name doesn't.
    fn holds(&self, state: &State) -> bool {
        self.0.iter().all(|c| {
            state
                .value(c.name)
                .is_some_and(|value| c.op.holds(value, c.number))
        })
    }
}

/// What a token of a condition is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A name, or `and`.
    Word,
    Number,
    /// A run of `<>=!`, which may not be an operator.
    Op,
    /// Any other character.
    Other,
}

#[derive(Clone, Copy, Debug)]
struct Token<'a> {
    kind: Kind,
    text: &'a str,
    /// In characters from 1.
    column: usize,
}

/// Splits a condition into tokens. Whitespace only separates them.
fn tokens(src: &str) -> Vec<Token<'_>> {
    let chars: Vec<(usize, char)> = src.char_indices().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while let Some(&(start, c)) = chars.get(i) {
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let next = chars.get(i + 1).map(|&(_, c)| c);
        let kind = match c {
            'a'..='z' | 'A'..='Z' | '_' => Kind::Word,
            '0'..='9' | '.' => Kind::Number,
            '-' if next.is_some_and(|n| n.is_ascii_digit() || n == '.') => Kind::Number,
            '<' | '>' | '=' | '!' => Kind::Op,
            _ => Kind::Other,
        };
        let goes_on = |c: char| match kind {
            Kind::Word => c.is_ascii_alphanumeric() || c == '_',
            Kind::Number => c.is_ascii_digit() || c == '.',
            Kind::Op => "<>=!".contains(c),
            Kind::Other => false,
        };
        let len = 1 + chars[i + 1..]
            .iter()
            .take_while(|&&(_, c)| goes_on(c))
            .count();
        let end = chars.get(i + len).map_or(src.len(), |&(b, _)| b);
        tokens.push(Token {
            kind,
            text: &src[start..end],
            column: i + 1,
        });
        i += len;
    }
    tokens
}

/// "'x'" for a token, "the end" past the last one.
fn found(token: Option<Token>) -> String {
    token.map_or("the end".into(), |t| format!("'{}'", t.text))
}

impl FromStr for Condition {
    type Err = Error;

    /// Parses `<name> <op> <number>` clauses joined with `and`. The names are
    /// those of `State`; the operators `< <= > >= == !=`.
    fn from_str(src: &str) -> Result<Condition, Error> {
        // At `token`, or just past the end.
        let error = |message: String, token: Option<Token>| {
            let column = token.map_or(src.chars().count() + 1, |t| t.column);
            Error::new(message, src, column)
        };
        let mut tokens = tokens(src).into_iter();
        let mut clauses = Vec::new();
        loop {
            let word = match tokens.next() {
                Some(t) if t.kind == Kind::Word && t.text != "and" => t,
                None if clauses.is_empty() => return Err(Error::new("empty condition", src, 1)),
                t => {
                    let after = if clauses.is_empty() {
                        ""
                    } else {
                        " after 'and'"
                    };
                    return Err(error(
                        format!("expected a name{after}, found {}", found(t)),
                        t,
                    ));
                }
            };
            let name = NAMES
                .iter()
                .copied()
                .find(|&n| n == word.text)
                .ok_or_else(|| error(format!("unknown name '{}'", word.text), Some(word)))?;
            let (op, op_token) = match tokens.next() {
                Some(t) if t.kind == Kind::Op => match Op::parse(t.text) {
                    Some(op) => (op, t),
                    None => return Err(error(format!("unknown operator '{}'", t.text), Some(t))),
                },
                t => {
                    let message = format!(
                        "expected an operator (< <= > >= == !=) after '{}', found {}",
                        word.text,
                        found(t)
                    );
                    return Err(error(message, t));
                }
            };
            let number = match tokens.next() {
                Some(t) if t.kind == Kind::Number => t
                    .text
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .ok_or_else(|| error(format!("bad number '{}'", t.text), Some(t)))?,
                t => {
                    let message = format!(
                        "expected a number after '{}', found {}",
                        op_token.text,
                        found(t)
                    );
                    return Err(error(message, t));
                }
            };
            clauses.push(Clause { name, op, number });
            match tokens.next() {
                None => return Ok(Condition(clauses)),
                Some(t) if t.text == "and" => {}
                t => return Err(error(format!("expected 'and', found {}", found(t)), t)),
            }
        }
    }
}

/// A quip with `{name}` placeholders.
#[derive(Clone, Debug, PartialEq)]
struct Template(Vec<Part>);

#[derive(Clone, Debug, PartialEq)]
enum Part {
    Text(String),
    Name(&'static str),
}

impl FromStr for Template {
    type Err = Error;

    /// Parses text with `{name}` placeholders: the names of `State`, and
    /// `clock`.
    fn from_str(src: &str) -> Result<Template, Error> {
        // At the byte offset `at`.
        let error =
            |message: String, at: usize| Error::new(message, src, src[..at].chars().count() + 1);
        if src.trim().is_empty() {
            return Err(error("empty template".into(), 0));
        }
        let mut parts = Vec::new();
        // Where the text not yet parsed starts.
        let mut rest = 0;
        while let Some(open) = src[rest..].find('{').map(|i| rest + i) {
            if open > rest {
                parts.push(Part::Text(src[rest..open].into()));
            }
            let close = src[open..]
                .find('}')
                .map(|i| open + i)
                .ok_or_else(|| error("unclosed '{'".into(), open))?;
            let name = &src[open + 1..close];
            let name = NAMES
                .iter()
                .chain(&[CLOCK])
                .copied()
                .find(|&n| n == name)
                .ok_or_else(|| error(format!("unknown placeholder '{{{name}}}'"), open))?;
            parts.push(Part::Name(name));
            rest = close + 1;
        }
        if rest < src.len() {
            parts.push(Part::Text(src[rest..].into()));
        }
        Ok(Template(parts))
    }
}

impl Template {
    /// The quip, with the placeholders filled in from `state`; `None` if one
    /// of them is undefined.
    fn fill(&self, state: &State) -> Option<String> {
        let mut out = String::new();
        for part in &self.0 {
            match part {
                Part::Text(text) => out += text,
                Part::Name(name) => out += &state.show(name)?,
            }
        }
        Some(out)
    }
}

/// A condition, and what to say when it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    when: Condition,
    say: Vec<Template>,
}

impl Rule {
    /// A rule that applies when the condition `when` holds, and says one of
    /// `say`.
    pub fn new(when: &str, say: &[impl AsRef<str>]) -> Result<Rule, Error> {
        Rule::with(when.parse()?, say)
    }

    /// A rule that always applies, like the fallback.
    pub fn always(say: &[impl AsRef<str>]) -> Result<Rule, Error> {
        Rule::with(Condition(Vec::new()), say)
    }

    fn with(when: Condition, say: &[impl AsRef<str>]) -> Result<Rule, Error> {
        if say.is_empty() {
            return Err(Error::new("no templates", "", 1));
        }
        let say = say
            .iter()
            .map(|s| s.as_ref().parse())
            .collect::<Result<_, _>>()?;
        Ok(Rule { when, say })
    }
}

/// The built-in rules in priority order, the fallback last.
pub fn built_in() -> Vec<Rule> {
    let rules = BUILT_IN.iter().map(|(when, say)| Rule::new(when, say));
    // The tests make sure they parse.
    rules
        .chain([Rule::always(FALLBACK)])
        .collect::<Result<_, _>>()
        .expect("the built-in quip rules parse")
}

/// The first rule in `rules` that applies to `state` and has templates it can
/// fill in: its index, and those templates filled in.
fn winner(rules: &[Rule], state: &State) -> Option<(usize, Vec<String>)> {
    rules.iter().enumerate().find_map(|(i, rule)| {
        if !rule.when.holds(state) {
            return None;
        }
        let quips: Vec<String> = rule.say.iter().filter_map(|t| t.fill(state)).collect();
        (!quips.is_empty()).then_some((i, quips))
    })
}

/// The quip for `state`: one of the templates of the first rule in `rules`
/// that applies, picked with `seed`. `None` if no rule applies.
pub fn pick(rules: &[Rule], state: &State, seed: u64) -> Option<String> {
    let (_, mut quips) = winner(rules, state)?;
    let i = SplitMix64(seed).below(quips.len());
    Some(quips.swap_remove(i))
}

/// A seed that changes from run to run: the clock, in nanoseconds.
pub fn clock_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// The pieces of a bubble's border.
struct Border {
    horizontal: char,
    /// Top left, top right, bottom left, bottom right.
    corners: [char; 4],
    side: &'static str,
    /// The left side, with the tail sticking out of it.
    tail: &'static str,
}

const ROUND: Border = Border {
    horizontal: '─',
    corners: ['╭', '╮', '╰', '╯'],
    side: "│",
    tail: "─┤",
};

const ASCII: Border = Border {
    horizontal: '-',
    corners: ['+'; 4],
    side: "|",
    tail: "-|",
};

/// Below this width, a bubble whose text had to be cut short isn't worth
/// showing.
const MIN_BUBBLE_COLS: usize = 12;

/// The columns a bubble adds to its text: a border and a space on each side,
/// and the tail.
fn frame_cols(tail: bool) -> usize {
    4 + usize::from(tail)
}

/// How wide the bubble around `text` is.
pub fn bubble_cols(text: &str, tail: bool) -> usize {
    text.chars().count() + frame_cols(tail)
}

/// The speech bubble around `text`, as three lines; with `tail`, it points
/// left at a logo beside it. The border is drawn in `color`, or in ASCII
/// without colour; the text keeps the default colour. Text too long for
/// `max_cols` is cut short with "…", and `None` means that would leave a
/// bubble narrower than `MIN_BUBBLE_COLS`.
pub fn bubble(
    text: &str,
    tail: bool,
    max_cols: Option<usize>,
    mode: ColorMode,
    color: Rgb,
) -> Option<Vec<String>> {
    let text = match max_cols {
        Some(max) if bubble_cols(text, tail) > max => {
            if max < MIN_BUBBLE_COLS {
                return None;
            }
            layout::truncate(text, Some(max - frame_cols(tail)))
        }
        _ => text.to_string(),
    };
    let border = match mode {
        ColorMode::None => ASCII,
        _ => ROUND,
    };
    let paint = |s: &str| mode.tint(s, color);
    let line: String = iter::repeat_n(border.horizontal, text.chars().count() + 2).collect();
    let [top_left, top_right, bottom_left, bottom_right] = border.corners;
    let (indent, left) = match tail {
        true => (" ", border.tail),
        false => ("", border.side),
    };
    Some(vec![
        format!("{indent}{}", paint(&format!("{top_left}{line}{top_right}"))),
        format!("{} {text} {}", paint(left), paint(border.side)),
        format!(
            "{indent}{}",
            paint(&format!("{bottom_left}{line}{bottom_right}"))
        ),
    ])
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::term::RESET;

    fn cond(src: &str) -> Condition {
        src.parse()
            .unwrap_or_else(|e| panic!("{src:?} doesn't parse: {e}"))
    }

    fn clause(name: &'static str, op: Op, number: f64) -> Clause {
        Clause { name, op, number }
    }

    #[test]
    fn conditions_parse() {
        assert_eq!(
            cond("uptime_days >= 30"),
            Condition(vec![clause("uptime_days", Op::Ge, 30.0)])
        );
        for (src, op) in [
            ("<", Op::Lt),
            ("<=", Op::Le),
            (">", Op::Gt),
            (">=", Op::Ge),
            ("==", Op::Eq),
            ("!=", Op::Ne),
        ] {
            assert_eq!(cond(&format!("hour {src} 5")).0, [clause("hour", op, 5.0)]);
        }
        assert_eq!(
            cond("battery < 15 and battery_discharging == 1 and hour != 3"),
            Condition(vec![
                clause("battery", Op::Lt, 15.0),
                clause("battery_discharging", Op::Eq, 1.0),
                clause("hour", Op::Ne, 3.0),
            ])
        );
        // Spaces are optional between tokens, and any amount will do.
        assert_eq!(cond("load1>2.5"), cond("  load1 \t >   2.5  "));
        assert_eq!(cond("hour<5 and root==1"), cond("hour < 5  and\troot == 1"));
        // Decimals, with or without a leading digit, and negative numbers.
        assert_eq!(cond("load_ratio > 1.5").0[0].number, 1.5);
        assert_eq!(cond("load1 >= .5").0[0].number, 0.5);
        assert_eq!(cond("load1 > -1").0[0].number, -1.0);
    }

    #[test]
    fn condition_errors() {
        let ops = "expected an operator (< <= > >= == !=)";
        for (src, message, column) in [
            ("", "empty condition".to_string(), 1),
            ("   ", "empty condition".into(), 1),
            ("hour => 5", "unknown operator '=>'".into(), 6),
            ("hour = 5", "unknown operator '='".into(), 6),
            ("hour <> 5", "unknown operator '<>'".into(), 6),
            ("hour ~ 5", format!("{ops} after 'hour', found '~'"), 6),
            ("hour 5", format!("{ops} after 'hour', found '5'"), 6),
            ("hour", format!("{ops} after 'hour', found the end"), 5),
            (
                "hour <",
                "expected a number after '<', found the end".into(),
                7,
            ),
            (
                "hour < and root == 1",
                "expected a number after '<', found 'and'".into(),
                8,
            ),
            (
                "hour < five",
                "expected a number after '<', found 'five'".into(),
                8,
            ),
            ("hour < 1.2.3", "bad number '1.2.3'".into(), 8),
            ("hour < .", "bad number '.'".into(), 8),
            (
                "hour < 5 root == 1",
                "expected 'and', found 'root'".into(),
                10,
            ),
            (
                "hour < 5 or root == 1",
                "expected 'and', found 'or'".into(),
                10,
            ),
            ("hour < 5)", "expected 'and', found ')'".into(), 9),
            ("hour < 5days", "expected 'and', found 'days'".into(), 9),
            (
                "hour < 5 and",
                "expected a name after 'and', found the end".into(),
                13,
            ),
            (
                "hour < 5 and and root == 1",
                "expected a name after 'and', found 'and'".into(),
                14,
            ),
            ("and hour < 5", "expected a name, found 'and'".into(), 1),
            ("< 5", "expected a name, found '<'".into(), 1),
            ("uptime < 5", "unknown name 'uptime'".into(), 1),
            ("hour < 5 and Root == 1", "unknown name 'Root'".into(), 14),
            // Only templates can use it.
            ("clock < 5", "unknown name 'clock'".into(), 1),
        ] {
            let e = src.parse::<Condition>().expect_err(src);
            assert_eq!(
                (e.message.as_str(), e.column),
                (message.as_str(), column),
                "{src:?}"
            );
        }
        let e = "hour => 5".parse::<Condition>().unwrap_err();
        assert_eq!(e.text, "hour => 5");
        assert_eq!(
            e.to_string(),
            r#"unknown operator '=>' at column 6 of "hour => 5""#
        );
    }

    #[test]
    fn evaluation() {
        let state = State {
            battery: Some(9),
            battery_discharging: Some(true),
            hour: Some(3),
            ..State::default()
        };
        let holds = |src: &str| cond(src).holds(&state);
        for (src, expected) in [
            ("battery < 15", true),
            ("battery < 9", false),
            ("battery <= 9", true),
            ("battery > 9", false),
            ("battery >= 9", true),
            ("battery == 9", true),
            ("battery != 9", false),
            ("battery_discharging == 1", true),
            ("battery < 15 and battery_discharging == 1", true),
            ("battery < 15 and hour > 3", false),
        ] {
            assert_eq!(holds(src), expected, "{src}");
        }
        // An undefined name makes the condition false, whatever the operator.
        for op in ["<", "<=", ">", ">=", "==", "!="] {
            assert!(!holds(&format!("mem_pct {op} 50")), "mem_pct {op} 50");
        }
        assert!(!holds("battery < 15 and mem_pct != 50"));

        // load_ratio needs both the load and a thread count.
        let load = |load1, threads| State {
            load1,
            threads,
            ..State::default()
        };
        let busy = cond("load_ratio > 1");
        assert!(busy.holds(&load(Some(17.0), Some(16))));
        assert!(!busy.holds(&load(Some(16.0), Some(16))));
        assert!(!busy.holds(&load(Some(17.0), None)));
        assert!(!busy.holds(&load(Some(17.0), Some(0))));
        assert!(!busy.holds(&load(None, Some(16))));
    }

    /// A machine with nothing to remark on.
    fn calm() -> State {
        State {
            battery: Some(80),
            battery_discharging: Some(true),
            mem_pct: Some(40),
            disk_pct: Some(50),
            load1: Some(0.5),
            threads: Some(16),
            uptime_days: Some(2),
            hour: Some(14),
            root: Some(false),
            packages: Some(1200),
        }
    }

    /// The priority (1-9, as in the spec's table) of the built-in rule that
    /// wins for `state`.
    fn priority(state: &State) -> usize {
        winner(&built_in(), state)
            .expect("the fallback always wins")
            .0
            + 1
    }

    #[test]
    fn each_built_in_rule() {
        for (state, expected) in [
            (
                State {
                    battery: Some(14),
                    ..calm()
                },
                1,
            ),
            (
                State {
                    mem_pct: Some(90),
                    ..calm()
                },
                2,
            ),
            (
                State {
                    disk_pct: Some(95),
                    ..calm()
                },
                3,
            ),
            (
                State {
                    load1: Some(16.5),
                    ..calm()
                },
                4,
            ),
            (
                State {
                    uptime_days: Some(7),
                    ..calm()
                },
                5,
            ),
            (
                State {
                    hour: Some(0),
                    ..calm()
                },
                6,
            ),
            (
                State {
                    hour: Some(4),
                    ..calm()
                },
                6,
            ),
            (
                State {
                    root: Some(true),
                    ..calm()
                },
                7,
            ),
            (
                State {
                    packages: Some(3001),
                    ..calm()
                },
                8,
            ),
            (calm(), 9),
            (State::default(), 9),
        ] {
            assert_eq!(priority(&state), expected, "{state:?}");
        }
    }

    #[test]
    fn built_in_rules_just_short_of_matching() {
        for state in [
            State {
                battery: Some(15),
                ..calm()
            },
            State {
                battery: Some(5),
                battery_discharging: Some(false),
                ..calm()
            },
            State {
                battery: Some(5),
                battery_discharging: None,
                ..calm()
            },
            State {
                mem_pct: Some(89),
                ..calm()
            },
            State {
                disk_pct: Some(89),
                ..calm()
            },
            State {
                load1: Some(16.0),
                ..calm()
            },
            State {
                uptime_days: Some(6),
                ..calm()
            },
            State {
                hour: Some(5),
                ..calm()
            },
            State {
                hour: Some(23),
                ..calm()
            },
            State {
                packages: Some(3000),
                ..calm()
            },
        ] {
            assert_eq!(priority(&state), 9, "{state:?}");
        }
    }

    #[test]
    fn priority_order() {
        // Everything is wrong at once. Fix one thing at a time, from the top
        // priority down, and the next rule takes over.
        let mut state = State {
            battery: Some(3),
            battery_discharging: Some(true),
            mem_pct: Some(97),
            disk_pct: Some(99),
            load1: Some(40.0),
            threads: Some(8),
            uptime_days: Some(30),
            hour: Some(3),
            root: Some(true),
            packages: Some(5000),
        };
        let fixes: [fn(&mut State); 8] = [
            |s| s.battery_discharging = Some(false),
            |s| s.mem_pct = Some(50),
            |s| s.disk_pct = Some(50),
            |s| s.load1 = Some(1.0),
            |s| s.uptime_days = Some(0),
            |s| s.hour = Some(12),
            |s| s.root = Some(false),
            |s| s.packages = Some(10),
        ];
        for (i, fix) in fixes.iter().enumerate() {
            assert_eq!(priority(&state), i + 1, "{state:?}");
            fix(&mut state);
        }
        assert_eq!(priority(&state), 9);
    }

    #[test]
    fn templates_only_need_what_their_condition_does() {
        // For each built-in rule, a state with only what its condition tests.
        let needs = [
            State {
                battery: Some(9),
                battery_discharging: Some(true),
                ..State::default()
            },
            State {
                mem_pct: Some(95),
                ..State::default()
            },
            State {
                disk_pct: Some(95),
                ..State::default()
            },
            State {
                load1: Some(20.0),
                threads: Some(4),
                ..State::default()
            },
            State {
                uptime_days: Some(10),
                ..State::default()
            },
            State {
                hour: Some(2),
                ..State::default()
            },
            State {
                root: Some(true),
                ..State::default()
            },
            State {
                packages: Some(4000),
                ..State::default()
            },
            State::default(),
        ];
        let rules = built_in();
        assert_eq!(rules.len(), needs.len());
        for (i, (rule, state)) in rules.iter().zip(&needs).enumerate() {
            let (won, quips) = winner(&rules, state).unwrap();
            assert_eq!(won, i, "{state:?}");
            assert_eq!(quips.len(), rule.say.len(), "rule {}: {quips:?}", i + 1);
        }
    }

    #[test]
    fn user_rules_come_first() {
        let mut rules =
            vec![Rule::new("uptime_days >= 30", &["a month. impressive. concerning."]).unwrap()];
        rules.extend(built_in());
        let month = State {
            uptime_days: Some(45),
            ..calm()
        };
        assert_eq!(
            pick(&rules, &month, 0).as_deref(),
            Some("a month. impressive. concerning.")
        );
        // Ahead of every built-in, whatever its priority.
        let flat = State {
            battery: Some(5),
            ..month
        };
        assert_eq!(winner(&rules, &flat).unwrap().0, 0);
        // When it doesn't match, the built-ins carry on as usual: rule 5,
        // after the user's.
        let week = State {
            uptime_days: Some(10),
            ..calm()
        };
        assert_eq!(winner(&rules, &week).unwrap().0, 5);

        // A rule whose templates can't be filled in is passed over.
        let rules = [
            Rule::new("hour < 5", &["{packages} packages at {clock}."]).unwrap(),
            Rule::always(&["fine."]).unwrap(),
        ];
        let night = State {
            hour: Some(2),
            ..State::default()
        };
        assert_eq!(pick(&rules, &night, 0).as_deref(), Some("fine."));
        let night = State {
            packages: Some(5),
            ..night
        };
        assert_eq!(
            pick(&rules, &night, 0).as_deref(),
            Some("5 packages at 2am.")
        );
        assert_eq!(pick(&[], &night, 0), None);
    }

    #[test]
    fn template_choice_is_seeded() {
        let rules = built_in();
        let state = State::default();
        let quips: BTreeSet<String> = (0..64)
            .map(|seed| pick(&rules, &state, seed).unwrap())
            .collect();
        let fallback: BTreeSet<String> = FALLBACK.iter().map(|s| s.to_string()).collect();
        assert_eq!(quips, fallback, "every template gets picked");
        for seed in [0, 1, 0xdead_beef] {
            assert_eq!(pick(&rules, &state, seed), pick(&rules, &state, seed));
        }
    }

    fn fill(template: &str, state: &State) -> Option<String> {
        let template: Template = template.parse().unwrap();
        template.fill(state)
    }

    #[test]
    fn placeholders() {
        let at = |hour| State {
            hour: Some(hour),
            ..State::default()
        };
        for (hour, clock) in [
            (0, "12am"),
            (2, "2am"),
            (11, "11am"),
            (12, "12pm"),
            (13, "1pm"),
            (23, "11pm"),
        ] {
            assert_eq!(
                fill("it's {clock}.", &at(hour)),
                Some(format!("it's {clock}."))
            );
        }
        assert_eq!(fill("{hour}h", &at(7)).as_deref(), Some("7h"));

        let packages = |n| State {
            packages: Some(n),
            ..State::default()
        };
        for (n, shown) in [
            (0, "0"),
            (999, "999"),
            (1000, "1,000"),
            (3412, "3,412"),
            (999_999, "999,999"),
            (1_234_567, "1,234,567"),
        ] {
            assert_eq!(fill("{packages}", &packages(n)).as_deref(), Some(shown));
        }

        let load = |load1| State {
            load1: Some(load1),
            threads: Some(2),
            ..State::default()
        };
        for (load1, shown) in [
            (2.31, "2.3"),
            (2.36, "2.4"),
            (16.0, "16.0"),
            (999.94, "999.9"),
        ] {
            assert_eq!(fill("{load1}", &load(load1)).as_deref(), Some(shown));
        }
        assert_eq!(fill("{load_ratio}", &load(3.0)).as_deref(), Some("1.5"));

        // Integers as they are.
        let state = State {
            uptime_days: Some(99_999),
            battery: Some(9),
            mem_pct: Some(91),
            disk_pct: Some(100),
            threads: Some(16),
            ..State::default()
        };
        assert_eq!(
            fill(
                "{uptime_days} {battery}% {mem_pct}% {disk_pct}% {threads}",
                &state
            )
            .as_deref(),
            Some("99999 9% 91% 100% 16")
        );
        // Without a value, the template can't be used.
        assert_eq!(fill("{packages} packages.", &State::default()), None);
        assert_eq!(
            fill("no placeholders.", &State::default()).as_deref(),
            Some("no placeholders.")
        );
    }

    #[test]
    fn template_errors() {
        for (src, message, column) in [
            ("", "empty template", 1),
            ("  ", "empty template", 1),
            ("{packges} packages.", "unknown placeholder '{packges}'", 1),
            ("load {load_1}.", "unknown placeholder '{load_1}'", 6),
            ("{}", "unknown placeholder '{}'", 1),
            ("it's {clock", "unclosed '{'", 6),
        ] {
            let e = src.parse::<Template>().expect_err(src);
            assert_eq!((e.message.as_str(), e.column), (message, column), "{src:?}");
        }
        let no_templates: &[&str] = &[];
        let e = Rule::new("hour < 5", no_templates).unwrap_err();
        assert_eq!(e.to_string(), "no templates");
        // Errors say which text they are in.
        let e = Rule::new("hour < 5", &["fine.", "it's {clok}."]).unwrap_err();
        assert_eq!(
            e.to_string(),
            r#"unknown placeholder '{clok}' at column 6 of "it's {clok}.""#
        );
        let e = Rule::new("hour >> 5", &["fine."]).unwrap_err();
        assert_eq!(
            e.to_string(),
            r#"unknown operator '>>' at column 6 of "hour >> 5""#
        );
    }

    /// Every value as wide as it gets.
    fn extreme(hour: u8) -> State {
        State {
            battery: Some(100),
            battery_discharging: Some(true),
            mem_pct: Some(100),
            disk_pct: Some(100),
            load1: Some(999.9),
            threads: Some(99_999),
            uptime_days: Some(99_999),
            hour: Some(hour),
            root: Some(true),
            packages: Some(999_999),
        }
    }

    #[test]
    fn built_in_quips_are_short_and_lowercase() {
        let rules = built_in();
        for (i, rule) in rules.iter().enumerate() {
            assert!((2..=3).contains(&rule.say.len()), "rule {}", i + 1);
            for hour in 0..24 {
                for template in &rule.say {
                    let quip = template.fill(&extreme(hour)).unwrap();
                    let len = quip.chars().count();
                    assert!(len <= MAX_CHARS, "rule {}: {quip:?} is {len} long", i + 1);
                    assert_eq!(quip, quip.to_lowercase(), "rule {}", i + 1);
                }
            }
        }
    }

    #[test]
    fn every_name_is_defined_on_a_full_state() {
        let state = extreme(3);
        for name in NAMES {
            assert!(state.value(name).is_some(), "{name}");
            assert!(state.show(name).is_some(), "{name}");
        }
        assert_eq!(state.show("clock").as_deref(), Some("3am"));
        assert_eq!(state.value("nope"), None);
    }

    const BORDER: Rgb = Rgb(1, 2, 3);

    fn plain(text: &str, tail: bool, max_cols: Option<usize>) -> Option<Vec<String>> {
        bubble(text, tail, max_cols, ColorMode::None, BORDER)
    }

    #[test]
    fn bubble_beside_and_above_the_logo() {
        assert_eq!(
            plain("what.", true, None).unwrap(),
            [" +-------+", "-| what. |", " +-------+"]
        );
        assert_eq!(
            plain("what.", false, None).unwrap(),
            ["+-------+", "| what. |", "+-------+"]
        );
        let m = "\x1b[38;2;1;2;3m";
        assert_eq!(
            bubble("what.", true, None, ColorMode::TrueColor, BORDER).unwrap(),
            [
                format!(" {m}╭───────╮{RESET}"),
                format!("{m}─┤{RESET} what. {m}│{RESET}"),
                format!(" {m}╰───────╯{RESET}"),
            ]
        );
        assert_eq!(
            bubble("what.", false, None, ColorMode::TrueColor, BORDER).unwrap(),
            [
                format!("{m}╭───────╮{RESET}"),
                format!("{m}│{RESET} what. {m}│{RESET}"),
                format!("{m}╰───────╯{RESET}"),
            ]
        );
        let ansi256 = bubble("what.", true, None, ColorMode::Ansi256, BORDER).unwrap();
        assert!(ansi256[1].starts_with("\x1b[38;5;"), "{ansi256:?}");
        assert_eq!(bubble_cols("what.", true), 10);
        assert_eq!(bubble_cols("what.", false), 9);
    }

    #[test]
    fn bubble_truncation() {
        let text = "11 days without a reboot. bold.";
        assert_eq!(bubble_cols(text, true), 36);
        // Exactly as wide as allowed: whole.
        assert_eq!(
            plain(text, true, Some(36)).unwrap()[1],
            "-| 11 days without a reboot. bold. |"
        );
        let narrow = plain(text, true, Some(20)).unwrap();
        assert_eq!(narrow[1], "-| 11 days withou… |");
        assert!(narrow.iter().all(|l| l.chars().count() == 20), "{narrow:?}");
        let narrow = plain(text, false, Some(20)).unwrap();
        assert_eq!(narrow[1], "| 11 days without… |");
        // The narrowest bubble worth showing, and none narrower.
        assert_eq!(plain(text, true, Some(12)).unwrap()[1], "-| 11 day… |");
        assert_eq!(plain(text, true, Some(11)), None);
        assert_eq!(plain(text, false, Some(0)), None);
        // A short quip that fits is shown, however narrow.
        assert!(plain("what.", true, Some(10)).is_some());
    }
}

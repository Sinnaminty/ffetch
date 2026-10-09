//! Modules for developers: the git status of the working directory, toolchain
//! versions and running containers.

use std::{process::Command, thread, time::Duration};

use super::{
    Ctx,
    value::{Containers, Git, Tool, Toolchains, Value},
};

/// `git status` gets this long; in a huge repository it's hidden instead.
const GIT_TIMEOUT: Duration = Duration::from_millis(50);
const DOCKER_SOCKET: &str = "/var/run/docker.sock";
/// HTTP/1.0, so the reply is never chunked and ends when the connection closes.
/// Without `?all=1` only running containers are listed.
const DOCKER_REQUEST: &[u8] = b"GET /containers/json HTTP/1.0\r\nHost: docker\r\n\r\n";
const DOCKER_TIMEOUT: Duration = Duration::from_millis(100);

pub fn git(ctx: &Ctx) -> Option<Value> {
    let mut cmd = Command::new("git");
    // Without optional locks git status doesn't refresh the index, so killing
    // it at the timeout can't leave a stale index.lock behind.
    cmd.args([
        "--no-optional-locks",
        "status",
        "--porcelain=v2",
        "--branch",
    ])
    .current_dir(ctx.cwd()?);
    // Outside a repository git fails, and the module is hidden.
    let out = ctx
        .run_within(cmd, GIT_TIMEOUT)
        .filter(|o| o.status.success())?;
    parse_status(&String::from_utf8_lossy(&out.stdout)).map(Value::Git)
}

/// Parses `git status --porcelain=v2 --branch`: `# branch.*` headers, then a
/// line per changed (`1`, `2`), unmerged (`u`) or untracked (`?`) file.
fn parse_status(out: &str) -> Option<Git> {
    let (mut oid, mut head, mut ahead_behind, mut changed) = (None, None, None, 0);
    for line in out.lines() {
        let Some(header) = line.strip_prefix("# ") else {
            // Ignored files (`!`) only show with --ignored, but aren't changes.
            if !line.is_empty() && !line.starts_with('!') {
                changed += 1;
            }
            continue;
        };
        match header.split_once(' ') {
            Some(("branch.oid", value)) => oid = Some(value),
            Some(("branch.head", value)) => head = Some(value),
            // "+1 -0"
            Some(("branch.ab", value)) => {
                let mut counts = value.split(' ');
                let ahead = counts.next()?.strip_prefix('+')?.parse().ok()?;
                let behind = counts.next()?.strip_prefix('-')?.parse().ok()?;
                ahead_behind = Some((ahead, behind));
            }
            _ => {}
        }
    }
    let branch = match head? {
        "(detached)" => oid?.get(..7)?.to_string(),
        branch => branch.to_string(),
    };
    Some(Git {
        branch,
        ahead_behind,
        changed,
    })
}

/// (name shown, program asked for `--version`)
const TOOLCHAINS: [(&str, &str); 3] = [("rust", "rustc"), ("node", "node"), ("python", "python3")];

pub fn toolchains(ctx: &Ctx) -> Option<Value> {
    // Each takes a few to 30 ms (rustc through rustup), so all at once.
    let tools: Vec<Tool> = thread::scope(|s| {
        let handles: Vec<_> = TOOLCHAINS
            .iter()
            .map(|&(name, program)| {
                s.spawn(move || {
                    let mut cmd = Command::new(program);
                    cmd.arg("--version");
                    // rustup would otherwise install a toolchain pinned by a
                    // rust-toolchain.toml in the working directory.
                    cmd.env("RUSTUP_AUTO_INSTALL", "0");
                    let out = ctx.run(cmd).filter(|o| o.status.success())?;
                    let version = tool_version(&String::from_utf8_lossy(&out.stdout))?;
                    Some(Tool { name, version })
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok().flatten())
            .collect()
    });
    (!tools.is_empty()).then_some(Value::Toolchains(Toolchains { tools }))
}

/// The version in the first line of `--version` output: the first word that
/// starts with a digit (after an optional `v`) and has a dot in it. For
/// "rustc 1.98.1 (48a229cea 2026-09-01)", "v22.11.0" or "Python 3.12.3".
fn tool_version(out: &str) -> Option<String> {
    let version = out
        .lines()
        .next()?
        .split_whitespace()
        .map(|w| w.strip_prefix('v').unwrap_or(w))
        .find(|w| w.starts_with(|c: char| c.is_ascii_digit()) && w.contains('.'))?;
    Some(version.to_string())
}

/// Running containers, asked of the Docker daemon over its socket (starting
/// the `docker` client would take longer than the whole fetch).
pub fn docker(ctx: &Ctx) -> Option<Value> {
    let reply = ctx.unix_request(DOCKER_SOCKET, DOCKER_REQUEST, DOCKER_TIMEOUT)?;
    let running = running_containers(&reply)?;
    Some(Value::Docker(Containers { running }))
}

/// The number of containers in the daemon's reply to `GET /containers/json`.
/// `None` unless it is a complete 200 response holding a JSON array; a chunked
/// body isn't decoded (an HTTP/1.0 request doesn't get one).
fn running_containers(reply: &[u8]) -> Option<usize> {
    let end = reply.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&reply[..end]).ok()?;
    let mut body = &reply[end + 4..];
    let mut lines = head.split("\r\n");
    let mut status = lines.next()?.split(' ');
    if !status.next()?.starts_with("HTTP/1.") || status.next()? != "200" {
        return None;
    }
    for line in lines {
        let (name, value) = line.split_once(':')?;
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("transfer-encoding") && !value.eq_ignore_ascii_case("identity")
        {
            return None;
        }
        if name.eq_ignore_ascii_case("content-length") {
            body = body.get(..value.parse().ok()?)?;
        }
    }
    json_array_len(std::str::from_utf8(body).ok()?)
}

/// The number of elements of the JSON array `json`. Only the nesting is
/// checked, not the rest of the grammar: enough to count Docker's objects
/// and to reject a reply that was cut short.
fn json_array_len(json: &str) -> Option<usize> {
    let inner = json.trim().strip_prefix('[')?.strip_suffix(']')?;
    let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
    let (mut commas, mut empty) = (0, true);
    for c in inner.chars() {
        empty &= c.is_whitespace();
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '[' | '{' => depth += 1,
            ']' | '}' => depth = depth.checked_sub(1)?,
            ',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    if in_string || depth != 0 {
        return None;
    }
    Some(if empty { 0 } else { commas + 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(out: &str) -> Option<String> {
        parse_status(out).map(|g| {
            let ab = g.ahead_behind.map(|(a, b)| format!(" +{a} -{b}"));
            format!("{}{} {}", g.branch, ab.unwrap_or_default(), g.changed)
        })
    }

    const OID: &str = "# branch.oid 4f2a9c1e0b7d3a5f6e8c9d0a1b2c3d4e5f607182\n";

    #[test]
    fn git_status_ahead_behind_and_changes() {
        let out = format!(
            "{OID}# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -5\n\
             1 .M N... 100644 100644 100644 3b18e512 3b18e512 src/main.rs\n\
             2 R. N... 100644 100644 100644 9c1d0a2b 9c1d0a2b R100 src/new.rs\tsrc/old.rs\n\
             u UU N... 100644 100644 100644 100644 aaaa bbbb cccc Cargo.lock\n\
             ? notes.txt\n\
             ! target/\n"
        );
        assert_eq!(status(&out).as_deref(), Some("main +2 -5 4"));
    }

    #[test]
    fn git_status_untracked_counts_as_changed() {
        let out = format!("{OID}# branch.head dev\n? a.txt\n? b/\n");
        assert_eq!(status(&out).as_deref(), Some("dev 2"), "no upstream");
    }

    #[test]
    fn git_status_clean_and_detached() {
        let clean =
            format!("{OID}# branch.head main\n# branch.upstream origin/main\n# branch.ab +0 -0\n");
        assert_eq!(status(&clean).as_deref(), Some("main +0 -0 0"));
        let detached =
            format!("{OID}# branch.head (detached)\n1 M. N... 100644 100644 100644 a b x\n");
        assert_eq!(status(&detached).as_deref(), Some("4f2a9c1 1"));
        // A new repository, before the first commit.
        let initial = "# branch.oid (initial)\n# branch.head main\n? README.md\n";
        assert_eq!(status(initial).as_deref(), Some("main 1"));
        // With --show-stash or headers from a newer git.
        let extra = format!("{OID}# branch.head main\n# stash 3\n# future.header\n");
        assert_eq!(status(&extra).as_deref(), Some("main 0"));
        assert_eq!(status(""), None);
        assert_eq!(
            status(&format!("{OID}# branch.head main\n# branch.ab +x -0\n")),
            None
        );
    }

    #[test]
    fn each_toolchain_is_a_field() {
        for (name, _) in TOOLCHAINS {
            assert!(Toolchains::FIELDS.contains(&name), "{name}");
        }
        assert_eq!(Toolchains::FIELDS.len(), TOOLCHAINS.len() + 1, "and count");
    }

    #[test]
    fn toolchain_versions() {
        let v = |out: &str| tool_version(out);
        assert_eq!(
            v("rustc 1.98.1 (48a229cea 2026-09-01)\n").as_deref(),
            Some("1.98.1")
        );
        assert_eq!(v("v22.11.0\n").as_deref(), Some("22.11.0"));
        assert_eq!(v("Python 3.12.3\n").as_deref(), Some("3.12.3"));
        assert_eq!(
            v("rustc 1.99.0-nightly (0123abc 2026-10-01)\n").as_deref(),
            Some("1.99.0-nightly")
        );
        assert_eq!(v("error: no default toolchain\n"), None);
        assert_eq!(v(""), None);
    }

    fn reply(head: &str, body: &str) -> Vec<u8> {
        format!("{head}\r\n\r\n{body}").into_bytes()
    }

    const CONTAINERS: &str = r#"[{"Id":"8dfafdbc3a40","Names":["/web"],"Image":"nginx","Command":"nginx -g 'daemon off;'","Labels":{"note":"a ] in a string, a \" too, and {braces}"},"Ports":[{"PrivatePort":80,"Type":"tcp"}]},
{"Id":"9cd87474be90","Names":["/db"],"Labels":{},"Ports":[]}]"#;

    #[test]
    fn docker_counts_containers() {
        let ok = "HTTP/1.0 200 OK\r\nApi-Version: 1.47\r\nContent-Type: application/json\r\nServer: Docker/27.3.1 (linux)";
        assert_eq!(running_containers(&reply(ok, CONTAINERS)), Some(2));
        assert_eq!(running_containers(&reply(ok, "[]\n")), Some(0));
        let sized = format!("{ok}\r\nContent-Length: {}", CONTAINERS.len());
        assert_eq!(
            running_containers(&reply(&sized, &format!("{CONTAINERS}trailing junk"))),
            Some(2)
        );
        let http11 = "HTTP/1.1 200 OK\r\nContent-Length: 3";
        assert_eq!(
            running_containers(&reply(http11, "[{}]")),
            None,
            "too short"
        );
        assert_eq!(
            running_containers(&reply("HTTP/1.1 200 OK", "[{}]")),
            Some(1)
        );
    }

    #[test]
    fn docker_rejects_other_replies() {
        let ok = "HTTP/1.0 200 OK";
        let chunked = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked";
        assert_eq!(
            running_containers(&reply(chunked, "2\r\n[]\r\n0\r\n\r\n")),
            None
        );
        let forbidden = "HTTP/1.0 403 Forbidden\r\nContent-Type: application/json";
        assert_eq!(
            running_containers(&reply(forbidden, r#"{"message":"no"}"#)),
            None
        );
        let error = "HTTP/1.0 500 Internal Server Error";
        assert_eq!(running_containers(&reply(error, "[]")), None);
        // Cut short, or not an array at all.
        let cut = &CONTAINERS[..CONTAINERS.len() / 2];
        assert_eq!(running_containers(&reply(ok, cut)), None);
        assert_eq!(running_containers(&reply(ok, r#"{"Containers":3}"#)), None);
        assert_eq!(running_containers(&reply(ok, r#"["unterminated]"#)), None);
        assert_eq!(running_containers(&reply(ok, "[1]],[[2]")), None);
        assert_eq!(running_containers(b"HTTP/1.0 200 OK\r\n"), None, "no body");
        assert_eq!(running_containers(b""), None);
        assert_eq!(running_containers(&reply("SSH-2.0-OpenSSH", "[]")), None);
    }

    #[test]
    fn json_arrays() {
        assert_eq!(json_array_len("[]"), Some(0));
        assert_eq!(json_array_len(" [ \n ] "), Some(0));
        assert_eq!(
            json_array_len("[1, \"a,b\", [2, 3], {\"k\": [4]}]"),
            Some(4)
        );
        assert_eq!(json_array_len(r#"["\\", "\"]"]"#), Some(2));
        assert_eq!(json_array_len("[{]"), None);
        assert_eq!(json_array_len("[}]"), None);
    }
}

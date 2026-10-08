//! Running external programs without letting one hang the fetch.

use std::{
    io::Read,
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

/// How long a program may usually run before it is killed. ffetch often runs at
/// shell startup, and some programs hang instead of failing (WSL interop can).
pub const TIMEOUT: Duration = Duration::from_secs(2);
/// How long a killed program gets to be reaped.
const REAP: Duration = Duration::from_millis(100);

/// Runs `cmd` with stdin and stderr on /dev/null and captures its stdout.
/// `None` if it can't be started or doesn't finish within `timeout`, in which
/// case it is killed. Callers check the exit status if they care about it.
pub fn run(mut cmd: Command, timeout: Duration) -> Option<Output> {
    let deadline = Instant::now() + timeout;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let (child_tx, child_rx) = mpsc::channel();
    let (stdout_tx, stdout_rx) = mpsc::channel();
    // Start and read on another thread: starting can block too (exec from a
    // wedged /mnt/c), and reading as we go keeps a full pipe from stalling it.
    thread::spawn(move || {
        let Ok(mut child) = cmd.spawn() else { return };
        let pipe = child.stdout.take();
        if let Err(mpsc::SendError(mut child)) = child_tx.send(child) {
            // It took too long to start; nobody is waiting for it any more.
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        let mut stdout = Vec::new();
        if let Some(mut pipe) = pipe
            && pipe.read_to_end(&mut stdout).is_ok()
        {
            let _ = stdout_tx.send(stdout);
        }
    });

    let mut child = child_rx.recv_timeout(left(deadline)).ok()?;
    // stdout normally closes as the child exits, but it can close it earlier.
    if let Ok(stdout) = stdout_rx.recv_timeout(left(deadline))
        && let Some(status) = wait_until(&mut child, deadline)
    {
        return Some(Output {
            status,
            stdout,
            stderr: Vec::new(),
        });
    }
    let _ = child.kill();
    let _ = wait_until(&mut child, Instant::now() + REAP);
    None
}

fn left(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// Waits for `child` to exit, but not past `deadline`. It polls, which is cheap
/// here: callers only wait once stdout has closed, so the child is exiting.
fn wait_until(child: &mut Child, deadline: Instant) -> Option<ExitStatus> {
    let mut nap = Duration::from_micros(50);
    loop {
        if let Some(status) = child.try_wait().ok()? {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(nap.min(left(deadline)));
        nap = (nap * 2).min(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        path::Path,
        process,
        time::{Duration, Instant},
    };

    use super::*;

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", script]);
        cmd
    }

    #[test]
    fn captures_stdout_and_exit_status() {
        let out = run(sh("echo out; echo err >&2; exit 3"), TIMEOUT).unwrap();
        assert_eq!(out.stdout, b"out\n");
        assert!(out.stderr.is_empty());
        assert_eq!(out.status.code(), Some(3));
    }

    #[test]
    fn stdin_is_empty() {
        let out = run(sh("cat; echo done"), TIMEOUT).unwrap();
        assert_eq!(out.stdout, b"done\n");
    }

    #[test]
    fn output_larger_than_a_pipe_buffer() {
        let out = run(sh("head -c 300000 /dev/zero"), TIMEOUT).unwrap();
        assert_eq!(out.stdout.len(), 300_000);
    }

    #[test]
    fn missing_program_is_none() {
        assert!(run(Command::new("/nonexistent/ffetch-test-program"), TIMEOUT).is_none());
    }

    #[test]
    fn slow_program_is_killed_at_the_timeout() {
        let pid_file = env::temp_dir().join(format!("ffetch-test-{}-pid", process::id()));
        let script = format!("echo $$ > {}; exec sleep 5", pid_file.display());
        let start = Instant::now();
        assert!(run(sh(&script), Duration::from_millis(200)).is_none());
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(200), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");

        // The child was killed and reaped, not left running.
        let pid = fs::read_to_string(&pid_file).unwrap();
        fs::remove_file(&pid_file).unwrap();
        assert!(!Path::new(&format!("/proc/{}", pid.trim())).exists());
    }

    #[test]
    fn closing_stdout_early_does_not_escape_the_timeout() {
        let start = Instant::now();
        assert!(run(sh("exec >&-; sleep 5"), Duration::from_millis(200)).is_none());
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}

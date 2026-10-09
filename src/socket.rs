//! Short requests over Unix sockets, without letting a stuck server hang the
//! fetch.

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    sync::mpsc,
    thread,
    time::Duration,
};

/// Replies are cut off here (and then fail to parse), so a runaway server
/// can't use up memory.
const MAX_REPLY: u64 = 4 << 20;

/// Connects to the Unix socket at `path`, sends `request`, and returns what the
/// server answers until it closes the connection. `None` if it can't connect
/// (no socket, or no permission) or the whole exchange takes longer than
/// `timeout`.
pub fn request(path: &Path, request: &[u8], timeout: Duration) -> Option<Vec<u8>> {
    let (path, request) = (path.to_path_buf(), request.to_vec());
    let (tx, rx) = mpsc::channel();
    // On another thread, because connecting can block too (when the server's
    // backlog is full). The read timeout lets the thread end by itself.
    thread::spawn(move || {
        let exchange = || {
            let mut stream = UnixStream::connect(&path).ok()?;
            stream.set_read_timeout(Some(timeout)).ok()?;
            stream.set_write_timeout(Some(timeout)).ok()?;
            stream.write_all(&request).ok()?;
            let mut reply = Vec::new();
            stream.take(MAX_REPLY).read_to_end(&mut reply).ok()?;
            Some(reply)
        };
        let _ = tx.send(exchange());
    });
    rx.recv_timeout(timeout).ok()?
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        io::BufRead,
        io::BufReader,
        os::unix::net::UnixListener,
        path::PathBuf,
        process,
        time::{Duration, Instant},
    };

    use super::*;

    /// A fresh directory for the sockets of one test.
    fn dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("ffetch-test-{}-{name}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn sends_the_request_and_reads_until_closed() {
        let dir = dir("socket-reply");
        let path = dir.join("server.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut first_line = String::new();
            BufReader::new(&stream).read_line(&mut first_line).unwrap();
            (&stream).write_all(b"HTTP/1.0 200 OK\r\n\r\n[]").unwrap();
            first_line
        });
        let reply = request(&path, b"GET / HTTP/1.0\r\n\r\n", SECOND);
        assert_eq!(reply.as_deref(), Some(&b"HTTP/1.0 200 OK\r\n\r\n[]"[..]));
        assert_eq!(server.join().unwrap(), "GET / HTTP/1.0\r\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_socket_is_none() {
        let dir = dir("socket-missing");
        assert_eq!(request(&dir.join("nope.sock"), b"hi", SECOND), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn silent_server_times_out() {
        let dir = dir("socket-silent");
        let path = dir.join("server.sock");
        // Accepts (the backlog does that) but never answers or closes.
        let _listener = UnixListener::bind(&path).unwrap();
        let start = Instant::now();
        assert_eq!(request(&path, b"hi", Duration::from_millis(100)), None);
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(100), "{elapsed:?}");
        assert!(elapsed < SECOND, "{elapsed:?}");
        fs::remove_dir_all(dir).unwrap();
    }
}

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use base64::Engine;

pub(crate) struct GitHttp {
    address: std::net::SocketAddr,
    stopping: Arc<AtomicBool>,
    authenticated: Arc<AtomicUsize>,
    worker: Option<JoinHandle<()>>,
}

impl GitHttp {
    pub(crate) fn start(root: &Path, password: Option<&str>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stopping = Arc::new(AtomicBool::new(false));
        let authenticated = Arc::new(AtomicUsize::new(0));
        let stop = Arc::clone(&stopping);
        let auth = Arc::clone(&authenticated);
        let root = root.to_path_buf();
        let expected = password.map(|password| {
            format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("reader:{password}"))
            )
        });
        let worker = std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => serve(stream, &root, expected.as_deref(), &auth),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("fixture listener: {error}"),
                }
            }
        });
        Self {
            address,
            stopping,
            authenticated,
            worker: Some(worker),
        }
    }

    pub(crate) fn url(&self) -> String {
        format!("http://{}/source/.git", self.address)
    }

    pub(crate) fn authenticated_requests(&self) -> usize {
        self.authenticated.load(Ordering::Relaxed)
    }

    fn shutdown(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }
}

impl Drop for GitHttp {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn serve(mut stream: TcpStream, root: &Path, expected: Option<&str>, authenticated: &AtomicUsize) {
    // Accepted sockets can inherit the listener's nonblocking mode.
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = Vec::new();
    let header_end = loop {
        let mut buffer = [0; 4096];
        let read = stream.read(&mut buffer).unwrap();
        if read == 0 {
            return;
        }
        request.extend_from_slice(&buffer[..read]);
        if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            break end + 4;
        }
        assert!(
            request.len() < 65536,
            "fixture request headers exceeded 64 KiB"
        );
    };
    let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
    let mut lines = headers.lines();
    let first = lines.next().unwrap().split_whitespace().collect::<Vec<_>>();
    let method = first[0];
    let (path, query) = first[1].split_once('?').unwrap_or((first[1], ""));
    let fields = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim()))
        .collect::<std::collections::BTreeMap<_, _>>();
    if let Some(expected) = expected {
        if fields.get("authorization").copied() != Some(expected) {
            stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"toolconfig-fixture\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            return;
        }
        authenticated.fetch_add(1, Ordering::Relaxed);
    }
    let length = fields
        .get("content-length")
        .map_or(0, |value| value.parse::<usize>().unwrap());
    while request.len() - header_end < length {
        let mut buffer = [0; 4096];
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0, "fixture request ended early");
        request.extend_from_slice(&buffer[..read]);
    }
    let mut child = Command::new("git")
        .arg("http-backend")
        .current_dir(root)
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", path)
        .env("QUERY_STRING", query)
        .env("REQUEST_METHOD", method)
        .env(
            "CONTENT_TYPE",
            fields.get("content-type").copied().unwrap_or(""),
        )
        .env("CONTENT_LENGTH", length.to_string())
        .env("REMOTE_USER", "reader")
        .env("GIT_CONFIG_GLOBAL", root.join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&request[header_end..header_end + length])
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "git http-backend: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let split = output
        .stdout
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .unwrap();
    let headers = String::from_utf8(output.stdout[..split].to_vec()).unwrap();
    let body = &output.stdout[split + 4..];
    let status = headers
        .lines()
        .find_map(|line| line.strip_prefix("Status: "))
        .unwrap_or("200 OK");
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )
    .unwrap();
    for line in headers.lines().filter(|line| !line.starts_with("Status:")) {
        write!(stream, "{line}\r\n").unwrap();
    }
    stream.write_all(b"\r\n").unwrap();
    stream.write_all(body).unwrap();
}

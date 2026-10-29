//! A fake Smelter for tests: a server on `127.0.0.1` (an ephemeral port, never any
//! other address) that records the requests it gets, answers them as Smelter does
//! (or as a test says), and sends WebSocket frames once `POST /api/start` arrives.
//! It renders nothing. Only compiled into tests.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};

/// One request the fake received.
#[derive(Debug, Clone, PartialEq)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub body: Value,
}

type Responder = Box<dyn Fn(&Recorded) -> Option<(u16, String)> + Send + Sync>;

struct Shared {
    requests: Mutex<Vec<Recorded>>,
    started: Mutex<bool>,
    started_changed: Condvar,
    after_start: Mutex<Vec<Vec<u8>>>,
    refuse_websocket: AtomicBool,
    stop: AtomicBool,
    responder: Responder,
}

pub struct FakeSmelter {
    port: u16,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

/// A server-to-client WebSocket frame (unmasked).
pub fn frame(opcode: u8, fin: bool, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![if fin { 0x80 } else { 0 } | opcode];
    match payload.len() {
        0..=125 => out.push(payload.len() as u8),
        126..=65_535 => {
            out.push(126);
            out.extend((payload.len() as u16).to_be_bytes());
        }
        _ => {
            out.push(127);
            out.extend((payload.len() as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(payload);
    out
}

impl FakeSmelter {
    /// Start a fake. `responder` answers a request first; `None` falls back to the
    /// defaults: `GET /status` is `{"instance_id": "fake"}`, an input registers as 12 s
    /// of video and audio, anything else is `{}`.
    pub fn start(
        responder: impl Fn(&Recorded) -> Option<(u16, String)> + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind loopback");
        let port = listener.local_addr().expect("address").port();
        let shared = Arc::new(Shared {
            requests: Mutex::new(Vec::new()),
            started: Mutex::new(false),
            started_changed: Condvar::new(),
            after_start: Mutex::new(Vec::new()),
            refuse_websocket: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            responder: Box::new(responder),
        });
        let for_thread = Arc::clone(&shared);
        let thread = std::thread::spawn(move || {
            for connection in listener.incoming() {
                if for_thread.stop.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(stream) = connection else { continue };
                let shared = Arc::clone(&for_thread);
                std::thread::spawn(move || handle(stream, &shared));
            }
        });
        Self {
            port,
            shared,
            thread: Some(thread),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The requests so far, in arrival order.
    pub fn requests(&self) -> Vec<Recorded> {
        self.shared.requests.lock().expect("lock").clone()
    }

    /// The raw frames the event stream sends once `start` has been posted.
    pub fn events_after_start(&self, frames: Vec<Vec<u8>>) {
        *self.shared.after_start.lock().expect("lock") = frames;
    }

    pub fn refuse_websocket(&self) {
        self.shared.refuse_websocket.store(true, Ordering::SeqCst);
    }
}

impl Drop for FakeSmelter {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop, which is waiting for a connection.
        let _ = TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_request(
    stream: &mut TcpStream,
) -> Option<(String, String, Vec<(String, String)>, Vec<u8>)> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    let split = loop {
        if let Some(at) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&data[..split]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    let mut body = data[split + 4..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some((method, path, headers, body))
}

fn handle(mut stream: TcpStream, shared: &Shared) {
    let Some((method, path, headers, body)) = read_request(&mut stream) else {
        return;
    };
    let wants_websocket = headers
        .iter()
        .any(|(name, value)| name == "upgrade" && value.eq_ignore_ascii_case("websocket"));
    if path == "/ws" && wants_websocket {
        if shared.refuse_websocket.load(Ordering::SeqCst) {
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
            return;
        }
        let _ = stream.write_all(
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
        );
        let mut started = shared.started.lock().expect("lock");
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !*started {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() || shared.stop.load(Ordering::SeqCst) {
                return;
            }
            started = shared
                .started_changed
                .wait_timeout(started, left.min(Duration::from_millis(100)))
                .expect("wait")
                .0;
        }
        drop(started);
        for frame in shared.after_start.lock().expect("lock").iter() {
            if stream.write_all(frame).is_err() {
                return;
            }
        }
        // Hold the stream open until the fake is dropped, so a client that waits for more
        // events waits rather than sees the stream close.
        while !shared.stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(20));
        }
        return;
    }
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let recorded = Recorded {
        method,
        path: path.clone(),
        body: parsed,
    };
    shared.requests.lock().expect("lock").push(recorded.clone());
    let (status, text) = (shared.responder)(&recorded).unwrap_or_else(|| {
        if recorded.method == "GET" && path == "/status" {
            (200, json!({"instance_id": "fake"}).to_string())
        } else if path.starts_with("/api/input/") && path.ends_with("/register") {
            (
                200,
                json!({"video_duration_ms": 12000, "audio_duration_ms": 12000}).to_string(),
            )
        } else {
            (200, "{}".to_string())
        }
    });
    if path == "/api/start" {
        *shared.started.lock().expect("lock") = true;
        shared.started_changed.notify_all();
    }
    let reply = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    let _ = stream.write_all(reply.as_bytes());
}

//! A small HTTP and WebSocket client for Smelter's API, over `std::net` only. It
//! talks to `127.0.0.1` and to nothing else, sends HTTP/1.0 with `Connection:
//! close` (so a reply is not chunked and ends at EOF), and reads the server's
//! WebSocket events. No new dependency: the API is a few JSON posts and one event
//! stream (spec: `docs/specs/smelter-backend.md`, ADR-023).

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

/// The longest reply or event this client reads. Smelter's replies are a few hundred
/// bytes; the cap keeps a wrong server from filling memory.
const MAX_BYTES: usize = 1 << 20;
/// How long one request may take. Registering an input reads the file's header, so
/// it is fast; the render itself is waited for through the event stream.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// A reply: the status code and the body text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Smelter's own account of a failure: `message` and the `stack` of causes of its
    /// error body, else the body as it is.
    pub fn explain(&self) -> String {
        let Ok(body) = serde_json::from_str::<Value>(&self.body) else {
            return self.body.trim().to_string();
        };
        let mut parts = Vec::new();
        if let Some(code) = body.get("error_code").and_then(Value::as_str) {
            parts.push(code.to_string());
        }
        match body.get("stack").and_then(Value::as_array) {
            Some(stack) if !stack.is_empty() => {
                parts.extend(stack.iter().filter_map(Value::as_str).map(str::to_string));
            }
            _ => {
                if let Some(message) = body.get("message").and_then(Value::as_str) {
                    parts.push(message.to_string());
                }
            }
        }
        if parts.is_empty() {
            self.body.trim().to_string()
        } else {
            parts.join(": ")
        }
    }
}

/// A client for one Smelter server on `127.0.0.1:port`.
#[derive(Debug, Clone, Copy)]
pub struct Client {
    port: u16,
}

impl Client {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    fn address(&self) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, self.port))
    }

    fn connect(&self, timeout: Duration) -> Result<TcpStream> {
        let stream = TcpStream::connect_timeout(&self.address(), timeout)
            .with_context(|| format!("connect to Smelter on 127.0.0.1:{}", self.port))?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        Ok(stream)
    }

    pub fn get(&self, path: &str) -> Result<Response> {
        self.request("GET", path, None)
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Response> {
        self.request("POST", path, Some(body))
    }

    fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Response> {
        let mut stream = self.connect(REQUEST_TIMEOUT)?;
        let payload = match body {
            Some(body) => serde_json::to_vec(body)?,
            None => Vec::new(),
        };
        let mut head = format!(
            "{method} {path} HTTP/1.0\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\nAccept: application/json\r\n",
            self.port
        );
        if body.is_some() {
            head.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                payload.len()
            ));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())?;
        stream.write_all(&payload)?;
        let mut reply = Vec::new();
        (&mut stream)
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut reply)
            .with_context(|| format!("read the reply to {method} {path}"))?;
        if reply.len() > MAX_BYTES {
            bail!("the reply to {method} {path} is longer than {MAX_BYTES} bytes");
        }
        parse_response(&reply).with_context(|| format!("the reply to {method} {path}"))
    }

    /// Open the event stream (`GET /ws`). Open it before `start`, so no event is missed.
    pub fn events(&self) -> Result<Events> {
        let mut stream = self.connect(REQUEST_TIMEOUT)?;
        // Any 16 bytes, base64: the key is only there to be answered. The server's
        // reply is not checked against it: this is a loopback connection to our own child.
        let request = format!(
            "GET /ws HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
            self.port
        );
        stream.write_all(request.as_bytes())?;
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() > 8192 {
                bail!("the WebSocket handshake reply is longer than 8192 bytes");
            }
            let n = stream
                .read(&mut byte)
                .context("read the WebSocket handshake")?;
            if n == 0 {
                bail!("Smelter closed the connection during the WebSocket handshake");
            }
            head.push(byte[0]);
        }
        let status = String::from_utf8_lossy(&head);
        let line = status.lines().next().unwrap_or_default();
        if !line.contains(" 101 ") {
            bail!("the WebSocket handshake was refused: {line}");
        }
        Ok(Events { stream })
    }
}

fn parse_response(reply: &[u8]) -> Result<Response> {
    let split = reply
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("no end of headers"))?;
    let head = String::from_utf8_lossy(&reply[..split]);
    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| anyhow!("not an HTTP status line: \"{status_line}\""))?;
    let chunked = lines.any(|line| {
        let line = line.to_ascii_lowercase();
        line.starts_with("transfer-encoding:") && line.contains("chunked")
    });
    let body = &reply[split + 4..];
    let body = if chunked {
        dechunk(body)?
    } else {
        body.to_vec()
    };
    Ok(Response {
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// The payload of a chunked body.
fn dechunk(mut rest: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = rest
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| anyhow!("a chunk size is not followed by a line end"))?;
        let size_text = String::from_utf8_lossy(&rest[..line_end]);
        let size_text = size_text.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| anyhow!("\"{size_text}\" is not a chunk size"))?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if rest.len() < size {
            bail!("a chunk is cut short");
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size..];
        rest = rest.strip_prefix(b"\r\n").unwrap_or(rest);
    }
}

/// What the event stream gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A JSON text message.
    Text(String),
    /// The server closed the stream.
    Closed,
    /// Nothing arrived before the deadline.
    Timeout,
}

/// The server's WebSocket events.
pub struct Events {
    stream: TcpStream,
}

impl Events {
    /// The next text message, waiting until `deadline`.
    pub fn next(&mut self, deadline: Instant) -> Result<Event> {
        let mut message: Option<Vec<u8>> = None;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(Event::Timeout);
            }
            self.stream.set_read_timeout(Some(deadline - now))?;
            let frame = match read_frame(&mut self.stream) {
                Ok(frame) => frame,
                Err(error) if is_timeout(&error) => return Ok(Event::Timeout),
                Err(error) if is_eof(&error) => return Ok(Event::Closed),
                Err(error) => return Err(error),
            };
            match frame.opcode {
                // Text, or the continuation of one.
                0x1 | 0x0 => {
                    let buffer = message.get_or_insert_with(Vec::new);
                    if buffer.len() + frame.payload.len() > MAX_BYTES {
                        bail!("a WebSocket message is longer than {MAX_BYTES} bytes");
                    }
                    buffer.extend_from_slice(&frame.payload);
                    if frame.fin {
                        let text = message.take().unwrap_or_default();
                        return Ok(Event::Text(String::from_utf8_lossy(&text).into_owned()));
                    }
                }
                0x8 => return Ok(Event::Closed),
                0x9 => self.pong(&frame.payload)?,
                _ => {}
            }
        }
    }

    /// Answer a ping with the same payload. A client frame is masked; the mask is zero.
    fn pong(&mut self, payload: &[u8]) -> Result<()> {
        let mut frame = vec![0x8A, 0x80 | payload.len().min(125) as u8, 0, 0, 0, 0];
        frame.extend_from_slice(&payload[..payload.len().min(125)]);
        self.stream.write_all(&frame)?;
        Ok(())
    }
}

fn is_timeout(error: &anyhow::Error) -> bool {
    error.downcast_ref::<std::io::Error>().is_some_and(|io| {
        matches!(
            io.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        )
    })
}

/// The connection ended: the server closed it, or went away, between frames.
fn is_eof(error: &anyhow::Error) -> bool {
    error.downcast_ref::<std::io::Error>().is_some_and(|io| {
        matches!(
            io.kind(),
            std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::BrokenPipe
        )
    })
}

#[derive(Debug, PartialEq, Eq)]
struct Frame {
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
}

/// One WebSocket frame (RFC 6455, section 5.2). A masked frame is unmasked.
fn read_frame(reader: &mut impl Read) -> Result<Frame> {
    let mut head = [0u8; 2];
    reader.read_exact(&mut head).map_err(anyhow::Error::new)?;
    let fin = head[0] & 0x80 != 0;
    let opcode = head[0] & 0x0F;
    let masked = head[1] & 0x80 != 0;
    let length = match head[1] & 0x7F {
        126 => {
            let mut bytes = [0u8; 2];
            reader.read_exact(&mut bytes)?;
            u64::from(u16::from_be_bytes(bytes))
        }
        127 => {
            let mut bytes = [0u8; 8];
            reader.read_exact(&mut bytes)?;
            u64::from_be_bytes(bytes)
        }
        short => u64::from(short),
    };
    if length > MAX_BYTES as u64 {
        bail!("a WebSocket frame of {length} bytes is longer than {MAX_BYTES}");
    }
    let mut mask = [0u8; 4];
    if masked {
        reader.read_exact(&mut mask)?;
    }
    let mut payload = vec![0u8; length as usize];
    reader.read_exact(&mut payload)?;
    if masked {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    Ok(Frame {
        fin,
        opcode,
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smelter_fake::{frame, FakeSmelter};
    use serde_json::json;
    use std::io::Cursor;

    #[test]
    fn a_frame_of_each_length_class_is_read() {
        for length in [0usize, 5, 125, 126, 300, 65_535, 65_536] {
            let payload = vec![b'x'; length];
            let bytes = frame(0x1, true, &payload);
            let got = read_frame(&mut Cursor::new(bytes)).expect("frame");
            assert_eq!(got.payload, payload, "length {length}");
            assert!(got.fin);
            assert_eq!(got.opcode, 1);
        }
    }

    #[test]
    fn a_masked_frame_is_unmasked() {
        let mut bytes = vec![0x81, 0x80 | 3, 1, 2, 3, 4];
        bytes.extend([b'a' ^ 1, b'b' ^ 2, b'c' ^ 3]);
        let got = read_frame(&mut Cursor::new(bytes)).expect("frame");
        assert_eq!(got.payload, b"abc");
    }

    #[test]
    fn a_frame_longer_than_the_cap_is_refused_before_it_is_read() {
        let mut bytes = vec![0x81, 127];
        bytes.extend((MAX_BYTES as u64 + 1).to_be_bytes());
        let error = read_frame(&mut Cursor::new(bytes)).expect_err("too long");
        assert!(error.to_string().contains("longer than"), "{error}");
    }

    #[test]
    fn a_cut_short_frame_is_an_error() {
        let mut bytes = frame(0x1, true, b"hello");
        bytes.truncate(4);
        assert!(read_frame(&mut Cursor::new(bytes)).is_err());
    }

    #[test]
    fn a_response_is_a_status_and_a_body() {
        let got = parse_response(b"HTTP/1.1 400 Bad Request\r\ncontent-type: x\r\n\r\n{\"a\":1}")
            .expect("response");
        assert_eq!(
            got,
            Response {
                status: 400,
                body: "{\"a\":1}".to_string()
            }
        );
        assert!(!got.ok());
        assert!(parse_response(b"HTTP/1.1 204 No Content\r\n\r\n")
            .expect("response")
            .ok());
        assert!(parse_response(b"garbage").is_err());
        assert!(parse_response(b"HTTP/1.1 abc\r\n\r\n").is_err());
    }

    #[test]
    fn a_chunked_body_is_joined() {
        let reply = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2;ext=1\r\nde\r\n0\r\n\r\n";
        assert_eq!(parse_response(reply).expect("response").body, "abcde");
        let cut = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n9\r\nabc";
        assert!(parse_response(cut).is_err());
    }

    #[test]
    fn an_error_body_is_explained_in_smelters_words() {
        let body = json!({
            "error_code": "INPUT_STREAM_INPUT_ERROR",
            "message": "Input initialization error while registering input for stream \"x\".",
            "stack": ["Input initialization error", "No suitable track in the mp4 file"]
        });
        let response = Response {
            status: 500,
            body: body.to_string(),
        };
        assert_eq!(
            response.explain(),
            "INPUT_STREAM_INPUT_ERROR: Input initialization error: No suitable track in the mp4 file"
        );
        let only_message = Response {
            status: 400,
            body: json!({"message": "bad"}).to_string(),
        };
        assert_eq!(only_message.explain(), "bad");
        let text = Response {
            status: 502,
            body: " not json \n".to_string(),
        };
        assert_eq!(text.explain(), "not json");
    }

    #[test]
    fn it_posts_json_and_reads_the_reply() {
        let fake = FakeSmelter::start(|request| {
            (request.path == "/api/x").then(|| (201, "{\"made\":true}".to_string()))
        });
        let client = Client::new(fake.port());
        let reply = client.post("/api/x", &json!({"a": [1, 2]})).expect("post");
        assert_eq!(
            (reply.status, reply.body.as_str()),
            (201, "{\"made\":true}")
        );
        let seen = fake.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            (seen[0].method.as_str(), seen[0].path.as_str()),
            ("POST", "/api/x")
        );
        assert_eq!(seen[0].body, json!({"a": [1, 2]}));
        let status = client.get("/status").expect("status");
        assert!(status.ok() && status.body.contains("instance_id"));
    }

    #[test]
    fn events_are_read_until_the_stream_closes() {
        let fake = FakeSmelter::start(|_| None);
        fake.events_after_start(vec![
            frame(0x1, true, b"{\"type\":\"A\"}"),
            // A ping, then a message in two fragments.
            frame(0x9, true, b"hi"),
            frame(0x1, false, b"{\"type\":"),
            frame(0x0, true, b"\"B\"}"),
            frame(0x8, true, b""),
        ]);
        let client = Client::new(fake.port());
        let mut events = client.events().expect("events");
        client.post("/api/start", &json!({})).expect("start");
        let deadline = Instant::now() + Duration::from_secs(10);
        assert_eq!(
            events.next(deadline).expect("a"),
            Event::Text("{\"type\":\"A\"}".to_string())
        );
        assert_eq!(
            events.next(deadline).expect("b"),
            Event::Text("{\"type\":\"B\"}".to_string())
        );
        assert_eq!(events.next(deadline).expect("closed"), Event::Closed);
    }

    #[test]
    fn waiting_for_an_event_that_never_comes_times_out() {
        let fake = FakeSmelter::start(|_| None);
        let client = Client::new(fake.port());
        let mut events = client.events().expect("events");
        let started = Instant::now();
        let got = events
            .next(Instant::now() + Duration::from_millis(150))
            .expect("timeout");
        assert_eq!(got, Event::Timeout);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_refused_handshake_is_an_error() {
        let fake = FakeSmelter::start(|_| None);
        fake.refuse_websocket();
        let error = Client::new(fake.port()).events().err().expect("refused");
        assert!(error.to_string().contains("refused"), "{error}");
    }

    #[test]
    fn a_closed_port_is_an_error_that_names_it() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("addr").port()
        };
        let error = Client::new(port).get("/status").expect_err("refused");
        assert!(
            format!("{error:#}").contains(&port.to_string()),
            "{error:#}"
        );
    }
}

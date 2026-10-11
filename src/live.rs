//! Replayable Server-Sent Events over loopback HTTP. Readers tail the committed
//! journal on independent threads; subscriber backpressure never reaches the
//! producer. Memory and client counts are bounded, with no unbounded event queue.
use crate::groups::PipelineGroup;
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
const MAX_CLIENTS: usize = 8;
const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;
const POLL: Duration = Duration::from_millis(10);
const WRITE_TIMEOUT: Duration = Duration::from_millis(500);
const DRAIN: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(crate) struct JournalState {
    pub committed_bytes: AtomicU64,
    pub closed: AtomicBool,
}
#[derive(Clone)]
struct Source {
    path: PathBuf,
    journal: Option<Arc<JournalState>>,
}
impl Source {
    fn visible_bytes(&self, file: &File) -> io::Result<u64> {
        match &self.journal {
            Some(j) => Ok(j.committed_bytes.load(Ordering::Acquire)),
            None => Ok(file.metadata()?.len()),
        }
    }
    fn closed(&self) -> bool {
        self.journal
            .as_ref()
            .is_some_and(|j| j.closed.load(Ordering::Acquire))
    }
}
/// The endpoint is live until this guard is dropped. Existing subscribers get a
/// bounded drain window; late replay can use `isr-events` on the retained journal.
pub struct LiveFeed {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl LiveFeed {
    pub(crate) fn for_journal(
        path: &Path,
        journal: Arc<JournalState>,
        bind: SocketAddr,
    ) -> io::Result<Self> {
        Self::start(
            Source {
                path: path.to_owned(),
                journal: Some(journal),
            },
            bind,
        )
    }
    /// Serve an existing journal, including a file still being appended by
    /// another process. Only complete JSONL records are sent.
    pub fn from_file(path: &Path, bind: SocketAddr) -> io::Result<Self> {
        if !fs::metadata(path)?.is_file() {
            return Err(io::Error::other("event source must be a regular file"));
        }
        Self::start(
            Source {
                path: path.canonicalize()?,
                journal: None,
            },
            bind,
        )
    }
    fn start(source: Source, bind: SocketAddr) -> io::Result<Self> {
        if !bind.ip().is_loopback() {
            return Err(io::Error::other(
                "live events must bind to a loopback address",
            ));
        }
        let listener = TcpListener::bind(bind)?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let handle = thread::Builder::new()
            .name("isr-events".into())
            .spawn(move || {
                let mut clients: Vec<JoinHandle<()>> = Vec::new();
                while !stopped.load(Ordering::Acquire) {
                    let mut i = 0;
                    while i < clients.len() {
                        if clients[i].is_finished() {
                            let _ = clients.swap_remove(i).join();
                        } else {
                            i += 1;
                        }
                    }
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
                            let _ = stream.set_nodelay(true);
                            if clients.len() >= MAX_CLIENTS {
                                let _ = reply(
                                    &mut stream,
                                    "503 Service Unavailable",
                                    "text/plain",
                                    b"subscriber limit reached\n",
                                );
                                continue;
                            }
                            let source = source.clone();
                            let stop = stopped.clone();
                            if let Ok(client) = thread::Builder::new()
                                .name("isr-event-client".into())
                                .spawn(move || {
                                    let _ = serve(stream, source, stop);
                                })
                            {
                                clients.push(client);
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
                        Err(_) => break,
                    }
                }
                for client in clients {
                    let _ = client.join();
                }
            })?;
        Ok(Self {
            address,
            stop,
            thread: Some(handle),
        })
    }
    pub fn endpoint(&self) -> String {
        format!("http://{}/events", self.address)
    }
}
impl Drop for LiveFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn reply(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) -> io::Result<()> {
    let header = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",body.len());
    write_frame(stream, &[header.as_bytes(), body], None)
}
// An absolute deadline for the whole frame, not a timeout renewed after each
// partial write. Even a client reading a few bytes at a time cannot delay exit.
fn write_frame(stream: &mut TcpStream, parts: &[&[u8]], drain: Option<Instant>) -> io::Result<()> {
    let deadline = (Instant::now() + WRITE_TIMEOUT)
        .min(drain.unwrap_or_else(|| Instant::now() + WRITE_TIMEOUT));
    for mut part in parts.iter().copied() {
        while !part.is_empty() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            stream.set_write_timeout(Some(remaining))?;
            match stream.write(part) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => part = &part[count..],
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}
fn request(stream: &mut TcpStream) -> io::Result<(String, u64)> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut bytes = Vec::new();
    while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
        if bytes.len() >= 8192 || Instant::now() >= deadline {
            return Err(io::Error::other("request headers too large or slow"));
        }
        stream.set_read_timeout(Some(
            deadline
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1)),
        ))?;
        let mut chunk = [0u8; 1024];
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() > 8192 {
            return Err(io::Error::other("request headers too large"));
        }
    }
    let request = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
    let mut lines = request.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    if first.next() != Some("GET") {
        return Err(io::Error::other("expected GET"));
    }
    let target = first
        .next()
        .ok_or_else(|| io::Error::other("missing request target"))?;
    if first.next() != Some("HTTP/1.1") || first.next().is_some() {
        return Err(io::Error::other("expected HTTP/1.1"));
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut after = 0;
    if !query.is_empty() {
        after = query
            .strip_prefix("after=")
            .ok_or_else(|| io::Error::other("expected after=N"))?
            .parse()
            .map_err(io::Error::other)?;
    }
    for line in lines.take_while(|s| !s.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| io::Error::other("invalid header"))?;
        if name.eq_ignore_ascii_case("last-event-id") {
            after = value.trim().parse().map_err(io::Error::other)?;
        }
    }
    Ok((path.to_owned(), after))
}
fn serve(mut stream: TcpStream, source: Source, stop: Arc<AtomicBool>) -> io::Result<()> {
    let (path, after) = match request(&mut stream) {
        Ok(r) => r,
        Err(e) => {
            reply(
                &mut stream,
                "400 Bad Request",
                "text/plain",
                format!("{e}\n").as_bytes(),
            )?;
            return Ok(());
        }
    };
    if path == "/groups" {
        return reply(
            &mut stream,
            "200 OK",
            "application/json",
            &serde_json::to_vec(&PipelineGroup::catalog())?,
        );
    }
    if path != "/events" {
        return reply(
            &mut stream,
            "404 Not Found",
            "text/plain",
            b"use /events?after=N or /groups\n",
        );
    }
    let mut file = File::open(&source.path)?;
    write_frame(&mut stream, &[b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nX-Accel-Buffering: no\r\nConnection: close\r\n\r\nretry: 250\n\n"], None)?;
    let mut offset = 0u64;
    let mut pending = Vec::new();
    let mut chunk = [0u8; 16384];
    let mut heartbeat = Instant::now();
    let mut drain = None;
    loop {
        if stop.load(Ordering::Acquire) || source.closed() {
            drain.get_or_insert_with(|| Instant::now() + DRAIN);
        }
        if drain.is_some_and(|deadline| Instant::now() >= deadline) {
            return Ok(());
        }
        let visible = source.visible_bytes(&file)?;
        if visible < offset {
            return Err(io::Error::other("event journal was truncated"));
        }
        let available = (visible - offset).min(chunk.len() as u64) as usize;
        if available == 0 {
            if drain.is_some() {
                return Ok(());
            }
            if heartbeat.elapsed() >= Duration::from_millis(250) {
                write_frame(&mut stream, &[b": keepalive\n\n"], drain)?;
                heartbeat = Instant::now();
            }
            thread::sleep(POLL);
            continue;
        }
        let count = file.read(&mut chunk[..available])?;
        if count == 0 {
            return Err(io::Error::other("committed event bytes missing"));
        }
        offset += count as u64;
        for part in chunk[..count].split_inclusive(|b| *b == b'\n') {
            if stop.load(Ordering::Acquire) || source.closed() {
                drain.get_or_insert_with(|| Instant::now() + DRAIN);
            }
            if drain.is_some_and(|deadline| Instant::now() >= deadline) {
                return Ok(());
            }
            if pending.len() + part.len() > MAX_EVENT_BYTES {
                return Err(io::Error::other("event exceeds stream record limit"));
            }
            pending.extend_from_slice(part);
            if pending.last() != Some(&b'\n') {
                continue;
            }
            let event: Value = serde_json::from_slice(&pending)?;
            let seq = event["seq"]
                .as_u64()
                .ok_or_else(|| io::Error::other("event has no sequence"))?;
            if seq > after {
                let header = format!("id: {seq}\nevent: ledger\ndata: ");
                write_frame(&mut stream, &[header.as_bytes(), &pending, b"\n"], drain)?;
            }
            if event["type"] == "result" {
                return Ok(());
            }
            pending.clear();
        }
    }
}

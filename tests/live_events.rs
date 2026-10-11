use cranelift_isr::{groups::PipelineGroup, ledger::RunLedger, live::LiveFeed};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "isr-live-{}-{}-{}",
            std::process::id(),
            cranelift_isr::ledger::timestamp_ns(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn ledger(&self) -> RunLedger {
        RunLedger::create(&self.0).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn bind() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}
fn connect(feed: &LiveFeed, target: &str, headers: &str) -> BufReader<TcpStream> {
    let address = feed.endpoint();
    let address = address
        .strip_prefix("http://")
        .unwrap()
        .strip_suffix("/events")
        .unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: {address}\r\n{headers}\r\n"
    )
    .unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(line.starts_with("HTTP/1.1 200"), "{line}");
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
    }
    reader
}
fn event(reader: &mut BufReader<TcpStream>) -> Value {
    let mut id = None;
    let mut kind = String::new();
    let mut data = None;
    loop {
        let mut line = String::new();
        assert!(
            reader.read_line(&mut line).unwrap() > 0,
            "stream ended before event"
        );
        if let Some(value) = line.strip_prefix("id: ") {
            id = Some(value.trim().parse::<u64>().unwrap());
        } else if let Some(value) = line.strip_prefix("event: ") {
            kind = value.trim().to_owned();
        } else if let Some(value) = line.strip_prefix("data: ") {
            data = Some(serde_json::from_str::<Value>(value).unwrap());
        } else if line == "\n" {
            if let Some(value) = data.take() {
                assert_eq!(kind, "ledger");
                assert_eq!(id, value["seq"].as_u64());
                return value;
            }
        }
    }
}
#[test]
fn live_delivery_precedes_completion_and_resume_matches_durable_journal() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    let feed = ledger.serve_events(bind()).unwrap();
    let mut first = connect(&feed, "/events", "");
    ledger.event("started", json!({})).unwrap();
    let start = event(&mut first);
    assert_eq!(start["seq"], 1);
    assert_eq!(start["groups"], PipelineGroup::catalog());
    ledger
        .event(
            "stage",
            json!({"stage":"lower","status":"started","block_id":1}),
        )
        .unwrap();
    assert_eq!(event(&mut first)["group"], "lower");
    drop(first);
    ledger
        .event(
            "stage",
            json!({"stage":"lower","status":"completed","block_id":1}),
        )
        .unwrap();
    // Last-Event-ID takes precedence over the initial URL's older cursor.
    let mut resumed = connect(&feed, "/events?after=1", "Last-Event-ID: 2\r\n");
    assert_eq!(event(&mut resumed)["seq"], 3);
    // No terminal record existed when those events were received.
    ledger
        .event("result", json!({"status":"completed"}))
        .unwrap();
    assert_eq!(event(&mut resumed)["seq"], 4);
    assert_eq!(resumed.read(&mut [0; 1]).unwrap(), 0);
    let journal = ledger.directory().join("events.jsonl");
    drop(feed);
    drop(ledger);
    // A new server can replay a completed journal after the producer has exited.
    let replay = LiveFeed::from_file(&journal, bind()).unwrap();
    let mut reader = connect(&replay, "/events", "");
    for line in fs::read_to_string(journal).unwrap().lines() {
        assert_eq!(
            event(&mut reader),
            serde_json::from_str::<Value>(line).unwrap()
        );
    }
    let mut cursor = connect(&replay, "/events?after=3", "");
    assert_eq!(event(&mut cursor)["type"], "result");
    let mut catalog = connect(&replay, "/groups", "");
    let mut body = String::new();
    catalog.read_to_string(&mut body).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap(),
        PipelineGroup::catalog()
    );
}

#[test]
fn file_follower_waits_for_complete_records() {
    let fixture = Fixture::new();
    let path = fixture.0.join("events.jsonl");
    fs::write(&path, b"{\"seq\":1,\"type\":\"sta").unwrap();
    let feed = LiveFeed::from_file(&path, bind()).unwrap();
    let mut reader = connect(&feed, "/events", "");
    let (tx, rx) = mpsc::channel();
    let client = thread::spawn(move || tx.send(event(&mut reader)).unwrap());
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    let mut writer = OpenOptions::new().append(true).open(path).unwrap();
    writer.write_all(b"rted\"}\n").unwrap();
    writer.sync_all().unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap()["seq"], 1);
    client.join().unwrap();
}

#[test]
fn stalled_and_disconnected_consumers_do_not_block_producer() {
    let fixture = Fixture::new();
    let mut ledger = fixture.ledger();
    let feed = ledger.serve_events(bind()).unwrap();
    let stalled = connect(&feed, "/events", "");
    drop(connect(&feed, "/events", ""));
    let (tx, rx) = mpsc::channel();
    let producer = thread::spawn(move || {
        // Exceeds normal TCP buffers: the first consumer never reads its events.
        let payload = "x".repeat(64 * 1024);
        for _ in 0..256 {
            ledger
                .event("progress", json!({"payload":payload}))
                .unwrap();
        }
        ledger
            .event("result", json!({"status":"completed"}))
            .unwrap();
        tx.send(ledger).unwrap();
    });
    let ledger = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("producer waited for a stalled subscriber");
    producer.join().unwrap();
    // Another consumer can recover from disk independently of the stalled one.
    let mut tail = connect(&feed, "/events?after=256", "");
    assert_eq!(event(&mut tail)["seq"], 257);
    let shutdown = Instant::now();
    drop(feed);
    assert!(
        shutdown.elapsed() < Duration::from_secs(5),
        "stalled subscriber delayed shutdown"
    );
    drop(stalled);
    drop(ledger);
}

#[test]
fn live_service_rejects_public_binding_and_bad_cursors() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    assert!(ledger.serve_events("0.0.0.0:0".parse().unwrap()).is_err());
    let feed = ledger.serve_events(bind()).unwrap();
    let endpoint = feed.endpoint();
    let address = endpoint
        .strip_prefix("http://")
        .unwrap()
        .strip_suffix("/events")
        .unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(
        stream,
        "GET /events?after=invalid HTTP/1.1\r\nHost: localhost\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 400"));
}

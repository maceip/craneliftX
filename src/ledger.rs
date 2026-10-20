//! A durable, local JSONL ledger. Each event is flushed and synced before returning.
use crate::{context::GuestContext, groups::PipelineGroup, pipeline::StageEvent};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub fn timestamp_ns() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}
pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn registers(ctx: &GuestContext) -> Value {
    let mut value = serde_json::to_value(ctx).expect("integer context serializes");
    for register in value
        .as_object_mut()
        .expect("context is an object")
        .values_mut()
    {
        *register = json!(format!(
            "0x{:016x}",
            register.as_u64().expect("u64 register")
        ));
    }
    value
}

pub struct RunLedger {
    directory: PathBuf,
    file: File,
    run_id: String,
    seq: u64,
    start: Instant,
    observer_error: Option<io::Error>,
    journal: Arc<crate::live::JournalState>,
}
impl RunLedger {
    pub fn create(root: &Path) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        fs::create_dir_all(root)?;
        let run_id = format!(
            "{}-{}-{}",
            timestamp_ns(),
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let directory = root.join(&run_id);
        fs::create_dir(&directory)?;
        let directory = directory.canonicalize()?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join("events.jsonl"))?;
        Ok(Self {
            directory,
            file,
            run_id,
            seq: 0,
            start: Instant::now(),
            observer_error: None,
            journal: Arc::default(),
        })
    }
    pub fn serve_events(&self, bind: std::net::SocketAddr) -> io::Result<crate::live::LiveFeed> {
        crate::live::LiveFeed::for_journal(
            &self.directory.join("events.jsonl"),
            self.journal.clone(),
            bind,
        )
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn event(&mut self, kind: &str, fields: Value) -> io::Result<()> {
        self.event_in(PipelineGroup::for_event(kind, &fields), kind, fields)
    }
    pub fn event_in(
        &mut self,
        group: PipelineGroup,
        kind: &str,
        mut fields: Value,
    ) -> io::Result<()> {
        self.check()?;
        if !fields.is_object() {
            return Err(io::Error::other("event fields must be an object"));
        }
        group.annotate(kind, &mut fields);
        let map = fields
            .as_object_mut()
            .ok_or_else(|| io::Error::other("event fields must be an object"))?;
        self.seq += 1;
        map.insert("v".into(), json!(1));
        map.insert("type".into(), json!(kind));
        map.insert("run_id".into(), json!(self.run_id));
        map.insert("seq".into(), json!(self.seq));
        // Preserve the actual stage timestamp, rather than time of persistence.
        map.entry("timestamp_unix_ns")
            .or_insert_with(|| json!(timestamp_ns()));
        map.insert(
            "run_elapsed_ns".into(),
            json!(self.start.elapsed().as_nanos().min(u64::MAX as u128) as u64),
        );
        let mut line = serde_json::to_vec(&fields)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.flush()?;
        if kind == "result" {
            self.file.sync_data()?;
        }
        // Live subscribers cannot observe a record until persistence succeeds.
        self.journal
            .committed_bytes
            .fetch_add(line.len() as u64, Ordering::Release);
        Ok(())
    }
    /// Adapter for the engine's infallible observer. Call check before execution
    /// and before publishing success; an I/O failure must never become success.
    pub fn stage(&mut self, event: StageEvent) {
        if self.observer_error.is_none() {
            if let Err(error) = self.event(
                "stage",
                serde_json::to_value(event).expect("stage serializes"),
            ) {
                self.observer_error = Some(error);
            }
        }
    }
    pub fn check(&mut self) -> io::Result<()> {
        match self.observer_error.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    pub fn artifact(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        if Path::new(name).file_name().and_then(|n| n.to_str()) != Some(name) {
            return Err(io::Error::other("artifact name must be a single filename"));
        }
        let path = self.directory.join(name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        self.event(
            "artifact",
            json!({"name":name, "path":path, "bytes":bytes.len(), "sha256":sha256(bytes)}),
        )
    }
}

impl Drop for RunLedger {
    fn drop(&mut self) {
        self.journal.closed.store(true, Ordering::Release);
    }
}

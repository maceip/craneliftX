//! Curated, bounded demo worker. stdout is a flushed JSONL protocol; stderr is diagnostics.
use cranelift_isr::{
    ledger::{sha256, timestamp_ns, RunLedger},
    pipeline::{PreparedTrace, StageEvent},
    riscv_exec::RiscvExecutor,
    EngineError, GuestContext,
};
use serde_json::{json, Value};
static RUN_LEDGER: OnceLock<Mutex<Option<RunLedger>>> = OnceLock::new();
use std::{
    io::{self, BufRead, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
fn emit(kind: &str, fields: &str) {
    emit_value(
        kind,
        serde_json::from_str(&format!("{{\"v\":1{fields}}}")).expect("event fields"),
    );
}
fn emit_value(kind: &str, mut event: Value) {
    static CLOCK: OnceLock<Mutex<(Instant, u64, String)>> = OnceLock::new();
    let mut clock = CLOCK
        .get_or_init(|| {
            Mutex::new((
                Instant::now(),
                0,
                format!("{}-{}", timestamp_ns(), std::process::id()),
            ))
        })
        .lock()
        .expect("event clock");
    clock.1 += 1;
    event["v"] = json!(1);
    event["type"] = json!(kind);
    cranelift_isr::groups::PipelineGroup::for_event(kind, &event).annotate(kind, &mut event);
    event["worker_run_id"] = json!(clock.2);
    event["worker_seq"] = json!(clock.1);
    event["worker_elapsed_ns"] = json!(clock.0.elapsed().as_nanos() as u64);
    if event.get("timestamp_unix_ns").is_none() {
        event["timestamp_unix_ns"] = json!(timestamp_ns());
    }
    if let Some(ledger) = RUN_LEDGER
        .get()
        .expect("ledger initialized")
        .lock()
        .expect("ledger lock")
        .as_mut()
    {
        ledger
            .event(kind, event.clone())
            .expect("persist execution ledger");
    }
    let mut output = io::stdout().lock();
    writeln!(output, "{event}").expect("event stdout");
    output.flush().expect("flush events");
}
fn emit_stage(kernel: Option<usize>, event: StageEvent) {
    let mut fields = serde_json::to_value(event).expect("stage event");
    fields["kernel"] = json!(kernel);
    emit_value("stage", fields);
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn word(bytes: &[u8], index: usize) -> u64 {
    u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().unwrap())
}
fn formula(bytes: &[u8], seed: u64, variant: usize) -> u64 {
    let mut a = word(bytes, 0);
    for index in 1..7 {
        a = a.wrapping_add(word(bytes, index));
    }
    ((a ^ seed) << (variant + 1)).wrapping_add([17, 37, 73, 109][variant]) | 5
}
// The .byte fixtures and portable byte slices use the exact same macro content.
macro_rules! kernel_bytes {
    ($shift:expr, $add:expr) => {
        [
            0x48, 0x8b, 0x07, 0x48, 0x8b, 0x4f, 8, 0x48, 0x01, 0xc8, 0x48, 0x8b, 0x4f, 16, 0x48,
            0x01, 0xc8, 0x48, 0x8b, 0x4f, 24, 0x48, 0x01, 0xc8, 0x48, 0x8b, 0x4f, 32, 0x48, 0x01,
            0xc8, 0x48, 0x8b, 0x4f, 40, 0x48, 0x01, 0xc8, 0x48, 0x8b, 0x4f, 48, 0x48, 0x01, 0xc8,
            0x48, 0x31, 0xf0, 0x48, 0xc1, 0xe0, $shift, 0x48, 0x83, 0xc0, $add, 0x48, 0x83, 0xc8,
            5, 0x48, 0x89, 0x47, 56, 0xc3,
        ]
    };
}
static PORTABLE: [[u8; 65]; 4] = [
    kernel_bytes!(1, 17),
    kernel_bytes!(2, 37),
    kernel_bytes!(3, 73),
    kernel_bytes!(4, 109),
];
#[cfg(target_arch = "x86_64")]
macro_rules! fixture {
    ($name:literal,$shift:literal,$add:literal) => {
        core::arch::global_asm!(concat!(
            ".text\n.p2align 4\n.globl ",
            $name,
            "\n",
            $name,
            ":\n",
            "mov rax,[rdi]\nmov rcx,[rdi+8]\nadd rax,rcx\nmov rcx,[rdi+16]\nadd rax,rcx\n",
            "mov rcx,[rdi+24]\nadd rax,rcx\nmov rcx,[rdi+32]\nadd rax,rcx\n",
            "mov rcx,[rdi+40]\nadd rax,rcx\nmov rcx,[rdi+48]\nadd rax,rcx\n",
            "xor rax,rsi\n.byte 0x48,0xc1,0xe0,",
            $shift,
            "\nadd rax,",
            $add,
            "\nor rax,5\nmov [rdi+56],rax\nret\n"
        ));
    };
}
#[cfg(all(target_arch = "x86_64", target_vendor = "apple"))]
mod native {
    fixture!("_isr_packet0", "1", "17");
    fixture!("_isr_packet1", "2", "37");
    fixture!("_isr_packet2", "3", "73");
    fixture!("_isr_packet3", "4", "109");
    unsafe extern "C" {
        pub fn isr_packet0(p: *mut u8, s: u64) -> u64;
        pub fn isr_packet1(p: *mut u8, s: u64) -> u64;
        pub fn isr_packet2(p: *mut u8, s: u64) -> u64;
        pub fn isr_packet3(p: *mut u8, s: u64) -> u64;
    }
}
#[cfg(all(target_arch = "x86_64", not(target_vendor = "apple")))]
mod native {
    fixture!("isr_packet0", "1", "17");
    fixture!("isr_packet1", "2", "37");
    fixture!("isr_packet2", "3", "73");
    fixture!("isr_packet3", "4", "109");
    unsafe extern "C" {
        pub fn isr_packet0(p: *mut u8, s: u64) -> u64;
        pub fn isr_packet1(p: *mut u8, s: u64) -> u64;
        pub fn isr_packet2(p: *mut u8, s: u64) -> u64;
        pub fn isr_packet3(p: *mut u8, s: u64) -> u64;
    }
}
#[cfg(target_arch = "x86_64")]
const NATIVE: [unsafe extern "C" fn(*mut u8, u64) -> u64; 4] = [
    native::isr_packet0,
    native::isr_packet1,
    native::isr_packet2,
    native::isr_packet3,
];
fn rip(variant: usize) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        NATIVE[variant] as usize as u64
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        PORTABLE[variant].as_ptr() as u64
    }
}
fn oracle() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "native-x86-and-rust-reference"
    } else {
        "rust-reference (host is not x86_64)"
    }
}
fn reference(record: &mut [u8], seed: u64, variant: usize) -> Result<(), String> {
    let result = formula(record, seed, variant);
    #[cfg(target_arch = "x86_64")]
    {
        // SAFETY: fixed SysV fixture only reads/writes this exclusive 64-byte record.
        let native = unsafe { NATIVE[variant](record.as_mut_ptr(), seed) };
        if native != result || word(record, 7) != result {
            return Err("native oracle disagrees with Rust reference".into());
        }
    }
    record[56..64].copy_from_slice(&result.to_le_bytes());
    Ok(())
}
fn context(record: &mut [u8], seed: u64, variant: usize) -> GuestContext {
    GuestContext {
        rax: 0xfedcba9876543210,
        rbx: 11,
        rcx: 12,
        rdx: 13,
        rsi: seed,
        rdi: record.as_mut_ptr() as u64,
        rbp: 14,
        rsp: 0x1800,
        r8: 18,
        r9: 19,
        r10: 20,
        r11: 21,
        r12: 22,
        r13: 23,
        r14: 24,
        r15: 25,
        rip: rip(variant),
    }
}
fn process(
    record: &mut [u8],
    seed: u64,
    variant: usize,
    trace: &PreparedTrace,
    executor: &mut RiscvExecutor,
) -> Result<u64, String> {
    let mut ctx = context(record, seed, variant);
    let mut expected = ctx;
    expected.rax = formula(record, seed, variant);
    expected.rcx = word(record, 6);
    expected.rip = trace.next_rip();
    // SAFETY: approved fixture, private record, valid context and stack bounds.
    let steps = unsafe { trace.execute(&mut ctx, executor) }.map_err(|e| e.to_string())?;
    if ctx != expected || word(record, 7) != expected.rax {
        return Err(format!("register or memory mismatch in kernel {variant}"));
    }
    Ok(steps)
}
#[derive(Default)]
struct Counters {
    bytes: AtomicU64,
    verified: AtomicU64,
    steps: AtomicU64,
    connections: AtomicU64,
}
fn progress(c: &Counters, start: Instant, total: u64) {
    emit("progress", &format!(",\"bytes\":{},\"total_bytes\":{total},\"verified\":{},\"riscv_steps\":{},\"connections\":{},\"elapsed_ms\":{}",c.bytes.load(Ordering::Relaxed),c.verified.load(Ordering::Relaxed),c.steps.load(Ordering::Relaxed),c.connections.load(Ordering::Relaxed),start.elapsed().as_millis()));
}
fn network(
    traces: Arc<Vec<PreparedTrace>>,
    seed: u64,
    total: u64,
    connections: usize,
    rounds: usize,
    cancelled: Arc<AtomicBool>,
    counters: Arc<Counters>,
) -> Result<(), String> {
    let per_connection = total / connections as u64;
    thread::scope(|scope| {
        let mut jobs = vec![];
        for connection in 0..connections {
            let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
            let address = listener.local_addr().map_err(|e| e.to_string())?;
            let mut client = TcpStream::connect(address).map_err(|e| e.to_string())?;
            let (mut server, _) = listener.accept().map_err(|e| e.to_string())?;
            for stream in [&client, &server] {
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .map_err(|e| e.to_string())?;
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .map_err(|e| e.to_string())?;
                stream.set_nodelay(true).map_err(|e| e.to_string())?;
            }
            counters.connections.fetch_add(1, Ordering::Relaxed);
            let t = traces.clone();
            let stop = cancelled.clone();
            let counts = counters.clone();
            jobs.push(scope.spawn(move || -> Result<(), String> {
                let mut executor = RiscvExecutor::default();
                let mut buffer = vec![0u8; 16384];
                let mut done = 0;
                while done < per_connection && !stop.load(Ordering::Relaxed) {
                    let size = (per_connection - done).min(buffer.len() as u64) as usize;
                    server
                        .read_exact(&mut buffer[..size])
                        .map_err(|e| e.to_string())?;
                    let mut steps = 0;
                    for (index, record) in buffer[..size]
                        .as_chunks_mut::<64>()
                        .0
                        .iter_mut()
                        .enumerate()
                    {
                        let variant = (done as usize / 64 + index + connection) % t.len();
                        for round in 0..rounds {
                            if stop.load(Ordering::Relaxed) {
                                return Ok(());
                            }
                            let kernel = (variant + round) % t.len();
                            let round_seed = seed ^ word(record, 7);
                            steps +=
                                process(record, round_seed, kernel, &t[kernel], &mut executor)?;
                        }
                    }
                    server
                        .write_all(&buffer[..size])
                        .map_err(|e| e.to_string())?;
                    counts.steps.fetch_add(steps, Ordering::Relaxed);
                    done += size as u64;
                }
                Ok(())
            }));
            let stop = cancelled.clone();
            let counts = counters.clone();
            let variants = traces.len();
            jobs.push(scope.spawn(move || -> Result<(), String> {
                let mut state = seed.wrapping_add(connection as u64 + 1);
                let mut sent = vec![0u8; 16384];
                let mut received = vec![0u8; 16384];
                let mut done = 0;
                while done < per_connection && !stop.load(Ordering::Relaxed) {
                    let size = (per_connection - done).min(sent.len() as u64) as usize;
                    for bytes in sent[..size].as_chunks_mut::<8>().0 {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        bytes.copy_from_slice(&state.to_le_bytes());
                    }
                    client.write_all(&sent[..size]).map_err(|e| e.to_string())?;
                    for (index, record) in
                        sent[..size].as_chunks_mut::<64>().0.iter_mut().enumerate()
                    {
                        for round in 0..rounds {
                            if stop.load(Ordering::Relaxed) {
                                return Ok(());
                            }
                            let round_seed = seed ^ word(record, 7);
                            reference(
                                record,
                                round_seed,
                                (done as usize / 64 + index + connection + round) % variants,
                            )?;
                        }
                    }
                    client
                        .read_exact(&mut received[..size])
                        .map_err(|e| e.to_string())?;
                    if sent[..size] != received[..size] {
                        return Err(format!(
                            "TCP response mismatch at connection {connection}, byte {done}"
                        ));
                    }
                    counts.bytes.fetch_add(size as u64, Ordering::Relaxed);
                    counts
                        .verified
                        .fetch_add(size as u64 / 64, Ordering::Relaxed);
                    done += size as u64;
                }
                Ok(())
            }));
        }
        let mut failure = None;
        for job in jobs {
            if let Err(error) = job
                .join()
                .map_err(|_| "network thread panicked".to_string())
                .and_then(|x| x)
            {
                failure.get_or_insert(error);
            }
        }
        if let Some(error) = failure {
            Err(error)
        } else {
            Ok(())
        }
    })
}
fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    // Never silently treat an arbitrary binary/path or misspelled flag as a
    // request to run the embedded fixture.
    let mut seen = std::collections::HashSet::new();
    let mut remaining = args.iter().skip(1);
    while let Some(flag) = remaining.next() {
        if !seen.insert(flag.as_str()) {
            return Err(format!("duplicate argument {flag}"));
        }
        match flag.as_str() {
            "--managed" => {}
            "--tier" | "--bytes" | "--seed" => {
                if remaining.next().is_none_or(|value| value.starts_with("--")) {
                    return Err(format!("missing value for {flag}"));
                }
            }
            _ => {
                return Err(format!(
                    "unknown argument {flag:?}; use isr-run to ingest a binary"
                ))
            }
        }
    }
    let get = |flag: &str| {
        args.windows(2)
            .find(|p| p[0] == flag)
            .map(|p| p[1].as_str())
    };
    let tier = get("--tier").unwrap_or("baby");
    let (default_bytes, connections, kernels, rounds) = match tier {
        "baby" => (64u64, 0usize, 1usize, 1usize),
        "medium" => (8 * 1024 * 1024, 4, 4, 128),
        "large" => (512 * 1024 * 1024, 16, 4, 256),
        _ => return Err("unknown tier".into()),
    };
    let total = get("--bytes")
        .map(|v| v.parse::<u64>())
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or(default_bytes);
    if !(64..=1024 * 1024 * 1024).contains(&total)
        || total % (64 * connections.max(1) as u64) != 0
        || (tier == "baby" && total != 64)
    {
        return Err("invalid work size".into());
    }
    let seed = get("--seed")
        .unwrap_or("42")
        .parse::<u64>()
        .map_err(|e| e.to_string())?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let stop = cancelled.clone();
    let managed = args.iter().any(|arg| arg == "--managed");
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            match line {
                Ok(line) if line == "cancel" => {
                    stop.store(true, Ordering::Relaxed);
                    break;
                }
                Err(_) => break,
                _ => {}
            }
        }
        if managed {
            stop.store(true, Ordering::Relaxed);
        }
    });
    emit("started",&format!(",\"tier\":{},\"seed\":{},\"total_bytes\":{total},\"connections\":{connections},\"kernels\":{kernels},\"rounds\":{rounds},\"oracle\":{},\"host_arch\":{},\"backend\":\"riscv64gc-unknown-none-elf\",\"network\":{}",quote(tier),quote(&seed.to_string()),quote(oracle()),quote(std::env::consts::ARCH),quote(if connections==0{"none"}else{"real TCP / 127.0.0.1"})));
    let mut traces = vec![];
    let mut compilation_ns = 0u64;
    let mut unique_lifted_bytes = 0usize;
    let mut unique_lifted_instructions = 0usize;
    for (variant, manifest) in PORTABLE.iter().enumerate().take(kernels) {
        emit_value(
            "stage",
            json!({"kernel":variant, "stage":"ingest", "state":"started", "elapsed_ns":0, "error":null}),
        );
        let ingest_start = Instant::now();
        emit_value(
            "stage",
            json!({"kernel":variant, "stage":"ingest", "state":"completed", "elapsed_ns":ingest_start.elapsed().as_nanos() as u64, "error":null,
            "details":{"input_kind":if cfg!(target_arch="x86_64") {"live native function"} else {"embedded x86 bytes"},
            "source":format!("isr_packet{variant}"), "expected_bytes":manifest.len(), "expected_sha256":sha256(manifest),
            "entry_rip":format!("0x{:016x}",rip(variant)), "scope":"single_trace"}}),
        );
        let mut observer = |event: StageEvent| {
            compilation_ns += event.elapsed_ns;
            emit_stage(Some(variant), event);
        };
        // SAFETY: immutable, complete curated live x86 code; no speculative boundaries.
        let trace =
            unsafe { PreparedTrace::prepare(rip(variant), 0x1000, 0x2000, None, &mut observer) }
                .map_err(|e| e.to_string())?;
        if trace.x86() != manifest {
            return Err("native fixture differs from portable byte manifest".into());
        }
        unique_lifted_bytes += trace.lifted_bytes();
        unique_lifted_instructions += trace.lifted_instruction_count();
        emit_value(
            "artifact",
            json!({"kernel":variant, "x86":hex(trace.x86()), "clif":trace.clif(),
            "riscv":hex(trace.code()), "instructions":trace.lifted_instruction_count(),
            "decoded_instructions":trace.instruction_count(), "lifted_bytes":trace.lifted_bytes(),
            "entry_rip":format!("0x{:016x}",rip(variant)), "exit_rip":format!("0x{:016x}",trace.next_rip()),
            "summary":trace.summary()}),
        );
        traces.push(trace);
    }
    let syscall = [0x0fu8, 0x05];
    let mut stages = vec![];
    // SAFETY: complete readable terminating SYSCALL buffer; it must never execute.
    let rejected = unsafe {
        PreparedTrace::prepare(
            syscall.as_ptr() as u64,
            0x1000,
            0x2000,
            None,
            &mut |event| stages.push(event),
        )
    };
    if !matches!(rejected, Err(EngineError::Security(_)))
        || stages
            .iter()
            .any(|e| e.stage == "lower" || e.stage == "codegen")
    {
        return Err("syscall audit regression".into());
    }
    emit_value(
        "security",
        json!({"syscall_rejected":true, "stage":"audit", "codegen_invoked":false,
        "case":"expected-syscall-rejection", "events":stages}),
    );
    let counters = Arc::new(Counters::default());
    let start = Instant::now();
    emit("stage",",\"kernel\":null,\"stage\":\"execute\",\"state\":\"started\",\"elapsed_ns\":0,\"error\":null");
    let execution = (|| -> Result<(), String> {
        if tier == "baby" {
            let mut record = [0xabu8; 64];
            let mut expected = record;
            reference(&mut expected, seed, 0)?;
            let before = context(&mut record, seed, 0);
            let steps = process(
                &mut record,
                seed,
                0,
                &traces[0],
                &mut RiscvExecutor::default(),
            )?;
            if record != expected {
                return Err("baby memory mismatch".into());
            }
            emit("context",&format!(",\"rax_before\":{},\"rax_after\":{},\"rcx_before\":{},\"rcx_after\":{},\"rip_before\":{},\"rip_after\":{},\"memory_before\":{},\"memory_after\":{}",quote(&format!("0x{:016x}",before.rax)),quote(&format!("0x{:016x}",word(&record,7))),quote(&format!("0x{:016x}",before.rcx)),quote(&format!("0x{:016x}",word(&record,6))),quote(&format!("0x{:016x}",before.rip)),quote(&format!("0x{:016x}",traces[0].next_rip())),quote(&hex(&[0xab;64])),quote(&hex(&record))));
            counters.bytes.store(64, Ordering::Relaxed);
            counters.verified.store(1, Ordering::Relaxed);
            counters.steps.store(steps, Ordering::Relaxed);
        } else {
            let counts = counters.clone();
            let stop = cancelled.clone();
            let (sender, receiver) = std::sync::mpsc::channel();
            let job = thread::spawn(move || {
                let result = network(
                    Arc::new(traces),
                    seed,
                    total,
                    connections,
                    rounds,
                    stop,
                    counts,
                );
                let _ = sender.send(result);
            });
            let result = loop {
                match receiver.recv_timeout(Duration::from_millis(150)) {
                    Ok(result) => break result,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        progress(&counters, start, total)
                    }
                    Err(_) => return Err("network supervisor disconnected".into()),
                }
            };
            job.join().map_err(|_| "network supervisor panicked")?;
            if !cancelled.load(Ordering::Relaxed) {
                result?;
            }
        }
        progress(&counters, start, total);
        if cancelled.load(Ordering::Relaxed) {
            emit_value(
                "stage",
                json!({"kernel":null, "stage":"execute", "state":"cancelled", "elapsed_ns":start.elapsed().as_nanos() as u64, "error":null}),
            );
            emit("cancelled", ",\"reason\":\"cancellation requested\"");
            return Ok(());
        }
        if counters.bytes.load(Ordering::Relaxed) != total
            || counters.verified.load(Ordering::Relaxed) != total / 64
        {
            return Err("incomplete network verification".into());
        }
        Ok(())
    })();
    if let Err(error) = execution {
        emit_value(
            "stage",
            json!({"kernel":null, "stage":"execute", "state":"failed", "elapsed_ns":start.elapsed().as_nanos() as u64, "error":error}),
        );
        return Err(error);
    }
    if cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    emit("stage",&format!(",\"kernel\":null,\"stage\":\"execute\",\"state\":\"completed\",\"elapsed_ns\":{},\"error\":null",start.elapsed().as_nanos()));
    emit("result",&format!(",\"verified\":{},\"bytes\":{total},\"riscv_steps\":{},\"mismatches\":0,\"trace_executions\":{},\"unique_lifted_bytes\":{unique_lifted_bytes},\"unique_lifted_instructions\":{unique_lifted_instructions},\"compiled_traces\":{kernels},\"compilation_ns\":{compilation_ns},\"execution_ns\":{},\"oracle\":{},\"syscall_rejected\":true",counters.verified.load(Ordering::Relaxed),counters.steps.load(Ordering::Relaxed),total/64*rounds as u64,start.elapsed().as_nanos(),quote(oracle())));
    Ok(())
}
fn main() {
    // Managed workers stream into the coordinator's durable SQLite journal.
    // Standalone workers retain the same events locally even without a pipe.
    let ledger = if std::env::args().any(|arg| arg == "--managed") {
        None
    } else {
        let root = std::env::var_os("ISR_LEDGER_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| ".isr-state/pipeline".into());
        match RunLedger::create(&root) {
            Ok(ledger) => {
                eprintln!(
                    "ledger: {}",
                    ledger.directory().join("events.jsonl").display()
                );
                Some(ledger)
            }
            Err(error) => {
                eprintln!("cannot create execution ledger: {error}");
                std::process::exit(1);
            }
        }
    };
    assert!(RUN_LEDGER.set(Mutex::new(ledger)).is_ok());
    if let Err(error) = run() {
        emit("error", &format!(",\"message\":{}", quote(&error)));
        eprintln!("{error}");
        std::process::exit(1);
    }
}

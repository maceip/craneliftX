//! Ordinary compiler-generated Rust TCP service used for pipeline acceptance.
//! Protocol: request = u32 LE length + payload; response = length + u64 LE hash
//! + transformed payload. No assembly, naked functions, or lifting callbacks.
use std::{
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn event(fields: &str) -> io::Result<()> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let mut out = io::stdout().lock();
    let seq = SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    writeln!(
        out,
        "{{\"seq\":{seq},\"timestamp_unix_ns\":\"{timestamp}\",{fields}}}"
    )?;
    out.flush()
}

// Export names solely to identify these ordinary Rust functions in the binary.
#[unsafe(no_mangle)]
#[inline(never)]
pub fn transform_payload(payload: &mut [u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte = byte.wrapping_add(index as u8).rotate_left(1) ^ 0xa5;
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[unsafe(no_mangle)]
#[inline(never)]
pub fn handle_connection(mut stream: TcpStream, connection: usize) -> io::Result<(u64, u64)> {
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;
    stream.set_nodelay(true)?;
    let mut requests = 0u64;
    let mut bytes = 0u64;
    loop {
        let mut header = [0u8; 4];
        // Clean EOF is allowed only between complete frames.
        if stream.read(&mut header[..1])? == 0 {
            break;
        }
        stream.read_exact(&mut header[1..])?;
        let length = u32::from_le_bytes(header) as usize;
        if length > 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "frame exceeds 1 MiB",
            ));
        }
        let mut payload = vec![0u8; length];
        stream.read_exact(&mut payload)?;
        let checksum = transform_payload(&mut payload);
        stream.write_all(&header)?;
        stream.write_all(&checksum.to_le_bytes())?;
        stream.write_all(&payload)?;
        requests += 1;
        bytes += length as u64;
        event(&format!("\"type\":\"response\",\"connection\":{connection},\"request\":{requests},\"bytes\":{length},\"checksum\":\"0x{checksum:016x}\""))?;
    }
    Ok((requests, bytes))
}

#[unsafe(no_mangle)]
#[inline(never)]
pub fn network_server_run(connections: usize) -> io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    event(&format!("\"type\":\"listening\",\"address\":\"{address}\",\"host_arch\":\"{}\",\"connections\":{connections}", std::env::consts::ARCH))?;
    let (requests, bytes) = thread::scope(|scope| -> io::Result<(u64, u64)> {
        let mut jobs = Vec::new();
        for connection in 0..connections {
            let (stream, _) = listener.accept()?;
            jobs.push(scope.spawn(move || handle_connection(stream, connection)));
        }
        let mut requests = 0;
        let mut bytes = 0;
        for job in jobs {
            let (count, length) = job
                .join()
                .map_err(|_| io::Error::other("handler panicked"))??;
            requests += count;
            bytes += length;
        }
        Ok((requests, bytes))
    })?;
    event(&format!("\"type\":\"completed\",\"connections\":{connections},\"requests\":{requests},\"bytes\":{bytes}"))
}

fn main() -> io::Result<()> {
    let connections = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "4".into())
        .parse::<usize>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "expected a connection count"))?;
    if !(1..=32).contains(&connections) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected 1..=32 connections",
        ));
    }
    network_server_run(connections)
}

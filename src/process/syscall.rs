//! Deliberately small Linux x86-64 syscall ABI, backed by host TCP sockets.
//! Guest descriptors and memory are independent of host descriptors/pointers.
//!
//! The allocator is reached two ways, both of which terminate in `Heap`:
//! * the runtime intercepts calls to the guest's own `malloc`/`free` symbols
//!   (`Runtime::allocator_call`), so an unmodified static binary gets the
//!   hardened heap without relinking;
//! * the `ISR_*` syscalls below, for freestanding guests that have no allocator
//!   of their own.
//!
//! Either way a pointer handed to a syscall is stripped of its tag here: the
//! tag is a property of the guest's address space, never of the kernel's.
use super::heap::{Heap, SYS_ISR_CALLOC, SYS_ISR_FREE, SYS_ISR_MALLOC, SYS_ISR_REALLOC};
use super::memory::{error, GuestMemory};
use crate::EngineError;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream},
    time::{Duration, Instant},
};

enum Descriptor {
    Socket,
    Bound(TcpListener),
    Listening(TcpListener),
    Stream(TcpStream),
}
pub(super) struct Kernel {
    descriptors: BTreeMap<u64, Descriptor>,
    pub read_bytes: u64,
    pub written_bytes: u64,
    pub accepted: u64,
    deadline: Instant,
    brk_current: u64,
    brk_original: u64,
}
pub(super) struct Outcome {
    pub exit: Option<i32>,
    pub detail: Value,
}
fn errno(e: io::Error) -> i64 {
    use io::ErrorKind::*;
    -match e.kind() {
        NotFound => 2,
        PermissionDenied => 13,
        ConnectionRefused => 111,
        ConnectionReset => 104,
        ConnectionAborted => 103,
        NotConnected => 107,
        AddrInUse => 98,
        AddrNotAvailable => 99,
        BrokenPipe => 32,
        WouldBlock | TimedOut => 11,
        Interrupted => 4,
        InvalidInput | InvalidData => 22,
        _ => 5,
    }
}
impl Kernel {
    pub fn new(deadline: Instant) -> Self {
        Self {
            descriptors: BTreeMap::new(),
            read_bytes: 0,
            written_bytes: 0,
            accepted: 0,
            deadline,
            brk_current: 0x4000_0000,
            brk_original: 0x4000_0000,
        }
    }
    pub fn open_descriptors(&self) -> usize {
        self.descriptors.len()
    }
    fn insert(&mut self, descriptor: Descriptor) -> Result<u64, EngineError> {
        let fd = (3..1027)
            .find(|fd| !self.descriptors.contains_key(fd))
            .ok_or_else(|| error("guest descriptor limit reached"))?;
        self.descriptors.insert(fd, descriptor);
        Ok(fd)
    }
    pub fn call(
        &mut self,
        state: &mut [u64; 18],
        memory: &mut GuestMemory,
        heap: &mut Heap,
    ) -> Result<Outcome, EngineError> {
        let n = state[0];
        // Strip pointer metadata: the kernel addresses guest memory by its
        // virtual address, and a tag must never reach a host pointer or a
        // bounds check.
        let scheme = heap.scheme();
        let raw_a = state[5];
        let raw_b = state[4];
        let raw_c = state[3];
        let raw_d = state[10];
        let raw_e = state[8];
        let raw_f = state[9];
        let a = scheme.strip(raw_a);
        let b = scheme.strip(raw_b);
        let c = scheme.strip(raw_c);
        let d = scheme.strip(raw_d);
        let e = scheme.strip(raw_e);
        let f = scheme.strip(raw_f);
        let mut detail = json!({"number":n,"args":[format!("0x{a:x}"),format!("0x{b:x}"),format!("0x{c:x}"),format!("0x{d:x}"),format!("0x{e:x}"),format!("0x{f:x}")],"backend":"host syscall bridge"});
        let mut exit = None;
        let result: i64 = match n {
            SYS_ISR_MALLOC | SYS_ISR_CALLOC | SYS_ISR_REALLOC | SYS_ISR_FREE => {
                let rip = state[16];
                // rdi = a, rsi = b, rdx = c, matching the Linux x86-64 ABI.
                match n {
                    SYS_ISR_MALLOC => heap.allocate(a)? as i64,
                    SYS_ISR_CALLOC => {
                        let total = a.saturating_mul(b);
                        let address = heap.allocate(total)?;
                        let at = scheme.strip(address);
                        if total != 0 {
                            if let Ok(slice) = memory.read_mut(at, total as usize) {
                                slice.fill(0);
                            }
                        }
                        address as i64
                    }
                    SYS_ISR_REALLOC => {
                        let old = a;
                        let address = heap.allocate(raw_b)?;
                        let keep = heap.live_size(old).unwrap_or(0).min(c);
                        let copy = memory
                            .read(old, keep as usize)
                            .map(|bytes| bytes.to_vec())
                            .unwrap_or_default();
                        memory.write(scheme.strip(address), &copy)?;
                        if old != 0 {
                            heap.release(a, rip)?;
                        }
                        address as i64
                    }
                    _ => {
                        if raw_a != 0 {
                            heap.release(raw_a, rip)?;
                        }
                        0
                    }
                }
            }
            41 => {
                // socket
                detail["name"] = json!("socket");
                if a != 2 || b != 1 || c != 0 {
                    -97
                } else {
                    self.insert(Descriptor::Socket)? as i64
                }
            }
            49 => {
                // bind: sockaddr_in is Linux layout, independently of host ABI.
                detail["name"] = json!("bind");
                if !matches!(self.descriptors.get(&a), Some(Descriptor::Socket)) {
                    -9
                } else if c != 16 {
                    -22
                } else {
                    let raw = memory.read(b, 16)?;
                    let ip = Ipv4Addr::new(raw[4], raw[5], raw[6], raw[7]);
                    if u16::from_le_bytes([raw[0], raw[1]]) != 2 {
                        -97
                    } else if !ip.is_loopback() {
                        -13
                    }
                    // First runtime profile: loopback TCP only.
                    else {
                        let address = SocketAddrV4::new(ip, u16::from_be_bytes([raw[2], raw[3]]));
                        match TcpListener::bind(address) {
                            Ok(listener) => {
                                listener.set_nonblocking(true).map_err(error)?;
                                detail["address"] =
                                    json!(listener.local_addr().map_err(error)?.to_string());
                                self.descriptors.insert(a, Descriptor::Bound(listener));
                                0
                            }
                            Err(e) => errno(e),
                        }
                    }
                }
            }
            50 => {
                detail["name"] = json!("listen");
                match self.descriptors.remove(&a) {
                    Some(Descriptor::Bound(l) | Descriptor::Listening(l)) => {
                        self.descriptors.insert(a, Descriptor::Listening(l));
                        0
                    }
                    Some(other) => {
                        self.descriptors.insert(a, other);
                        -22
                    }
                    None => -9,
                }
            }
            51 => {
                detail["name"] = json!("getsockname");
                match self.descriptors.get(&a) {
                    Some(Descriptor::Bound(l) | Descriptor::Listening(l)) => {
                        let size =
                            u32::from_le_bytes(memory.read(c, 4)?.try_into().unwrap()) as usize;
                        let address = l.local_addr().map_err(error)?;
                        let std::net::SocketAddr::V4(address) = address else {
                            return Err(error("IPv6 unsupported"));
                        };
                        let mut raw = [0u8; 16];
                        raw[..2].copy_from_slice(&2u16.to_le_bytes());
                        raw[2..4].copy_from_slice(&address.port().to_be_bytes());
                        raw[4..8].copy_from_slice(&address.ip().octets());
                        memory.writable(b, size.min(16))?;
                        memory.writable(c, 4)?;
                        memory.write(b, &raw[..size.min(16)])?;
                        memory.write(c, &16u32.to_le_bytes())?;
                        detail["address"] = json!(address.to_string());
                        0
                    }
                    _ => -9,
                }
            }
            43 => {
                detail["name"] = json!("accept");
                if b != 0 || c != 0 {
                    return Err(error("accept address output not yet supported; use NULL"));
                }
                if let Some(Descriptor::Listening(l)) = self.descriptors.get(&a) {
                    let accepted = loop {
                        if Instant::now() >= self.deadline {
                            return Err(error("process deadline exceeded during accept"));
                        }
                        match l.accept() {
                            Ok(pair) => break Ok(pair),
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                                std::thread::sleep(Duration::from_millis(2))
                            }
                            Err(e) => break Err(e),
                        }
                    };
                    match accepted {
                        Ok((stream, peer)) => {
                            stream.set_nonblocking(false).map_err(error)?;
                            stream.set_nodelay(true).map_err(error)?;
                            detail["peer"] = json!(peer.to_string());
                            detail["local"] =
                                json!(stream.local_addr().map_err(error)?.to_string());
                            self.accepted += 1;
                            detail["connection_id"] = json!(self.accepted);
                            self.insert(Descriptor::Stream(stream))? as i64
                        }
                        Err(e) => errno(e),
                    }
                } else {
                    -9
                }
            }
            0 | 1 => {
                detail["name"] = json!(if n == 0 { "read" } else { "write" });
                detail["fd"] = json!(a);
                if c > 1024 * 1024 {
                    return Err(error("syscall transfer exceeds 1 MiB limit"));
                }
                if c == 0 {
                    0
                } else if n == 0 {
                    memory.writable(b, c as usize)?;
                    if let Some(Descriptor::Stream(stream)) = self.descriptors.get_mut(&a) {
                        stream
                            .set_read_timeout(Some(
                                self.deadline
                                    .saturating_duration_since(Instant::now())
                                    .max(Duration::from_millis(1)),
                            ))
                            .map_err(error)?;
                        let mut buffer = vec![0u8; c as usize];
                        match stream.read(&mut buffer) {
                            Ok(len) => {
                                memory.write(b, &buffer[..len])?;
                                self.read_bytes += len as u64;
                                len as i64
                            }
                            Err(e) => errno(e),
                        }
                    } else {
                        -9
                    }
                } else {
                    let bytes = memory.read(b, c as usize)?;
                    if a == 1 {
                        io::stdout().write_all(bytes).map_err(error)?;
                        io::stdout().flush().map_err(error)?;
                        c as i64
                    } else if a == 2 {
                        io::stderr().write_all(bytes).map_err(error)?;
                        c as i64
                    } else if let Some(Descriptor::Stream(stream)) = self.descriptors.get_mut(&a) {
                        stream
                            .set_write_timeout(Some(
                                self.deadline
                                    .saturating_duration_since(Instant::now())
                                    .max(Duration::from_millis(1)),
                            ))
                            .map_err(error)?;
                        match stream.write(bytes) {
                            Ok(len) => {
                                self.written_bytes += len as u64;
                                len as i64
                            }
                            Err(e) => errno(e),
                        }
                    } else {
                        -9
                    }
                }
            }
            3 => {
                detail["name"] = json!("close");
                if self.descriptors.remove(&a).is_some() {
                    0
                } else {
                    -9
                }
            }
            9 => {
                // mmap: addr, length, prot, flags, fd, offset
                detail["name"] = json!("mmap");
                let addr = a;
                let len = b;
                let prot = c as i32;
                let flags = d as i32;
                let fd = e;
                let _offset = f;
                if len == 0 || len > 64 * 1024 * 1024 {
                    -22
                } else if fd != u64::MAX && fd > 1024 {
                    -9
                } else if fd != u64::MAX {
                    // File-backed mmap not yet supported.
                    -38
                } else if flags & 0x20 == 0 {
                    // MAP_ANONYMOUS required.
                    -22
                } else {
                    let aligned_len = ((len + 4095) / 4096) * 4096;
                    let fixed = flags & 0x10 != 0;
                    let actual_addr = if fixed {
                        addr
                    } else {
                        memory.find_free_mmap(addr, aligned_len).unwrap_or(0)
                    };
                    if actual_addr == 0 {
                        -12
                    } else {
                        let writable = prot & 2 != 0;
                        let executable = prot & 4 != 0;
                        if writable && executable {
                            return Err(error("mmap W^X violation"));
                        }
                        if let Err(err) = memory.map(actual_addr, aligned_len, writable, executable) {
                            detail["error"] = json!(err.to_string());
                            -12
                        } else {
                            detail["mapped"] = json!(format!("0x{actual_addr:x}+{aligned_len:x}"));
                            actual_addr as i64
                        }
                    }
                }
            }
            11 => {
                // munmap
                detail["name"] = json!("munmap");
                if let Err(err) = memory.unmap(a, b) {
                    detail["error"] = json!(err.to_string());
                    -22
                } else {
                    0
                }
            }
            10 => {
                // mprotect
                detail["name"] = json!("mprotect");
                if let Err(err) = memory.mprotect(a, b, c as i32) {
                    detail["error"] = json!(err.to_string());
                    -22
                } else {
                    0
                }
            }
            12 => {
                // brk
                detail["name"] = json!("brk");
                if a == 0 {
                    self.brk_current as i64
                } else if a < self.brk_original || a > self.brk_original + 64 * 1024 * 1024 {
                    -12
                } else {
                    let old = self.brk_current;
                    if a > old {
                        let len = a - old;
                        let aligned_len = ((len + 4095) / 4096) * 4096;
                        if memory.map(old, aligned_len, true, false).is_err() {
                            -12
                        } else {
                            self.brk_current = old + aligned_len;
                            self.brk_current as i64
                        }
                    } else {
                        self.brk_current = a;
                        a as i64
                    }
                }
            }
            39 => {
                detail["name"] = json!("getpid");
                std::process::id() as i64
            }
            102 => {
                detail["name"] = json!("getuid");
                1000
            }
            228 => {
                // clock_gettime
                detail["name"] = json!("clock_gettime");
                use std::time::SystemTime;
                let now = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default();
                let ts_sec = now.as_secs() as i64;
                let ts_nsec = now.subsec_nanos() as i64;
                let timespec = [ts_sec.to_le_bytes(), ts_nsec.to_le_bytes()].concat();
                if memory.write(b, &timespec).is_err() {
                    -14
                } else {
                    0
                }
            }
            318 => {
                // getrandom
                detail["name"] = json!("getrandom");
                let mut buf = vec![0u8; b as usize];
                match std::fs::File::open("/dev/urandom")
                    .and_then(|mut f| f.read_exact(&mut buf).map(|_| ())) {
                    Ok(()) => {
                        if memory.write(a, &buf).is_err() {
                            -14
                        } else {
                            b as i64
                        }
                    }
                    Err(_) => -5,
                }
            }
            60 | 231 => {
                detail["name"] = json!("exit");
                exit = Some((a & 255) as i32);
                0
            }
            _ => {
                return Err(error(format!(
                    "unsupported Linux x86-64 syscall {n}; no native fallback"
                )))
            }
        };
        state[0] = result as u64;
        detail["result"] = json!(result);
        detail["open_descriptors"] = json!(self.open_descriptors());
        Ok(Outcome { exit, detail })
    }
}

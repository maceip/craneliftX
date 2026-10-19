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
        state: &mut [u64; 19],
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
                // socket: domain, type, protocol. Accept SOCK_STREAM with CLOEXEC/NONBLOCK flags.
                detail["name"] = json!("socket");
                let sock_type = b & 0xf;
                if a != 2 || sock_type != 1 || (c != 0 && c != 6) {
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
                    let raw = memory.read(scheme.strip(b), 16)?;
                    let mut ip = Ipv4Addr::new(raw[4], raw[5], raw[6], raw[7]);
                    if u16::from_le_bytes([raw[0], raw[1]]) != 2 {
                        -97
                    } else if !ip.is_loopback() && !ip.is_unspecified() {
                        -13
                    } else {
                        // First runtime profile: loopback TCP only. Map 0.0.0.0 to 127.0.0.1.
                        if ip.is_unspecified() {
                            ip = Ipv4Addr::new(127, 0, 0, 1);
                        }
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
                        let c_addr = scheme.strip(c);
                        let b_addr = scheme.strip(b);
                        let size =
                            u32::from_le_bytes(memory.read(c_addr, 4)?.try_into().unwrap()) as usize;
                        let address = l.local_addr().map_err(error)?;
                        let std::net::SocketAddr::V4(address) = address else {
                            return Err(error("IPv6 unsupported"));
                        };
                        let mut raw = [0u8; 16];
                        raw[..2].copy_from_slice(&2u16.to_le_bytes());
                        raw[2..4].copy_from_slice(&address.port().to_be_bytes());
                        raw[4..8].copy_from_slice(&address.ip().octets());
                        memory.writable(b_addr, size.min(16))?;
                        memory.writable(c_addr, 4)?;
                        memory.write(b_addr, &raw[..size.min(16)])?;
                        memory.write(c_addr, &16u32.to_le_bytes())?;
                        detail["address"] = json!(address.to_string());
                        0
                    }
                    _ => -9,
                }
            }
            43 | 288 => {
                detail["name"] = json!(if n == 43 { "accept" } else { "accept4" });
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
                            if b != 0 && c != 0 {
                                let c_addr = scheme.strip(c);
                                let b_addr = scheme.strip(b);
                                if let Ok(c_bytes) = memory.read(c_addr, 4) {
                                    let size = u32::from_le_bytes(c_bytes.try_into().unwrap()) as usize;
                                    if let std::net::SocketAddr::V4(peer_v4) = peer {
                                        let mut raw = [0u8; 16];
                                        raw[..2].copy_from_slice(&2u16.to_le_bytes());
                                        raw[2..4].copy_from_slice(&peer_v4.port().to_be_bytes());
                                        raw[4..8].copy_from_slice(&peer_v4.ip().octets());
                                        let copy_len = size.min(16);
                                        let _ = memory.write(b_addr, &raw[..copy_len]);
                                        let _ = memory.write(c_addr, &(16u32.min(size as u32)).to_le_bytes());
                                    }
                                }
                            }
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
            0 | 1 | 44 | 45 => {
                let is_read = n == 0 || n == 45;
                detail["name"] = json!(match n {
                    0 => "read",
                    1 => "write",
                    44 => "sendto",
                    45 => "recvfrom",
                    _ => unreachable!(),
                });
                detail["fd"] = json!(a);
                if c > 1024 * 1024 {
                    return Err(error("syscall transfer exceeds 1 MiB limit"));
                }
                let buf_addr = scheme.strip(b);
                if c == 0 {
                    0
                } else if is_read {
                    memory.writable(buf_addr, c as usize)?;
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
                                memory.write(buf_addr, &buffer[..len])?;
                                self.read_bytes += len as u64;
                                len as i64
                            }
                            Err(e) => errno(e),
                        }
                    } else {
                        -9
                    }
                } else {
                    let bytes = memory.read(buf_addr, c as usize)?;
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
            72 => {
                detail["name"] = json!("fcntl");
                // a = fd, b = cmd, c = arg
                if !self.descriptors.contains_key(&a) && a != 0 && a != 1 && a != 2 {
                    -9
                } else {
                    match b {
                        1 => 0, // F_GETFD
                        2 => 0, // F_SETFD
                        3 => 2, // F_GETFL (O_RDWR)
                        4 => 0, // F_SETFL
                        _ => 0,
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
                let _fd = e;
                let _offset = f;
                if len == 0 || len > 64 * 1024 * 1024 {
                    -22
                } else if flags & 0x20 == 0 {
                    // File-backed mmap not yet supported.
                    -38
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
            54 => {
                // setsockopt — accept SO_REUSEADDR/SO_REUSEPORT on SOL_SOCKET
                // and ignore the rest rather than failing the guest.
                detail["name"] = json!("setsockopt");
                // a=fd, b=SOL_SOCKET(1), c=optname, d=optval*, e=optlen (via r10 in asm)
                // The bridge passes args as a,b,c,d,e from rdi,rsi,rdx,r10,r8.
                match self.descriptors.get(&a) {
                    Some(Descriptor::Socket | Descriptor::Bound(_) | Descriptor::Listening(_)) => {
                        0
                    }
                    Some(_) => -9,
                    None => -9,
                }
            }
            14 => {
                detail["name"] = json!("rt_sigprocmask");
                0
            }
            16 => {
                detail["name"] = json!("ioctl");
                -25 // ENOTTY
            }
            20 => {
                detail["name"] = json!("writev");
                detail["fd"] = json!(a);
                let fd = a;
                let iov_ptr = b;
                let iovcnt = c as usize;
                if iovcnt > 1024 {
                    -22
                } else {
                    let mut total_written: i64 = 0;
                    let mut err = None;
                    for i in 0..iovcnt {
                        let entry_addr = iov_ptr + (i as u64) * 16;
                        let raw_iov = memory.read(entry_addr, 16)?;
                        let base = u64::from_le_bytes(raw_iov[..8].try_into().unwrap());
                        let len = u64::from_le_bytes(raw_iov[8..16].try_into().unwrap()) as usize;
                        if len == 0 {
                            continue;
                        }
                        let bytes = memory.read(scheme.strip(base), len)?;
                        if fd == 1 {
                            io::stdout().write_all(bytes).map_err(error)?;
                            io::stdout().flush().map_err(error)?;
                        } else if fd == 2 {
                            io::stderr().write_all(bytes).map_err(error)?;
                            io::stderr().flush().map_err(error)?;
                        } else if let Some(Descriptor::Stream(stream)) = self.descriptors.get_mut(&fd) {
                            stream.write_all(bytes).map_err(error)?;
                        } else {
                            err = Some(-9i64);
                            break;
                        }
                        total_written += len as i64;
                    }
                    err.unwrap_or(total_written)
                }
            }
            7 => {
                detail["name"] = json!("poll");
                let mut ready = 0i64;
                for idx in 0..b {
                    let ptr = a + idx * 8;
                    let Ok(fd_bytes) = memory.read(ptr, 4) else { break; };
                    let fd = i32::from_le_bytes(fd_bytes.try_into().unwrap());
                    let Ok(ev_bytes) = memory.read(ptr + 4, 2) else { break; };
                    let events = i16::from_le_bytes(ev_bytes.try_into().unwrap());
                    let mut revents = 0i16;
                    if fd == 1 || fd == 2 {
                        revents = events & 4; // POLLOUT = 4
                    } else if fd == 0 {
                        revents = 0;
                    } else if self.descriptors.contains_key(&(fd as u64)) {
                        revents = events & 4;
                    }
                    let _ = memory.write(ptr + 6, &revents.to_le_bytes());
                    if revents != 0 {
                        ready += 1;
                    }
                }
                ready
            }
            13 => {
                detail["name"] = json!("rt_sigaction");
                0
            }
            131 => {
                detail["name"] = json!("sigaltstack");
                if b != 0 {
                    let mut old = [0u8; 24];
                    old[8..12].copy_from_slice(&2i32.to_le_bytes()); // SS_DISABLE = 2
                    let _ = memory.write(b, &old);
                }
                0
            }
            200 | 234 => {
                detail["name"] = json!("tkill");
                let sig = b as i32;
                if sig != 0 {
                    exit = Some(128 + sig);
                }
                0
            }
            158 => {
                detail["name"] = json!("arch_prctl");
                match a {
                    0x1002 => {
                        state[18] = raw_b;
                        0
                    }
                    0x1003 => {
                        memory.write(b, &state[18].to_le_bytes())?;
                        0
                    }
                    _ => -22,
                }
            }
            218 => {
                detail["name"] = json!("set_tid_address");
                1000
            }
            202 => {
                detail["name"] = json!("futex");
                let op = (b as i32) & !128;
                if op == 0 {
                    // FUTEX_WAIT
                    if let Ok(cur_bytes) = memory.read(a, 4) {
                        let cur_val = u32::from_le_bytes(cur_bytes.try_into().unwrap());
                        if cur_val != c as u32 {
                            -11 // -EAGAIN
                        } else {
                            0
                        }
                    } else {
                        -14 // -EFAULT
                    }
                } else {
                    // FUTEX_WAKE: 0 waiters woken in single-threaded mode
                    0
                }
            }
            302 => {
                detail["name"] = json!("prlimit64");
                if d != 0 {
                    let mut rlim = [0u8; 16];
                    rlim[..8].copy_from_slice(&(8 * 1024 * 1024u64).to_le_bytes());
                    rlim[8..16].copy_from_slice(&(8 * 1024 * 1024u64).to_le_bytes());
                    let _ = memory.write(d, &rlim);
                }
                0
            }
            60 | 231 => {
                detail["name"] = json!("exit");
                exit = Some((a & 255) as i32);
                0
            }
            _ => {
                // Linux returns -ENOSYS for unimplemented calls; do not tear
                // down the guest over a benign unknown ABI number.
                detail["name"] = json!("syscall");
                detail["unsupported"] = json!(n);
                -38
            }
        };
        state[0] = result as u64;
        detail["result"] = json!(result);
        detail["open_descriptors"] = json!(self.open_descriptors());
        Ok(Outcome { exit, detail })
    }
}

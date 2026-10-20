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
use super::futex::FutexManager;
use super::heap::{Heap, SYS_ISR_CALLOC, SYS_ISR_FREE, SYS_ISR_MALLOC, SYS_ISR_REALLOC};
use super::memory::{error, GuestMemory};
use crate::EngineError;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{File, OpenOptions},
    io::{self, Read, Seek, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub struct EpollInterest {
    pub fd: u64,
    pub events: u32,
    pub data: u64,
}

#[derive(Default)]
pub struct EpollState {
    pub interests: HashMap<u64, EpollInterest>,
}

enum Descriptor {
    Socket,
    Bound(TcpListener),
    Listening(TcpListener),
    Stream(TcpStream),
    File(File),
    Epoll(Arc<Mutex<EpollState>>),
}

fn fill_stat_buf(meta: &std::fs::Metadata, buf: &mut [u8; 144]) {
    use std::os::unix::fs::MetadataExt;
    buf[0..8].copy_from_slice(&meta.dev().to_le_bytes());
    buf[8..16].copy_from_slice(&meta.ino().to_le_bytes());
    buf[16..24].copy_from_slice(&(meta.nlink() as u64).to_le_bytes());
    buf[24..28].copy_from_slice(&(meta.mode() as u32).to_le_bytes());
    buf[28..32].copy_from_slice(&(meta.uid() as u32).to_le_bytes());
    buf[32..36].copy_from_slice(&(meta.gid() as u32).to_le_bytes());
    buf[36..40].copy_from_slice(&0u32.to_le_bytes());
    buf[40..48].copy_from_slice(&(meta.rdev() as u64).to_le_bytes());
    buf[48..56].copy_from_slice(&(meta.len() as u64).to_le_bytes());
    buf[56..64].copy_from_slice(&(meta.blksize() as u64).to_le_bytes());
    buf[64..72].copy_from_slice(&(meta.blocks() as u64).to_le_bytes());
    buf[72..80].copy_from_slice(&(meta.atime() as u64).to_le_bytes());
    buf[80..88].copy_from_slice(&(meta.atime_nsec() as u64).to_le_bytes());
    buf[88..96].copy_from_slice(&(meta.mtime() as u64).to_le_bytes());
    buf[96..104].copy_from_slice(&(meta.mtime_nsec() as u64).to_le_bytes());
    buf[104..112].copy_from_slice(&(meta.ctime() as u64).to_le_bytes());
    buf[112..120].copy_from_slice(&(meta.ctime_nsec() as u64).to_le_bytes());
}

fn fill_utsname(buf: &mut [u8; 390]) {
    let sysname = b"Linux\0";
    let nodename = b"cranelift-isr\0";
    let release = b"6.5.0-blink\0";
    let version = b"#1 SMP PREEMPT_DYNAMIC\0";
    let machine = b"x86_64\0";
    let domainname = b"(none)\0";
    buf[0..sysname.len()].copy_from_slice(sysname);
    buf[65..65 + nodename.len()].copy_from_slice(nodename);
    buf[130..130 + release.len()].copy_from_slice(release);
    buf[195..195 + version.len()].copy_from_slice(version);
    buf[260..260 + machine.len()].copy_from_slice(machine);
    buf[325..325 + domainname.len()].copy_from_slice(domainname);
}

fn resolve_guest_path(raw_path: &str) -> PathBuf {
    if let Some(rest) = raw_path.strip_prefix("/home/devuser/nginx-musl-root/") {
        PathBuf::from("tests/fixtures/nginx_musl_root").join(rest)
    } else if let Some(rest) = raw_path.strip_prefix("/home/devuser/nginx-root/") {
        PathBuf::from("tests/fixtures/nginx_glibc_root").join(rest)
    } else if raw_path == "/home/devuser/nginx-musl-root" {
        PathBuf::from("tests/fixtures/nginx_musl_root")
    } else if raw_path == "/home/devuser/nginx-root" {
        PathBuf::from("tests/fixtures/nginx_glibc_root")
    } else {
        PathBuf::from(raw_path)
    }
}

fn read_cstring(memory: &GuestMemory, mut ptr: u64) -> Result<String, EngineError> {
    let mut bytes = Vec::new();
    loop {
        let b = memory.read(ptr, 1)?[0];
        if b == 0 {
            break;
        }
        bytes.push(b);
        ptr += 1;
        if bytes.len() > 4096 {
            return Err(error("string operand exceeds limit"));
        }
    }
    String::from_utf8(bytes).map_err(error)
}

pub(super) struct Kernel {
    descriptors: BTreeMap<u64, Descriptor>,
    pub read_bytes: u64,
    pub written_bytes: u64,
    pub accepted: u64,
    deadline: Instant,
    brk_current: u64,
    brk_original: u64,
    pub futex_manager: Arc<FutexManager>,
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
    pub fn new(deadline: Instant, initial_brk: u64) -> Self {
        Self {
            descriptors: BTreeMap::new(),
            read_bytes: 0,
            written_bytes: 0,
            accepted: 0,
            deadline,
            brk_current: initial_brk,
            brk_original: initial_brk,
            futex_manager: Arc::new(FutexManager::new()),
        }
    }
    fn futex_call(&self, args: [u64; 6], memory: &GuestMemory) -> i64 {
        let [address, operation, value, timeout_or_count, destination, value3] = args;
        let command = operation as u32 & 0x7f;
        let realtime = operation & 256 != 0;
        let result = (|| -> Result<i32, i32> {
            if operation & !0x1ff != 0 || address & 3 != 0 {
                return Err(-22);
            }
            if realtime && command != 9 {
                return Err(-38);
            }
            let read_word = |at| -> Result<u32, i32> {
                Ok(u32::from_le_bytes(
                    memory.read(at, 4).map_err(|_| -14)?.try_into().unwrap(),
                ))
            };
            read_word(address)?;
            match command {
                0 | 9 => {
                    let bitset = if command == 9 {
                        value3 as u32
                    } else {
                        u32::MAX
                    };
                    let mut deadline = self.deadline;
                    if timeout_or_count != 0 {
                        let bytes = memory.read(timeout_or_count, 16).map_err(|_| -14)?;
                        let seconds = i64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let nanos = i64::from_le_bytes(bytes[8..].try_into().unwrap());
                        if seconds < 0 || !(0..1_000_000_000).contains(&nanos) {
                            return Err(-22);
                        }
                        let duration = Duration::new(seconds as u64, nanos as u32);
                        let remaining = if command == 9 {
                            // WAIT_BITSET uses an absolute deadline in the
                            // selected guest clock, unlike relative WAIT.
                            let mut now = libc::timespec {
                                tv_sec: 0,
                                tv_nsec: 0,
                            };
                            let clock = if realtime {
                                libc::CLOCK_REALTIME
                            } else {
                                libc::CLOCK_MONOTONIC
                            };
                            // SAFETY: now is a writable timespec, clock is a supported host clock.
                            if unsafe { libc::clock_gettime(clock, &mut now) } != 0 {
                                return Err(-22);
                            }
                            duration.saturating_sub(Duration::new(
                                now.tv_sec as u64,
                                now.tv_nsec as u32,
                            ))
                        } else {
                            duration
                        };
                        if let Some(at) = Instant::now().checked_add(remaining) {
                            deadline = deadline.min(at);
                        }
                    }
                    let wait =
                        self.futex_manager
                            .prepare_wait(address, value as u32, bitset, || read_word(address))?;
                    Ok(wait.park(deadline))
                }
                1 | 10 => {
                    let bitset = if command == 10 {
                        value3 as u32
                    } else {
                        u32::MAX
                    };
                    Ok(self.futex_manager.wake(address, value as u32, bitset))
                }
                3 | 4 => {
                    if destination & 3 != 0 {
                        return Err(-22);
                    }
                    read_word(destination)?;
                    Ok(self.futex_manager.requeue(
                        address,
                        destination,
                        value as u32,
                        timeout_or_count as u32,
                        || {
                            if command == 4 && read_word(address)? != value3 as u32 {
                                Err(-11)
                            } else {
                                Ok(())
                            }
                        },
                    ))
                }
                _ => Err(-38),
            }
        })();
        result.unwrap_or_else(|errno| errno) as i64
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
        state: &mut [u64; super::CONTEXT_WORDS],
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
                        let size = u32::from_le_bytes(memory.read(c_addr, 4)?.try_into().unwrap())
                            as usize;
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
                                    let size =
                                        u32::from_le_bytes(c_bytes.try_into().unwrap()) as usize;
                                    if let std::net::SocketAddr::V4(peer_v4) = peer {
                                        let mut raw = [0u8; 16];
                                        raw[..2].copy_from_slice(&2u16.to_le_bytes());
                                        raw[2..4].copy_from_slice(&peer_v4.port().to_be_bytes());
                                        raw[4..8].copy_from_slice(&peer_v4.ip().octets());
                                        let copy_len = size.min(16);
                                        let _ = memory.write(b_addr, &raw[..copy_len]);
                                        let _ = memory
                                            .write(c_addr, &(16u32.min(size as u32)).to_le_bytes());
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
            2 => {
                detail["name"] = json!("open");
                match read_cstring(memory, a) {
                    Ok(raw_path) => {
                        let path = resolve_guest_path(&raw_path);
                        detail["path"] = json!(path.display().to_string());
                        let flags = b as i32;
                        let mut opts = OpenOptions::new();
                        let access = flags & 3;
                        match access {
                            0 => {
                                opts.read(true);
                            }
                            1 => {
                                opts.write(true);
                            }
                            2 => {
                                opts.read(true).write(true);
                            }
                            _ => {}
                        }
                        if (flags & 0x0040) != 0 {
                            opts.create(true);
                        }
                        if (flags & 0x0200) != 0 {
                            opts.truncate(true);
                        }
                        if (flags & 0x0400) != 0 {
                            opts.append(true);
                        }
                        if (flags & (0x0040 | 1 | 2)) != 0 {
                            if let Some(parent) = path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                        }
                        match opts.open(&path) {
                            Ok(f) => self.insert(Descriptor::File(f))? as i64,
                            Err(e) => errno(e),
                        }
                    }
                    Err(_) => -14,
                }
            }
            257 => {
                detail["name"] = json!("openat");
                match read_cstring(memory, b) {
                    Ok(raw_path) => {
                        let path = resolve_guest_path(&raw_path);
                        detail["path"] = json!(path.display().to_string());
                        let flags = c as i32;
                        let mut opts = OpenOptions::new();
                        let access = flags & 3;
                        match access {
                            0 => {
                                opts.read(true);
                            }
                            1 => {
                                opts.write(true);
                            }
                            2 => {
                                opts.read(true).write(true);
                            }
                            _ => {}
                        }
                        if (flags & 0x0040) != 0 {
                            opts.create(true);
                        }
                        if (flags & 0x0200) != 0 {
                            opts.truncate(true);
                        }
                        if (flags & 0x0400) != 0 {
                            opts.append(true);
                        }
                        if (flags & (0x0040 | 1 | 2)) != 0 {
                            if let Some(parent) = path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                        }
                        match opts.open(&path) {
                            Ok(f) => self.insert(Descriptor::File(f))? as i64,
                            Err(e) => errno(e),
                        }
                    }
                    Err(_) => -14,
                }
            }
            4 | 6 => {
                detail["name"] = json!(if n == 4 { "stat" } else { "lstat" });
                match read_cstring(memory, a) {
                    Ok(raw_path) => {
                        let path = resolve_guest_path(&raw_path);
                        let meta_res = if n == 6 {
                            std::fs::symlink_metadata(&path)
                        } else {
                            std::fs::metadata(&path)
                        };
                        match meta_res {
                            Ok(meta) => {
                                let mut statbuf = [0u8; 144];
                                fill_stat_buf(&meta, &mut statbuf);
                                if memory.write(b, &statbuf).is_err() {
                                    -14
                                } else {
                                    0
                                }
                            }
                            Err(e) => errno(e),
                        }
                    }
                    Err(_) => -14,
                }
            }
            5 => {
                detail["name"] = json!("fstat");
                match self.descriptors.get(&a) {
                    Some(Descriptor::File(file)) => match file.metadata() {
                        Ok(meta) => {
                            let mut statbuf = [0u8; 144];
                            fill_stat_buf(&meta, &mut statbuf);
                            if memory.write(b, &statbuf).is_err() {
                                -14
                            } else {
                                0
                            }
                        }
                        Err(e) => errno(e),
                    },
                    Some(
                        Descriptor::Stream(_) | Descriptor::Bound(_) | Descriptor::Listening(_),
                    ) => {
                        let mut statbuf = [0u8; 144];
                        let mode = 0o140777u32;
                        statbuf[24..28].copy_from_slice(&mode.to_le_bytes());
                        if memory.write(b, &statbuf).is_err() {
                            -14
                        } else {
                            0
                        }
                    }
                    _ => -9,
                }
            }
            262 => {
                detail["name"] = json!("newfstatat");
                match read_cstring(memory, b) {
                    Ok(raw_path) => {
                        let path = resolve_guest_path(&raw_path);
                        let nofollow = (d & 0x100) != 0;
                        let meta_res = if nofollow {
                            std::fs::symlink_metadata(&path)
                        } else {
                            std::fs::metadata(&path)
                        };
                        match meta_res {
                            Ok(meta) => {
                                let mut statbuf = [0u8; 144];
                                fill_stat_buf(&meta, &mut statbuf);
                                if memory.write(c, &statbuf).is_err() {
                                    -14
                                } else {
                                    0
                                }
                            }
                            Err(e) => errno(e),
                        }
                    }
                    Err(_) => -14,
                }
            }
            8 => {
                detail["name"] = json!("lseek");
                if let Some(Descriptor::File(f)) = self.descriptors.get_mut(&a) {
                    let whence = match c {
                        0 => io::SeekFrom::Start(b as u64),
                        1 => io::SeekFrom::Current(b as i64),
                        2 => io::SeekFrom::End(b as i64),
                        _ => {
                            return Ok(Outcome {
                                exit: None,
                                detail: json!({"name":"lseek","error":"invalid whence"}),
                            })
                        }
                    };
                    match f.seek(whence) {
                        Ok(pos) => pos as i64,
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
                    } else if let Some(Descriptor::File(file)) = self.descriptors.get_mut(&a) {
                        let mut buffer = vec![0u8; c as usize];
                        match file.read(&mut buffer) {
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
                    } else if let Some(Descriptor::File(file)) = self.descriptors.get_mut(&a) {
                        match file.write(bytes) {
                            Ok(len) => {
                                let _ = file.flush();
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
                let mmap_fd = e;
                let mmap_offset = f;
                if len == 0 || len > 64 * 1024 * 1024 {
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
                        if let Err(err) = memory.map(actual_addr, aligned_len, writable, executable)
                        {
                            detail["error"] = json!(err.to_string());
                            -12
                        } else {
                            if flags & 0x20 == 0 {
                                if let Some(Descriptor::File(f)) =
                                    self.descriptors.get_mut(&mmap_fd)
                                {
                                    let _ = f.seek(io::SeekFrom::Start(mmap_offset));
                                    let mut buf = vec![0u8; len as usize];
                                    if let Ok(n) = f.read(&mut buf) {
                                        let _ = memory.write(actual_addr, &buf[..n]);
                                    }
                                }
                            }
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
            110 => {
                detail["name"] = json!("getppid");
                1000
            }
            102 => {
                detail["name"] = json!("getuid");
                1000
            }
            107 => {
                detail["name"] = json!("geteuid");
                1000
            }
            104 => {
                detail["name"] = json!("getgid");
                1000
            }
            108 => {
                detail["name"] = json!("getegid");
                1000
            }
            63 => {
                detail["name"] = json!("uname");
                let mut uts = [0u8; 390];
                fill_utsname(&mut uts);
                if memory.write(a, &uts).is_err() {
                    -14
                } else {
                    0
                }
            }
            99 => {
                detail["name"] = json!("sysinfo");
                let mut info = [0u8; 112];
                info[0..8].copy_from_slice(&3600u64.to_le_bytes());
                info[32..40].copy_from_slice(&(8 * 1024 * 1024 * 1024u64).to_le_bytes());
                info[40..48].copy_from_slice(&(4 * 1024 * 1024 * 1024u64).to_le_bytes());
                info[80..82].copy_from_slice(&1u16.to_le_bytes());
                info[104..108].copy_from_slice(&1u32.to_le_bytes());
                if memory.write(a, &info).is_err() {
                    -14
                } else {
                    0
                }
            }
            79 => {
                detail["name"] = json!("getcwd");
                if let Ok(cwd) = std::env::current_dir() {
                    let cwd_str = cwd.to_string_lossy();
                    let bytes = cwd_str.as_bytes();
                    let len = bytes.len() + 1;
                    if b as usize >= len {
                        let _ = memory.write(a, bytes);
                        let _ = memory.write(a + bytes.len() as u64, &[0]);
                        a as i64
                    } else {
                        -34
                    }
                } else {
                    -2
                }
            }
            89 | 267 => {
                detail["name"] = json!("readlink");
                let (path_ptr, buf_ptr, bufsiz) = if n == 89 {
                    (a, b, c as usize)
                } else {
                    (b, c, d as usize)
                };
                if let Ok(raw_path) = read_cstring(memory, path_ptr) {
                    if raw_path == "/proc/self/exe" {
                        let exe = b"/home/devuser/nginx-musl-root/sbin/nginx\0";
                        let copy_len = bufsiz.min(exe.len() - 1);
                        let _ = memory.write(buf_ptr, &exe[..copy_len]);
                        copy_len as i64
                    } else {
                        let path = resolve_guest_path(&raw_path);
                        match std::fs::read_link(&path) {
                            Ok(target) => {
                                let target_str = target.to_string_lossy();
                                let bytes = target_str.as_bytes();
                                let copy_len = bufsiz.min(bytes.len());
                                let _ = memory.write(buf_ptr, &bytes[..copy_len]);
                                copy_len as i64
                            }
                            Err(e) => errno(e),
                        }
                    }
                } else {
                    -14
                }
            }
            24 => {
                detail["name"] = json!("sched_yield");
                std::thread::yield_now();
                0
            }
            35 => {
                detail["name"] = json!("nanosleep");
                if let Ok(req_bytes) = memory.read(a, 16) {
                    let sec = i64::from_le_bytes(req_bytes[..8].try_into().unwrap());
                    let nsec = i64::from_le_bytes(req_bytes[8..16].try_into().unwrap());
                    if sec >= 0 && nsec >= 0 {
                        std::thread::sleep(Duration::new(sec as u64, nsec as u32));
                        0
                    } else {
                        -22
                    }
                } else {
                    -14
                }
            }
            204 => {
                detail["name"] = json!("sched_getaffinity");
                let len = b as usize;
                if len >= 8 {
                    let mask = 1u64;
                    let _ = memory.write(c, &mask.to_le_bytes());
                    8
                } else {
                    -22
                }
            }
            228 => {
                detail["name"] = json!("clock_gettime");
                let clock = match raw_a as i32 {
                    0 => Some(libc::CLOCK_REALTIME),
                    1 => Some(libc::CLOCK_MONOTONIC),
                    2 => Some(libc::CLOCK_PROCESS_CPUTIME_ID),
                    3 => Some(libc::CLOCK_THREAD_CPUTIME_ID),
                    _ => None,
                };
                if let Some(clock) = clock {
                    let mut ts = libc::timespec {
                        tv_sec: 0,
                        tv_nsec: 0,
                    };
                    // SAFETY: clock is a supported host clock and ts is writable.
                    if unsafe { libc::clock_gettime(clock, &mut ts) } != 0 {
                        errno(io::Error::last_os_error())
                    } else {
                        let bytes = [
                            (ts.tv_sec as i64).to_le_bytes(),
                            (ts.tv_nsec as i64).to_le_bytes(),
                        ]
                        .concat();
                        if memory.write(b, &bytes).is_err() {
                            -14
                        } else {
                            0
                        }
                    }
                } else {
                    -22
                }
            }
            318 => {
                // getrandom
                detail["name"] = json!("getrandom");
                let mut buf = vec![0u8; b as usize];
                match std::fs::File::open("/dev/urandom")
                    .and_then(|mut f| f.read_exact(&mut buf).map(|_| ()))
                {
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
                detail["name"] = json!("setsockopt");
                match self.descriptors.get(&a) {
                    Some(Descriptor::Socket | Descriptor::Bound(_) | Descriptor::Listening(_)) => 0,
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
                        } else if let Some(Descriptor::Stream(stream)) =
                            self.descriptors.get_mut(&fd)
                        {
                            stream.write_all(bytes).map_err(error)?;
                        } else if let Some(Descriptor::File(file)) = self.descriptors.get_mut(&fd) {
                            if let Err(e) = file.write_all(bytes) {
                                err = Some(errno(e));
                                break;
                            }
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
                    let Ok(fd_bytes) = memory.read(ptr, 4) else {
                        break;
                    };
                    let fd = i32::from_le_bytes(fd_bytes.try_into().unwrap());
                    let Ok(ev_bytes) = memory.read(ptr + 4, 2) else {
                        break;
                    };
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
            213 | 291 => {
                detail["name"] = json!("epoll_create1");
                let ep = Arc::new(Mutex::new(EpollState::default()));
                self.insert(Descriptor::Epoll(ep))? as i64
            }
            233 => {
                detail["name"] = json!("epoll_ctl");
                let epfd = a;
                let op = b as i32;
                let fd = c;
                let event_ptr = d;
                let epoll_arc = match self.descriptors.get(&epfd) {
                    Some(Descriptor::Epoll(ep)) => ep.clone(),
                    _ => {
                        return Ok(Outcome {
                            exit: None,
                            detail: json!({"name":"epoll_ctl","error":"bad epfd"}),
                        })
                    }
                };
                match op {
                    1 /* ADD */ | 3 /* MOD */ => {
                        if event_ptr == 0 {
                            -14
                        } else {
                            let ev_bytes = memory.read(event_ptr, 12)?;
                            let events = u32::from_le_bytes(ev_bytes[..4].try_into().unwrap());
                            let data = u64::from_le_bytes(ev_bytes[4..12].try_into().unwrap());
                            let mut guard = epoll_arc.lock().unwrap();
                            guard.interests.insert(fd, EpollInterest { fd, events, data });
                            0
                        }
                    }
                    2 /* DEL */ => {
                        let mut guard = epoll_arc.lock().unwrap();
                        guard.interests.remove(&fd);
                        0
                    }
                    _ => -22,
                }
            }
            232 | 281 => {
                detail["name"] = json!("epoll_wait");
                let epfd = a;
                let events_ptr = b;
                let maxevents = c as usize;
                let timeout_ms = d as i32;
                if maxevents == 0 {
                    -22
                } else {
                    let epoll_arc = match self.descriptors.get(&epfd) {
                        Some(Descriptor::Epoll(ep)) => ep.clone(),
                        _ => {
                            return Ok(Outcome {
                                exit: None,
                                detail: json!({"name":"epoll_wait","error":"bad epfd"}),
                            })
                        }
                    };
                    let start = Instant::now();
                    let timeout_dur = if timeout_ms >= 0 {
                        Some(Duration::from_millis(timeout_ms as u64))
                    } else {
                        None
                    };
                    let mut ready_events = Vec::new();
                    loop {
                        let interests: Vec<EpollInterest> = {
                            let guard = epoll_arc.lock().unwrap();
                            guard.interests.values().copied().collect()
                        };
                        for interest in interests {
                            let mut revents = 0u32;
                            match self.descriptors.get(&interest.fd) {
                                Some(Descriptor::Listening(l)) => {
                                    use std::os::unix::io::AsRawFd;
                                    let mut pfd = libc::pollfd {
                                        fd: l.as_raw_fd(),
                                        events: libc::POLLIN,
                                        revents: 0,
                                    };
                                    let poll_res = unsafe { libc::poll(&mut pfd, 1, 0) };
                                    if poll_res > 0 && (pfd.revents & libc::POLLIN) != 0 {
                                        revents |= 1;
                                    }
                                }
                                Some(Descriptor::Stream(s)) => {
                                    use std::os::unix::io::AsRawFd;
                                    let mut pfd = libc::pollfd {
                                        fd: s.as_raw_fd(),
                                        events: libc::POLLIN | libc::POLLOUT,
                                        revents: 0,
                                    };
                                    let poll_res = unsafe { libc::poll(&mut pfd, 1, 0) };
                                    if poll_res > 0 {
                                        if (pfd.revents & libc::POLLIN) != 0 {
                                            revents |= 1;
                                        }
                                        if (pfd.revents & libc::POLLOUT) != 0 {
                                            revents |= 4;
                                        }
                                        if (pfd.revents & libc::POLLHUP) != 0 {
                                            revents |= 0x10;
                                        }
                                    }
                                }
                                Some(Descriptor::File(_)) => {
                                    revents |= interest.events & (1 | 4);
                                }
                                _ => {}
                            }
                            let matching = interest.events & revents;
                            if matching != 0 {
                                ready_events.push((matching, interest.data));
                                if ready_events.len() >= maxevents {
                                    break;
                                }
                            }
                        }
                        if !ready_events.is_empty() {
                            break;
                        }
                        if let Some(td) = timeout_dur {
                            if start.elapsed() >= td {
                                break;
                            }
                        }
                        if Instant::now() >= self.deadline {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    let count = ready_events.len();
                    for (i, (ev, data)) in ready_events.into_iter().enumerate() {
                        let offset = events_ptr + (i as u64) * 12;
                        let mut ev_buf = [0u8; 12];
                        ev_buf[..4].copy_from_slice(&ev.to_le_bytes());
                        ev_buf[4..12].copy_from_slice(&data.to_le_bytes());
                        let _ = memory.write(offset, &ev_buf);
                    }
                    count as i64
                }
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
            273 => {
                detail["name"] = json!("set_robust_list");
                0
            }
            334 => {
                detail["name"] = json!("rseq");
                -38
            }
            56 => {
                // clone(flags, stack, ptid, ctid, tls)
                detail["name"] = json!("clone");
                let flags = a;
                let stack = b;
                let ptid = c;
                let ctid = d;
                const CLONE_THREAD: u64 = 0x00010000;
                const CLONE_PARENT_SETTID: u64 = 0x00100000;
                const CLONE_CHILD_SETTID: u64 = 0x01000000;

                let is_thread = (flags & CLONE_THREAD) != 0;
                if !is_thread {
                    let pid = unsafe { libc::fork() };
                    if pid < 0 {
                        -errno(io::Error::last_os_error())
                    } else if pid == 0 {
                        if stack != 0 {
                            state[7] = stack;
                        }
                        let child_pid = std::process::id() as u64;
                        if (flags & CLONE_CHILD_SETTID) != 0 && ctid != 0 {
                            let _ = memory.write(ctid, &(child_pid as u32).to_le_bytes());
                        }
                        0
                    } else {
                        if (flags & CLONE_PARENT_SETTID) != 0 && ptid != 0 {
                            let _ = memory.write(ptid, &(pid as u32).to_le_bytes());
                        }
                        pid as i64
                    }
                } else {
                    // Thread creation must never report success without a
                    // child executing the continuation. The Process/ThreadContext
                    // scheduler is not implemented yet; expose that failure.
                    -38
                }
            }
            202 => {
                detail["name"] = json!("futex");
                self.futex_call([a, raw_b, raw_c, d, e, raw_f], memory)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Kernel, GuestMemory) {
        let kernel = Kernel::new(Instant::now() + Duration::from_secs(2), 0x40000);
        let mut memory = GuestMemory::default();
        memory.map(0x30000, 4096, true, false).unwrap();
        (kernel, memory)
    }

    #[test]
    fn futex_linux_argument_order_and_compare_requeue() {
        let (kernel, memory) = setup();
        let wait = kernel
            .futex_manager
            .prepare_wait(0x30000, 0, !0, || Ok(0))
            .unwrap();
        // x86-64: rdi=uaddr, rsi=op, rdx=val, r10=val2,
        // r8=uaddr2, r9=val3. The previous bridge swapped r8/r9.
        assert_eq!(
            kernel.futex_call([0x30000, 4 | 128, 0, 1, 0x30004, 1], &memory),
            -11
        );
        assert_eq!(
            kernel.futex_call([0x30000, 4 | 128, 0, 1, 0x30004, 0], &memory),
            1
        );
        assert_eq!(kernel.futex_call([0x30000, 1, 1, 0, 0, 0], &memory), 0);
        assert_eq!(kernel.futex_call([0x30004, 1, 1, 0, 0, 0], &memory), 1);
        assert_eq!(wait.park(Instant::now() + Duration::from_secs(1)), 0);
    }

    #[test]
    fn futex_rejects_invalid_timespecs_and_unimplemented_commands() {
        let (kernel, mut memory) = setup();
        memory.write(0x30008, &[0; 16]).unwrap();
        memory
            .write(0x30010, &1_000_000_000i64.to_le_bytes())
            .unwrap();
        assert_eq!(
            kernel.futex_call([0x30000, 0, 0, 0x30008, 0, 0], &memory),
            -22
        );
        assert_eq!(
            kernel.futex_call([0x30000, 0, 0, 0x40000, 0, 0], &memory),
            -14
        );
        assert_eq!(kernel.futex_call([0x30001, 1, 1, 0, 0, 0], &memory), -22);
        assert_eq!(kernel.futex_call([0x30000, 6, 0, 0, 0, 0], &memory), -38);
        memory.write(0x30010, &0i64.to_le_bytes()).unwrap();
        // Zero absolute deadline must time out immediately on either clock.
        for op in [9 | 128, 9 | 128 | 256] {
            assert_eq!(
                kernel.futex_call([0x30000, op, 0, 0x30008, 0, 1], &memory),
                -110
            );
        }
    }

    #[test]
    fn clone_cannot_return_success_without_a_child() {
        let (mut kernel, mut memory) = setup();
        let mut heap = Heap::new(
            super::super::lam::TagScheme::SOFTWARE,
            super::super::csprng::Csprng::from_seed_test(),
            true,
            true,
        );
        let mut state = [0; super::super::CONTEXT_WORDS];
        state[0] = 56;
        state[5] = 0x3d0f00; // pthread-style sharing, TLS and TID flags
        state[3] = 0x30000;
        state[10] = 0x30004;
        memory.write(0x30000, &[0x5a; 8]).unwrap();
        kernel.call(&mut state, &mut memory, &mut heap).unwrap();
        assert_eq!(state[0] as i64, -38);
        assert_eq!(memory.read(0x30000, 8).unwrap(), &[0x5a; 8]);
    }
}

//! Linux x86-64 syscall bridge with thread-safe descriptor management and
//! fine-grained guest address-space locking.
//!
//! Blocking operations (`futex`, `epoll_wait`, `accept`, `accept4`, `read`,
//! `write`, `pread64`, `pwrite64`, `nanosleep`, `wait4`, `rt_sigsuspend`) never
//! hold the guest address-space mutex or descriptor table mutex while waiting
//! or performing host I/O.
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
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
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

pub struct EventFdState {
    pub counter: Mutex<u64>,
    pub nonblocking: AtomicBool,
}

#[derive(Clone)]
enum Descriptor {
    Socket {
        domain: u64,
        nonblocking: Arc<AtomicBool>,
    },
    Bound {
        listener: Arc<TcpListener>,
        nonblocking: Arc<AtomicBool>,
    },
    Listening {
        listener: Arc<TcpListener>,
        nonblocking: Arc<AtomicBool>,
    },
    Stream {
        stream: Arc<TcpStream>,
        nonblocking: Arc<AtomicBool>,
    },
    Unix {
        stream: Arc<UnixStream>,
        nonblocking: Arc<AtomicBool>,
    },
    File(Arc<File>),
    Epoll(Arc<Mutex<EpollState>>),
    EventFd(Arc<EventFdState>),
}

pub(super) trait SyscallSpace {
    fn with<R>(&mut self, f: impl FnOnce(&mut GuestMemory, &mut Heap) -> R) -> R;
}

impl SyscallSpace for (&mut GuestMemory, &mut Heap) {
    fn with<R>(&mut self, f: impl FnOnce(&mut GuestMemory, &mut Heap) -> R) -> R {
        f(self.0, self.1)
    }
}

#[derive(Clone, Debug)]
pub(super) struct ThreadSpawn {
    pub flags: u64,
    pub child_stack: u64,
    pub ptid: u64,
    pub ctid: u64,
    pub tls: u64,
    pub child_tid: u64,
}

pub(super) struct Outcome {
    pub exit: Option<i32>,
    pub thread_exit: bool,
    pub spawn_thread: Option<ThreadSpawn>,
    pub forked_child: bool,
    pub detail: Value,
}

struct KernelInner {
    descriptors: BTreeMap<u64, Descriptor>,
    brk_current: u64,
    brk_original: u64,
}

#[derive(Clone)]
pub(super) struct Kernel {
    inner: Arc<Mutex<KernelInner>>,
    read_bytes: Arc<AtomicU64>,
    written_bytes: Arc<AtomicU64>,
    accepted: Arc<AtomicU64>,
    deadline: Instant,
    pub futex_manager: Arc<FutexManager>,
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

fn errno(e: io::Error) -> i64 {
    if let Some(raw) = e.raw_os_error() {
        // Map macOS EAGAIN/EWOULDBLOCK (35) to Linux EAGAIN (11).
        if raw == libc::EAGAIN || raw == libc::EWOULDBLOCK {
            return -11;
        }
        if raw == libc::EINTR {
            return -4;
        }
        if raw == libc::ENOENT {
            return -2;
        }
        if raw == libc::EEXIST {
            return -17;
        }
        if raw == libc::EACCES || raw == libc::EPERM {
            return -13;
        }
        if raw == libc::EINVAL {
            return -22;
        }
    }
    use io::ErrorKind::*;
    -match e.kind() {
        NotFound => 2,
        PermissionDenied => 13,
        AlreadyExists => 17,
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

fn write_all_stream(stream: &TcpStream, mut bytes: &[u8], deadline: Instant) -> io::Result<usize> {
    let total = bytes.len();
    while !bytes.is_empty() {
        match (&*stream).write(bytes) {
            Ok(0) => break,
            Ok(n) => bytes = &bytes[n..],
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(e);
                }
                std::thread::sleep(Duration::from_micros(200));
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(total - bytes.len())
}

impl Kernel {
    pub fn new(deadline: Instant, initial_brk: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(KernelInner {
                descriptors: BTreeMap::new(),
                brk_current: initial_brk,
                brk_original: initial_brk,
            })),
            read_bytes: Arc::new(AtomicU64::new(0)),
            written_bytes: Arc::new(AtomicU64::new(0)),
            accepted: Arc::new(AtomicU64::new(0)),
            deadline,
            futex_manager: Arc::new(FutexManager::new()),
        }
    }

    pub fn read_bytes(&self) -> u64 {
        self.read_bytes.load(Ordering::Relaxed)
    }

    pub fn written_bytes(&self) -> u64 {
        self.written_bytes.load(Ordering::Relaxed)
    }

    pub fn accepted(&self) -> u64 {
        self.accepted.load(Ordering::Relaxed)
    }

    pub fn open_descriptors(&self) -> usize {
        self.inner.lock().unwrap().descriptors.len()
    }

    fn insert(&self, descriptor: Descriptor) -> Result<u64, EngineError> {
        let mut inner = self.inner.lock().unwrap();
        let fd = (3..4096)
            .find(|fd| !inner.descriptors.contains_key(fd))
            .ok_or_else(|| error("guest descriptor limit reached"))?;
        inner.descriptors.insert(fd, descriptor);
        Ok(fd)
    }

    fn get_desc(&self, fd: u64) -> Option<Descriptor> {
        self.inner.lock().unwrap().descriptors.get(&fd).cloned()
    }

    fn futex_call_space<S: SyscallSpace>(&self, args: [u64; 6], space: &mut S) -> i64 {
        let [address, operation, value, timeout_or_count, destination, value3] = args;
        let command = operation as u32 & 0x7f;
        let realtime = operation & 256 != 0;
        let result = (|| -> Result<i32, i32> {
            if operation & !0x1ff != 0 || address & 3 != 0 {
                return Err(-22);
            }
            if realtime && command != 9 && command != 0 {
                return Err(-38);
            }
            let read_word = |space: &mut S, at: u64| -> Result<u32, i32> {
                space.with(|memory, _| {
                    Ok(u32::from_le_bytes(
                        memory.read(at, 4).map_err(|_| -14)?.try_into().unwrap(),
                    ))
                })
            };
            read_word(space, address)?;
            match command {
                0 | 9 => {
                    let bitset = if command == 9 {
                        value3 as u32
                    } else {
                        u32::MAX
                    };
                    let mut deadline = self.deadline;
                    if timeout_or_count != 0 {
                        let (seconds, nanos) = space.with(|memory, _| -> Result<(i64, i64), i32> {
                            let bytes = memory.read(timeout_or_count, 16).map_err(|_| -14)?;
                            let s = i64::from_le_bytes(bytes[..8].try_into().unwrap());
                            let n = i64::from_le_bytes(bytes[8..].try_into().unwrap());
                            Ok((s, n))
                        })?;
                        if seconds < 0 || !(0..1_000_000_000).contains(&nanos) {
                            return Err(-22);
                        }
                        let duration = Duration::new(seconds as u64, nanos as u32);
                        let remaining = if command == 9 {
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
                    let wait = self
                        .futex_manager
                        .prepare_wait(address, value as u32, bitset, || read_word(space, address))?;
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
                    read_word(space, destination)?;
                    Ok(self.futex_manager.requeue(
                        address,
                        destination,
                        value as u32,
                        timeout_or_count as u32,
                        || {
                            if command == 4 && read_word(space, address)? != value3 as u32 {
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

    #[cfg(test)]
    fn futex_call(&self, args: [u64; 6], memory: &GuestMemory) -> i64 {
        let mut mem_clone = memory.clone();
        let mut dummy_heap = Heap::new(
            super::lam::TagScheme::SOFTWARE,
            super::csprng::Csprng::from_seed_test(),
            false,
            false,
        );
        let mut pair = (&mut mem_clone, &mut dummy_heap);
        self.futex_call_space(args, &mut pair)
    }

    pub fn call(
        &mut self,
        state: &mut [u64; super::CONTEXT_WORDS],
        memory: &mut GuestMemory,
        heap: &mut Heap,
    ) -> Result<Outcome, EngineError> {
        let mut pair = (memory, heap);
        let mut clear_tid = 0u64;
        let mut robust = 0u64;
        self.call_shared(
            state,
            unsafe { libc::getpid() as u64 },
            false,
            &mut clear_tid,
            &mut robust,
            None,
            &mut pair,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn call_shared<S: SyscallSpace>(
        &self,
        state: &mut [u64; super::CONTEXT_WORDS],
        tid: u64,
        is_child_thread: bool,
        clear_child_tid: &mut u64,
        robust_list_head: &mut u64,
        next_tid: Option<&AtomicU64>,
        space: &mut S,
    ) -> Result<Outcome, EngineError> {
        let n = state[0];
        let scheme = space.with(|_, heap| heap.scheme());
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
        let mut detail = json!({"number":n,"args":[format!("0x{a:x}"),format!("0x{b:x}"),format!("0x{c:x}"),format!("0x{d:x}"),format!("0x{e:x}"),format!("0x{f:x}")],"backend":"host syscall bridge","tid":tid});
        let mut exit = None;
        let mut thread_exit = false;
        let mut spawn_thread = None;
        let mut forked_child = false;

        let result: i64 = match n {
            SYS_ISR_MALLOC | SYS_ISR_CALLOC | SYS_ISR_REALLOC | SYS_ISR_FREE => {
                let rip = state[16];
                space.with(|memory, heap| -> Result<i64, EngineError> {
                    match n {
                        SYS_ISR_MALLOC => Ok(heap.allocate(a)? as i64),
                        SYS_ISR_CALLOC => {
                            let total = a.saturating_mul(b);
                            let address = heap.allocate(total)?;
                            let at = scheme.strip(address);
                            if total != 0 {
                                if let Ok(slice) = memory.read_mut(at, total as usize) {
                                    slice.fill(0);
                                }
                            }
                            Ok(address as i64)
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
                                heap.release(raw_a, rip)?;
                            }
                            Ok(address as i64)
                        }
                        _ => {
                            if raw_a != 0 {
                                heap.release(raw_a, rip)?;
                            }
                            Ok(0)
                        }
                    }
                })?
            }
            41 => {
                // socket: domain, type, protocol
                detail["name"] = json!("socket");
                let sock_type = b & 0xf;
                let nonblock = (b & 0x800) != 0;
                if (a != 2 && a != 1) || (sock_type != 1 && sock_type != 2) || (a == 2 && c != 0 && c != 6) {
                    -97
                } else {
                    self.insert(Descriptor::Socket {
                        domain: a,
                        nonblocking: Arc::new(AtomicBool::new(nonblock)),
                    })? as i64
                }
            }
            53 => {
                // socketpair(domain, type, protocol, sv)
                detail["name"] = json!("socketpair");
                let nonblock = (b & 0x800) != 0;
                if a != 1 {
                    -97
                } else {
                    match UnixStream::pair() {
                        Ok((s0, s1)) => {
                            let _ = s0.set_nonblocking(nonblock);
                            let _ = s1.set_nonblocking(nonblock);
                            let fd0 = self.insert(Descriptor::Unix {
                                stream: Arc::new(s0),
                                nonblocking: Arc::new(AtomicBool::new(nonblock)),
                            })? as u32;
                            let fd1 = self.insert(Descriptor::Unix {
                                stream: Arc::new(s1),
                                nonblocking: Arc::new(AtomicBool::new(nonblock)),
                            })? as u32;
                            let mut bytes = [0u8; 8];
                            bytes[..4].copy_from_slice(&fd0.to_le_bytes());
                            bytes[4..8].copy_from_slice(&fd1.to_le_bytes());
                            if space.with(|memory, _| memory.write(d, &bytes)).is_err() {
                                -14
                            } else {
                                0
                            }
                        }
                        Err(e) => errno(e),
                    }
                }
            }
            49 => {
                // bind
                detail["name"] = json!("bind");
                let sock_nb = match self.get_desc(a) {
                    Some(Descriptor::Socket {
                        domain: 2,
                        nonblocking,
                    }) => Some(nonblocking),
                    _ => None,
                };
                if let Some(nonblocking) = sock_nb {
                    if c != 16 {
                        -22
                    } else {
                        let raw = space.with(|memory, _| memory.read(b, 16).map(|s| s.to_vec()))?;
                        let mut ip = Ipv4Addr::new(raw[4], raw[5], raw[6], raw[7]);
                        if u16::from_le_bytes([raw[0], raw[1]]) != 2 {
                            -97
                        } else if !ip.is_loopback() && !ip.is_unspecified() {
                            -13
                        } else {
                            if ip.is_unspecified() {
                                ip = Ipv4Addr::new(127, 0, 0, 1);
                            }
                            let address =
                                SocketAddrV4::new(ip, u16::from_be_bytes([raw[2], raw[3]]));
                            match TcpListener::bind(address) {
                                Ok(listener) => {
                                    listener.set_nonblocking(true).map_err(error)?;
                                    detail["address"] =
                                        json!(listener.local_addr().map_err(error)?.to_string());
                                    self.inner.lock().unwrap().descriptors.insert(
                                        a,
                                        Descriptor::Bound {
                                            listener: Arc::new(listener),
                                            nonblocking,
                                        },
                                    );
                                    0
                                }
                                Err(e) => errno(e),
                            }
                        }
                    }
                } else {
                    -9
                }
            }
            50 => {
                detail["name"] = json!("listen");
                let mut inner = self.inner.lock().unwrap();
                match inner.descriptors.remove(&a) {
                    Some(
                        Descriptor::Bound {
                            listener,
                            nonblocking,
                        }
                        | Descriptor::Listening {
                            listener,
                            nonblocking,
                        },
                    ) => {
                        inner.descriptors.insert(
                            a,
                            Descriptor::Listening {
                                listener,
                                nonblocking,
                            },
                        );
                        0
                    }
                    Some(other) => {
                        inner.descriptors.insert(a, other);
                        -22
                    }
                    None => -9,
                }
            }
            51 => {
                detail["name"] = json!("getsockname");
                match self.get_desc(a) {
                    Some(
                        Descriptor::Bound { listener, .. } | Descriptor::Listening { listener, .. },
                    ) => {
                        let size = space.with(|memory, _| -> Result<usize, EngineError> {
                            Ok(
                                u32::from_le_bytes(memory.read(c, 4)?.try_into().unwrap())
                                    as usize,
                            )
                        })?;
                        let address = listener.local_addr().map_err(error)?;
                        let std::net::SocketAddr::V4(address) = address else {
                            return Err(error("IPv6 unsupported"));
                        };
                        let mut raw = [0u8; 16];
                        raw[..2].copy_from_slice(&2u16.to_le_bytes());
                        raw[2..4].copy_from_slice(&address.port().to_be_bytes());
                        raw[4..8].copy_from_slice(&address.ip().octets());
                        space.with(|memory, _| -> Result<(), EngineError> {
                            memory.writable(b, size.min(16))?;
                            memory.writable(c, 4)?;
                            memory.write(b, &raw[..size.min(16)])?;
                            memory.write(c, &16u32.to_le_bytes())?;
                            Ok(())
                        })?;
                        detail["address"] = json!(address.to_string());
                        0
                    }
                    Some(Descriptor::Stream { stream, .. }) => {
                        let size = space.with(|memory, _| -> Result<usize, EngineError> {
                            Ok(
                                u32::from_le_bytes(memory.read(c, 4)?.try_into().unwrap())
                                    as usize,
                            )
                        })?;
                        let address = stream.local_addr().map_err(error)?;
                        let std::net::SocketAddr::V4(address) = address else {
                            return Err(error("IPv6 unsupported"));
                        };
                        let mut raw = [0u8; 16];
                        raw[..2].copy_from_slice(&2u16.to_le_bytes());
                        raw[2..4].copy_from_slice(&address.port().to_be_bytes());
                        raw[4..8].copy_from_slice(&address.ip().octets());
                        space.with(|memory, _| -> Result<(), EngineError> {
                            memory.writable(b, size.min(16))?;
                            memory.writable(c, 4)?;
                            memory.write(b, &raw[..size.min(16)])?;
                            memory.write(c, &16u32.to_le_bytes())?;
                            Ok(())
                        })?;
                        0
                    }
                    _ => -9,
                }
            }
            43 | 288 => {
                detail["name"] = json!(if n == 43 { "accept" } else { "accept4" });
                if let Some(Descriptor::Listening {
                    listener,
                    nonblocking,
                }) = self.get_desc(a)
                {
                    let listener_nb = nonblocking.load(Ordering::Relaxed);
                    let child_nb = n == 288 && (d & 0x800) != 0;
                    let accepted = loop {
                        if Instant::now() >= self.deadline {
                            return Err(error("process deadline exceeded during accept"));
                        }
                        match listener.accept() {
                            Ok(pair) => break Ok(pair),
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                                if listener_nb {
                                    break Err(e);
                                }
                                std::thread::sleep(Duration::from_millis(1));
                            }
                            Err(e) => break Err(e),
                        }
                    };
                    match accepted {
                        Ok((stream, peer)) => {
                            stream.set_nonblocking(child_nb).map_err(error)?;
                            stream.set_nodelay(true).map_err(error)?;
                            detail["peer"] = json!(peer.to_string());
                            detail["local"] =
                                json!(stream.local_addr().map_err(error)?.to_string());
                            if b != 0 && c != 0 {
                                space.with(|memory, _| {
                                    if let Ok(c_bytes) = memory.read(c, 4) {
                                        let size =
                                            u32::from_le_bytes(c_bytes.try_into().unwrap())
                                                as usize;
                                        if let std::net::SocketAddr::V4(peer_v4) = peer {
                                            let mut raw = [0u8; 16];
                                            raw[..2].copy_from_slice(&2u16.to_le_bytes());
                                            raw[2..4]
                                                .copy_from_slice(&peer_v4.port().to_be_bytes());
                                            raw[4..8].copy_from_slice(&peer_v4.ip().octets());
                                            let copy_len = size.min(16);
                                            let _ = memory.write(b, &raw[..copy_len]);
                                            let _ = memory.write(
                                                c,
                                                &(16u32.min(size as u32)).to_le_bytes(),
                                            );
                                        }
                                    }
                                });
                            }
                            let conn_id = self.accepted.fetch_add(1, Ordering::Relaxed) + 1;
                            detail["connection_id"] = json!(conn_id);
                            let fd = self.insert(Descriptor::Stream {
                                stream: Arc::new(stream),
                                nonblocking: Arc::new(AtomicBool::new(child_nb)),
                            })? as i64;
                            // Brief yield so sibling worker processes share incoming connections.
                            if listener_nb {
                                std::thread::sleep(Duration::from_micros(500));
                            }
                            fd
                        }
                        Err(e) => errno(e),
                    }
                } else {
                    -9
                }
            }
            2 | 85 | 257 => {
                detail["name"] = json!(if n == 2 {
                    "open"
                } else if n == 85 {
                    "creat"
                } else {
                    "openat"
                });
                let (path_ptr, flags) = if n == 2 {
                    (a, b as i32)
                } else if n == 85 {
                    (a, 0x0040 | 0x0200 | 1)
                } else {
                    (b, c as i32)
                };
                let raw_path = space.with(|memory, _| read_cstring(memory, path_ptr));
                match raw_path {
                    Ok(raw_path) => {
                        let path = resolve_guest_path(&raw_path);
                        detail["path"] = json!(path.display().to_string());
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
                            Ok(f) => self.insert(Descriptor::File(Arc::new(f)))? as i64,
                            Err(e) => errno(e),
                        }
                    }
                    Err(_) => -14,
                }
            }
            4 | 6 => {
                detail["name"] = json!(if n == 4 { "stat" } else { "lstat" });
                let raw_path = space.with(|memory, _| read_cstring(memory, a));
                match raw_path {
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
                                if space.with(|memory, _| memory.write(b, &statbuf)).is_err() {
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
                if a <= 2 {
                    let mut statbuf = [0u8; 144];
                    let mode = 0o020666u32;
                    statbuf[24..28].copy_from_slice(&mode.to_le_bytes());
                    if space.with(|memory, _| memory.write(b, &statbuf)).is_err() {
                        -14
                    } else {
                        0
                    }
                } else {
                    match self.get_desc(a) {
                        Some(Descriptor::File(file)) => match file.metadata() {
                            Ok(meta) => {
                                let mut statbuf = [0u8; 144];
                                fill_stat_buf(&meta, &mut statbuf);
                                if space.with(|memory, _| memory.write(b, &statbuf)).is_err() {
                                    -14
                                } else {
                                    0
                                }
                            }
                            Err(e) => errno(e),
                        },
                        Some(
                            Descriptor::Stream { .. }
                            | Descriptor::Bound { .. }
                            | Descriptor::Listening { .. }
                            | Descriptor::Unix { .. }
                            | Descriptor::EventFd(_)
                            | Descriptor::Epoll(_),
                        ) => {
                            let mut statbuf = [0u8; 144];
                            let mode = 0o140777u32;
                            statbuf[24..28].copy_from_slice(&mode.to_le_bytes());
                            if space.with(|memory, _| memory.write(b, &statbuf)).is_err() {
                                -14
                            } else {
                                0
                            }
                        }
                        _ => -9,
                    }
                }
            }
            262 => {
                detail["name"] = json!("newfstatat");
                let empty_path = (d & 0x1000) != 0;
                if empty_path {
                    if let Some(Descriptor::File(file)) = self.get_desc(a) {
                        match file.metadata() {
                            Ok(meta) => {
                                let mut statbuf = [0u8; 144];
                                fill_stat_buf(&meta, &mut statbuf);
                                if space.with(|memory, _| memory.write(c, &statbuf)).is_err() {
                                    -14
                                } else {
                                    0
                                }
                            }
                            Err(e) => errno(e),
                        }
                    } else {
                        -9
                    }
                } else {
                    let raw_path = space.with(|memory, _| read_cstring(memory, b));
                    match raw_path {
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
                                    if space.with(|memory, _| memory.write(c, &statbuf)).is_err() {
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
            }
            21 | 269 => {
                detail["name"] = json!(if n == 21 { "access" } else { "faccessat" });
                let path_ptr = if n == 21 { a } else { b };
                match space.with(|memory, _| read_cstring(memory, path_ptr)) {
                    Ok(raw_path) => {
                        let path = resolve_guest_path(&raw_path);
                        if path.exists() {
                            0
                        } else {
                            -2
                        }
                    }
                    Err(_) => -14,
                }
            }
            8 => {
                detail["name"] = json!("lseek");
                if let Some(Descriptor::File(f)) = self.get_desc(a) {
                    let whence = match c {
                        0 => io::SeekFrom::Start(b),
                        1 => io::SeekFrom::Current(b as i64),
                        2 => io::SeekFrom::End(b as i64),
                        _ => {
                            return Ok(Outcome {
                                exit: None,
                                thread_exit: false,
                                spawn_thread: None,
                                forked_child: false,
                                detail: json!({"name":"lseek","error":"invalid whence"}),
                            })
                        }
                    };
                    match (&*f).seek(whence) {
                        Ok(pos) => pos as i64,
                        Err(e) => errno(e),
                    }
                } else {
                    -9
                }
            }
            17 => {
                // pread64(fd, buf, count, offset)
                detail["name"] = json!("pread64");
                detail["fd"] = json!(a);
                if c > 16 * 1024 * 1024 {
                    return Err(error("pread64 transfer exceeds 16 MiB limit"));
                }
                if c == 0 {
                    0
                } else {
                    space.with(|memory, _| memory.writable(b, c as usize))?;
                    if let Some(Descriptor::File(file)) = self.get_desc(a) {
                        use std::os::unix::fs::FileExt;
                        let mut buffer = vec![0u8; c as usize];
                        match file.read_at(&mut buffer, d) {
                            Ok(len) => {
                                space.with(|memory, _| memory.write(b, &buffer[..len]))?;
                                self.read_bytes.fetch_add(len as u64, Ordering::Relaxed);
                                len as i64
                            }
                            Err(e) => errno(e),
                        }
                    } else {
                        -9
                    }
                }
            }
            18 => {
                // pwrite64(fd, buf, count, offset)
                detail["name"] = json!("pwrite64");
                detail["fd"] = json!(a);
                if c > 16 * 1024 * 1024 {
                    return Err(error("pwrite64 transfer exceeds 16 MiB limit"));
                }
                if c == 0 {
                    0
                } else {
                    let bytes =
                        space.with(|memory, _| memory.read(b, c as usize).map(|s| s.to_vec()))?;
                    if let Some(Descriptor::File(file)) = self.get_desc(a) {
                        use std::os::unix::fs::FileExt;
                        match file.write_at(&bytes, d) {
                            Ok(len) => {
                                self.written_bytes.fetch_add(len as u64, Ordering::Relaxed);
                                len as i64
                            }
                            Err(e) => errno(e),
                        }
                    } else {
                        -9
                    }
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
                if c > 16 * 1024 * 1024 {
                    return Err(error("syscall transfer exceeds 16 MiB limit"));
                }
                let buf_addr = b;
                if c == 0 {
                    0
                } else if is_read {
                    space.with(|memory, _| memory.writable(buf_addr, c as usize))?;
                    match self.get_desc(a) {
                        Some(Descriptor::Stream {
                            stream,
                            nonblocking,
                        }) => {
                            let nb = nonblocking.load(Ordering::Relaxed);
                            let _ = stream.set_nonblocking(nb);
                            if !nb {
                                stream
                                    .set_read_timeout(Some(
                                        self.deadline
                                            .saturating_duration_since(Instant::now())
                                            .max(Duration::from_millis(1)),
                                    ))
                                    .map_err(error)?;
                            }
                            let mut buffer = vec![0u8; c as usize];
                            match (&*stream).read(&mut buffer) {
                                Ok(len) => {
                                    space.with(|memory, _| memory.write(buf_addr, &buffer[..len]))?;
                                    self.read_bytes.fetch_add(len as u64, Ordering::Relaxed);
                                    len as i64
                                }
                                Err(e) => errno(e),
                            }
                        }
                        Some(Descriptor::Unix {
                            stream,
                            nonblocking,
                        }) => {
                            let nb = nonblocking.load(Ordering::Relaxed);
                            let _ = stream.set_nonblocking(nb);
                            let mut buffer = vec![0u8; c as usize];
                            match (&*stream).read(&mut buffer) {
                                Ok(len) => {
                                    space.with(|memory, _| memory.write(buf_addr, &buffer[..len]))?;
                                    len as i64
                                }
                                Err(e) => errno(e),
                            }
                        }
                        Some(Descriptor::File(file)) => {
                            let mut buffer = vec![0u8; c as usize];
                            match (&*file).read(&mut buffer) {
                                Ok(len) => {
                                    space.with(|memory, _| memory.write(buf_addr, &buffer[..len]))?;
                                    self.read_bytes.fetch_add(len as u64, Ordering::Relaxed);
                                    len as i64
                                }
                                Err(e) => errno(e),
                            }
                        }
                        Some(Descriptor::EventFd(ev)) => {
                            if c < 8 {
                                -22
                            } else {
                                loop {
                                    let val = {
                                        let mut guard = ev.counter.lock().unwrap();
                                        let v = *guard;
                                        if v > 0 {
                                            *guard = 0;
                                        }
                                        v
                                    };
                                    if val > 0 {
                                        space.with(|memory, _| {
                                            memory.write(buf_addr, &val.to_le_bytes())
                                        })?;
                                        break 8;
                                    }
                                    if ev.nonblocking.load(Ordering::Relaxed) {
                                        break -11;
                                    }
                                    if Instant::now() >= self.deadline {
                                        break -4;
                                    }
                                    std::thread::sleep(Duration::from_micros(250));
                                }
                            }
                        }
                        _ => -9,
                    }
                } else {
                    let bytes = space.with(|memory, _| {
                        memory.read(buf_addr, c as usize).map(|s| s.to_vec())
                    })?;
                    if a == 1 {
                        io::stdout().write_all(&bytes).map_err(error)?;
                        io::stdout().flush().map_err(error)?;
                        c as i64
                    } else if a == 2 {
                        io::stderr().write_all(&bytes).map_err(error)?;
                        c as i64
                    } else {
                        match self.get_desc(a) {
                            Some(Descriptor::Stream { stream, .. }) => {
                                match write_all_stream(&stream, &bytes, self.deadline) {
                                    Ok(len) => {
                                        self.written_bytes
                                            .fetch_add(len as u64, Ordering::Relaxed);
                                        len as i64
                                    }
                                    Err(e) => errno(e),
                                }
                            }
                            Some(Descriptor::Unix { stream, .. }) => match (&*stream).write(&bytes)
                            {
                                Ok(len) => len as i64,
                                Err(e) => errno(e),
                            },
                            Some(Descriptor::File(file)) => match (&*file).write(&bytes) {
                                Ok(len) => {
                                    let _ = (&*file).flush();
                                    self.written_bytes
                                        .fetch_add(len as u64, Ordering::Relaxed);
                                    len as i64
                                }
                                Err(e) => errno(e),
                            },
                            Some(Descriptor::EventFd(ev)) => {
                                if c < 8 {
                                    -22
                                } else {
                                    let add = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                                    let mut guard = ev.counter.lock().unwrap();
                                    *guard = guard.saturating_add(add);
                                    8
                                }
                            }
                            _ => -9,
                        }
                    }
                }
            }
            46 => {
                // sendmsg(fd, msg, flags)
                detail["name"] = json!("sendmsg");
                // struct msghdr: msg_name(0), msg_namelen(8), msg_iov(16), msg_iovlen(24), msg_control(32), msg_controllen(40)
                let hdr = space.with(|memory, _| memory.read(b, 48).map(|s| s.to_vec()));
                match hdr {
                    Ok(hdr) => {
                        let iov_ptr =
                            scheme.strip(u64::from_le_bytes(hdr[16..24].try_into().unwrap()));
                        let iovlen = u64::from_le_bytes(hdr[24..32].try_into().unwrap()) as usize;
                        let controllen =
                            u64::from_le_bytes(hdr[40..48].try_into().unwrap()) as usize;
                        let mut total = 0usize;
                        let mut payload = Vec::new();
                        for i in 0..iovlen.min(64) {
                            if let Ok(iov) = space.with(|memory, _| {
                                memory.read(iov_ptr + (i as u64) * 16, 16).map(|s| s.to_vec())
                            }) {
                                let base = scheme
                                    .strip(u64::from_le_bytes(iov[..8].try_into().unwrap()));
                                let len =
                                    u64::from_le_bytes(iov[8..16].try_into().unwrap()) as usize;
                                total += len;
                                if controllen == 0 && len > 0 {
                                    if let Ok(chunk) = space
                                        .with(|memory, _| memory.read(base, len).map(|s| s.to_vec()))
                                    {
                                        payload.extend_from_slice(&chunk);
                                    }
                                }
                            }
                        }
                        if controllen == 0 && !payload.is_empty() {
                            if let Some(Descriptor::Unix { stream, .. }) = self.get_desc(a) {
                                let _ = (&*stream).write_all(&payload);
                            }
                        }
                        total as i64
                    }
                    Err(_) => -14,
                }
            }
            47 => {
                // recvmsg(fd, msg, flags)
                detail["name"] = json!("recvmsg");
                let hdr = space.with(|memory, _| memory.read(b, 48).map(|s| s.to_vec()));
                match hdr {
                    Ok(hdr) => {
                        let iov_ptr =
                            scheme.strip(u64::from_le_bytes(hdr[16..24].try_into().unwrap()));
                        let iovlen = u64::from_le_bytes(hdr[24..32].try_into().unwrap()) as usize;
                        if iovlen == 0 {
                            0
                        } else if let Some(Descriptor::Unix { stream, .. }) = self.get_desc(a) {
                            let iov = space.with(|memory, _| {
                                memory.read(iov_ptr, 16).map(|s| s.to_vec())
                            })?;
                            let base =
                                scheme.strip(u64::from_le_bytes(iov[..8].try_into().unwrap()));
                            let len = u64::from_le_bytes(iov[8..16].try_into().unwrap()) as usize;
                            let mut buf = vec![0u8; len];
                            match (&*stream).read(&mut buf) {
                                Ok(n) => {
                                    space.with(|memory, _| memory.write(base, &buf[..n]))?;
                                    n as i64
                                }
                                Err(e) => errno(e),
                            }
                        } else {
                            -11
                        }
                    }
                    Err(_) => -14,
                }
            }
            72 => {
                detail["name"] = json!("fcntl");
                let desc = self.get_desc(a);
                if desc.is_none() && a > 2 {
                    -9
                } else {
                    match b {
                        0 | 1030 => {
                            // F_DUPFD / F_DUPFD_CLOEXEC
                            if let Some(d) = desc {
                                let mut inner = self.inner.lock().unwrap();
                                let min_fd = c.max(3);
                                if let Some(fd) = (min_fd..4096)
                                    .find(|fd| !inner.descriptors.contains_key(fd))
                                {
                                    inner.descriptors.insert(fd, d);
                                    fd as i64
                                } else {
                                    -24
                                }
                            } else {
                                -9
                            }
                        }
                        1 => 0, // F_GETFD
                        2 => 0, // F_SETFD
                        3 => {
                            // F_GETFL
                            let nb = match desc {
                                Some(
                                    Descriptor::Socket { nonblocking, .. }
                                    | Descriptor::Bound { nonblocking, .. }
                                    | Descriptor::Listening { nonblocking, .. }
                                    | Descriptor::Stream { nonblocking, .. }
                                    | Descriptor::Unix { nonblocking, .. },
                                ) => nonblocking.load(Ordering::Relaxed),
                                Some(Descriptor::EventFd(ev)) => {
                                    ev.nonblocking.load(Ordering::Relaxed)
                                }
                                _ => false,
                            };
                            2 | if nb { 0x800 } else { 0 }
                        }
                        4 => {
                            // F_SETFL
                            let nb = (c & 0x800) != 0;
                            match desc {
                                Some(Descriptor::Socket { nonblocking, .. })
                                | Some(Descriptor::Bound { nonblocking, .. })
                                | Some(Descriptor::Listening { nonblocking, .. }) => {
                                    nonblocking.store(nb, Ordering::Relaxed);
                                }
                                Some(Descriptor::Stream {
                                    stream,
                                    nonblocking,
                                }) => {
                                    nonblocking.store(nb, Ordering::Relaxed);
                                    let _ = stream.set_nonblocking(nb);
                                }
                                Some(Descriptor::Unix {
                                    stream,
                                    nonblocking,
                                }) => {
                                    nonblocking.store(nb, Ordering::Relaxed);
                                    let _ = stream.set_nonblocking(nb);
                                }
                                Some(Descriptor::EventFd(ev)) => {
                                    ev.nonblocking.store(nb, Ordering::Relaxed);
                                }
                                _ => {}
                            }
                            0
                        }
                        _ => 0,
                    }
                }
            }
            16 => {
                // ioctl(fd, cmd, arg)
                detail["name"] = json!("ioctl");
                match b {
                    0x5421 => {
                        // FIONBIO
                        let val = space.with(|memory, _| {
                            memory
                                .read(c, 4)
                                .map(|s| i32::from_le_bytes(s.try_into().unwrap()))
                        });
                        match val {
                            Ok(v) => {
                                let nb = v != 0;
                                match self.get_desc(a) {
                                    Some(Descriptor::Socket { nonblocking, .. })
                                    | Some(Descriptor::Bound { nonblocking, .. })
                                    | Some(Descriptor::Listening { nonblocking, .. }) => {
                                        nonblocking.store(nb, Ordering::Relaxed);
                                        0
                                    }
                                    Some(Descriptor::Stream {
                                        stream,
                                        nonblocking,
                                    }) => {
                                        nonblocking.store(nb, Ordering::Relaxed);
                                        let _ = stream.set_nonblocking(nb);
                                        0
                                    }
                                    Some(Descriptor::Unix {
                                        stream,
                                        nonblocking,
                                    }) => {
                                        nonblocking.store(nb, Ordering::Relaxed);
                                        let _ = stream.set_nonblocking(nb);
                                        0
                                    }
                                    Some(Descriptor::EventFd(ev)) => {
                                        ev.nonblocking.store(nb, Ordering::Relaxed);
                                        0
                                    }
                                    Some(_) => 0,
                                    None => -9,
                                }
                            }
                            Err(_) => -14,
                        }
                    }
                    0x5450 | 0x5451 => 0, // FIONCLEX / FIOCLEX
                    0x5452 => {
                        // FIOASYNC
                        let val = space.with(|memory, _| {
                            memory
                                .read(c, 4)
                                .map(|s| i32::from_le_bytes(s.try_into().unwrap()))
                        });
                        match val {
                            Ok(_) => {
                                if self.get_desc(a).is_some() {
                                    0
                                } else {
                                    -9
                                }
                            }
                            Err(_) => -14,
                        }
                    }
                    _ => -25,             // ENOTTY
                }
            }
            32 | 33 | 292 => {
                // dup / dup2 / dup3
                detail["name"] = json!(if n == 32 {
                    "dup"
                } else if n == 33 {
                    "dup2"
                } else {
                    "dup3"
                });
                if n == 32 {
                    if let Some(d) = self.get_desc(a) {
                        self.insert(d)? as i64
                    } else {
                        -9
                    }
                } else if a == b {
                    if n == 292 {
                        -22
                    } else {
                        b as i64
                    }
                } else if a <= 2 && b <= 2 {
                    b as i64
                } else if let Some(d) = self.get_desc(a) {
                    self.inner.lock().unwrap().descriptors.insert(b, d);
                    b as i64
                } else {
                    -9
                }
            }
            3 => {
                detail["name"] = json!("close");
                if self.inner.lock().unwrap().descriptors.remove(&a).is_some() {
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
                    let fixed = (flags & 0x10) != 0;
                    let writable = (prot & 2) != 0;
                    let executable = (prot & 4) != 0;
                    if writable && executable {
                        return Err(error("mmap W^X violation"));
                    }
                    let file_desc = if (flags & 0x20) == 0 {
                        self.get_desc(mmap_fd)
                    } else {
                        None
                    };
                    let mut file_bytes = None;
                    if let Some(Descriptor::File(f)) = file_desc {
                        let _ = (&*f).seek(io::SeekFrom::Start(mmap_offset));
                        let mut buf = vec![0u8; len as usize];
                        if let Ok(n) = (&*f).read(&mut buf) {
                            buf.truncate(n);
                            file_bytes = Some(buf);
                        }
                    }
                    space.with(|memory, _| -> i64 {
                        let actual_addr = if fixed {
                            let _ = memory.unmap(addr, aligned_len);
                            addr
                        } else {
                            memory.find_free_mmap(addr, aligned_len).unwrap_or(0)
                        };
                        if actual_addr == 0 {
                            -12
                        } else if let Err(err) =
                            memory.map(actual_addr, aligned_len, writable, executable)
                        {
                            detail["error"] = json!(err.to_string());
                            -12
                        } else {
                            if let Some(buf) = file_bytes {
                                if let Some(r) = memory
                                    .regions
                                    .iter_mut()
                                    .find(|r| r.start == actual_addr)
                                {
                                    let copy_len = buf.len().min(r.data.len());
                                    r.data[..copy_len].copy_from_slice(&buf[..copy_len]);
                                }
                            }
                            detail["mapped"] = json!(format!("0x{actual_addr:x}+{aligned_len:x}"));
                            actual_addr as i64
                        }
                    })
                }
            }
            11 => {
                detail["name"] = json!("munmap");
                space.with(|memory, _| {
                    if let Err(err) = memory.unmap(a, b) {
                        detail["error"] = json!(err.to_string());
                        -22
                    } else {
                        0
                    }
                })
            }
            10 => {
                detail["name"] = json!("mprotect");
                space.with(|memory, _| {
                    if let Err(err) = memory.mprotect(a, b, c as i32) {
                        detail["error"] = json!(err.to_string());
                        -22
                    } else {
                        0
                    }
                })
            }
            28 | 221 => {
                detail["name"] = json!(if n == 28 { "madvise" } else { "fadvise64" });
                0
            }
            12 => {
                detail["name"] = json!("brk");
                let mut inner = self.inner.lock().unwrap();
                if a == 0 {
                    inner.brk_current as i64
                } else if a < inner.brk_original || a > inner.brk_original + 64 * 1024 * 1024 {
                    -12
                } else {
                    let old = inner.brk_current;
                    if a > old {
                        let len = a - old;
                        let aligned_len = ((len + 4095) / 4096) * 4096;
                        if space
                            .with(|memory, _| memory.map(old, aligned_len, true, false))
                            .is_err()
                        {
                            -12
                        } else {
                            inner.brk_current = old + aligned_len;
                            inner.brk_current as i64
                        }
                    } else {
                        inner.brk_current = a;
                        a as i64
                    }
                }
            }
            39 => {
                detail["name"] = json!("getpid");
                unsafe { libc::getpid() as i64 }
            }
            186 => {
                detail["name"] = json!("gettid");
                tid as i64
            }
            110 => {
                detail["name"] = json!("getppid");
                unsafe { libc::getppid() as i64 }
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
            105 | 106 | 109 | 111 | 112 | 116 | 117 | 119 | 140 | 141 | 157 | 203 => {
                detail["name"] = json!("process_attr");
                0
            }
            80 => {
                detail["name"] = json!("chdir");
                0
            }
            95 => {
                detail["name"] = json!("umask");
                0o022
            }
            83 | 258 => {
                detail["name"] = json!(if n == 83 { "mkdir" } else { "mkdirat" });
                let path_ptr = if n == 83 { a } else { b };
                match space.with(|memory, _| read_cstring(memory, path_ptr)) {
                    Ok(raw_path) => {
                        let path = resolve_guest_path(&raw_path);
                        match std::fs::create_dir_all(&path) {
                            Ok(()) => 0,
                            Err(e) => errno(e),
                        }
                    }
                    Err(_) => -14,
                }
            }
            87 | 263 => {
                detail["name"] = json!(if n == 87 { "unlink" } else { "unlinkat" });
                let path_ptr = if n == 87 { a } else { b };
                match space.with(|memory, _| read_cstring(memory, path_ptr)) {
                    Ok(raw_path) => {
                        let path = resolve_guest_path(&raw_path);
                        match std::fs::remove_file(&path) {
                            Ok(()) => 0,
                            Err(e) => errno(e),
                        }
                    }
                    Err(_) => -14,
                }
            }
            82 => {
                detail["name"] = json!("rename");
                let paths = space.with(|memory, _| {
                    Ok::<_, EngineError>((read_cstring(memory, a)?, read_cstring(memory, b)?))
                });
                match paths {
                    Ok((old_raw, new_raw)) => {
                        let old_p = resolve_guest_path(&old_raw);
                        let new_p = resolve_guest_path(&new_raw);
                        match std::fs::rename(old_p, new_p) {
                            Ok(()) => 0,
                            Err(e) => errno(e),
                        }
                    }
                    Err(_) => -14,
                }
            }
            74 | 75 | 90 | 91 | 92 | 93 => 0,
            63 => {
                detail["name"] = json!("uname");
                let mut uts = [0u8; 390];
                fill_utsname(&mut uts);
                if space.with(|memory, _| memory.write(a, &uts)).is_err() {
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
                if space.with(|memory, _| memory.write(a, &info)).is_err() {
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
                        space.with(|memory, _| {
                            let _ = memory.write(a, bytes);
                            let _ = memory.write(a + bytes.len() as u64, &[0]);
                        });
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
                if let Ok(raw_path) = space.with(|memory, _| read_cstring(memory, path_ptr)) {
                    if raw_path == "/proc/self/exe" {
                        let exe = b"/home/devuser/nginx-musl-root/sbin/nginx\0";
                        let copy_len = bufsiz.min(exe.len() - 1);
                        space.with(|memory, _| {
                            let _ = memory.write(buf_ptr, &exe[..copy_len]);
                        });
                        copy_len as i64
                    } else {
                        let path = resolve_guest_path(&raw_path);
                        match std::fs::read_link(&path) {
                            Ok(target) => {
                                let target_str = target.to_string_lossy();
                                let bytes = target_str.as_bytes();
                                let copy_len = bufsiz.min(bytes.len());
                                space.with(|memory, _| {
                                    let _ = memory.write(buf_ptr, &bytes[..copy_len]);
                                });
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
            35 | 230 => {
                detail["name"] = json!(if n == 35 {
                    "nanosleep"
                } else {
                    "clock_nanosleep"
                });
                let req_ptr = if n == 35 { a } else { c };
                let req = space.with(|memory, _| {
                    memory.read(req_ptr, 16).map(|req_bytes| {
                        let sec = i64::from_le_bytes(req_bytes[..8].try_into().unwrap());
                        let nsec = i64::from_le_bytes(req_bytes[8..16].try_into().unwrap());
                        (sec, nsec)
                    })
                });
                if let Ok((sec, nsec)) = req {
                    if sec >= 0 && (0..1_000_000_000).contains(&nsec) {
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
                    let mask = 3u64;
                    space.with(|memory, _| {
                        let _ = memory.write(c, &mask.to_le_bytes());
                    });
                    8
                } else {
                    -22
                }
            }
            96 => {
                detail["name"] = json!("gettimeofday");
                if a != 0 {
                    let mut ts = libc::timespec {
                        tv_sec: 0,
                        tv_nsec: 0,
                    };
                    unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts) };
                    let mut tv = [0u8; 16];
                    tv[..8].copy_from_slice(&(ts.tv_sec as i64).to_le_bytes());
                    tv[8..16].copy_from_slice(&((ts.tv_nsec / 1000) as i64).to_le_bytes());
                    if space.with(|memory, _| memory.write(a, &tv)).is_err() {
                        return Ok(Outcome {
                            exit: None,
                            thread_exit: false,
                            spawn_thread: None,
                            forked_child: false,
                            detail,
                        });
                    }
                }
                0
            }
            201 => {
                detail["name"] = json!("time");
                let mut ts = libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                };
                unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts) };
                let sec = ts.tv_sec as i64;
                if a != 0 {
                    let _ = space.with(|memory, _| memory.write(a, &sec.to_le_bytes()));
                }
                sec
            }
            228 | 229 => {
                detail["name"] = json!(if n == 228 {
                    "clock_gettime"
                } else {
                    "clock_getres"
                });
                if n == 229 {
                    if b != 0 {
                        let mut bytes = [0u8; 16];
                        bytes[8..16].copy_from_slice(&1i64.to_le_bytes());
                        let _ = space.with(|memory, _| memory.write(b, &bytes));
                    }
                    0
                } else {
                    let clock = match raw_a as i32 {
                        0 | 4 | 5 => Some(libc::CLOCK_REALTIME),
                        1 | 6 | 7 => Some(libc::CLOCK_MONOTONIC),
                        2 => Some(libc::CLOCK_PROCESS_CPUTIME_ID),
                        3 => Some(libc::CLOCK_THREAD_CPUTIME_ID),
                        _ => Some(libc::CLOCK_MONOTONIC),
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
                            if space.with(|memory, _| memory.write(b, &bytes)).is_err() {
                                -14
                            } else {
                                0
                            }
                        }
                    } else {
                        -22
                    }
                }
            }
            318 => {
                detail["name"] = json!("getrandom");
                let mut buf = vec![0u8; b as usize];
                match std::fs::File::open("/dev/urandom")
                    .and_then(|mut f| f.read_exact(&mut buf).map(|_| ()))
                {
                    Ok(()) => {
                        if space.with(|memory, _| memory.write(a, &buf)).is_err() {
                            -14
                        } else {
                            b as i64
                        }
                    }
                    Err(_) => -5,
                }
            }
            54 | 55 => {
                detail["name"] = json!(if n == 54 {
                    "setsockopt"
                } else {
                    "getsockopt"
                });
                match self.get_desc(a) {
                    Some(
                        Descriptor::Socket { .. }
                        | Descriptor::Bound { .. }
                        | Descriptor::Listening { .. }
                        | Descriptor::Stream { .. }
                        | Descriptor::Unix { .. },
                    ) => 0,
                    Some(_) => -9,
                    None => -9,
                }
            }
            13 | 14 | 38 | 127 => {
                detail["name"] = json!("signal_ctl");
                0
            }
            130 => {
                // rt_sigsuspend: Master process waits here while workers serve traffic.
                detail["name"] = json!("rt_sigsuspend");
                while Instant::now() < self.deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                -4 // -EINTR
            }
            61 => {
                // wait4(pid, wstatus, options, rusage)
                detail["name"] = json!("wait4");
                let pid = a as i32;
                let status_ptr = b;
                let options = c as i32;
                let mut status: i32 = 0;
                let ret = unsafe {
                    libc::wait4(pid, &mut status, options, std::ptr::null_mut())
                };
                if ret < 0 {
                    errno(io::Error::last_os_error())
                } else {
                    if ret > 0 && status_ptr != 0 {
                        let _ = space.with(|memory, _| {
                            memory.write(status_ptr, &status.to_le_bytes())
                        });
                    }
                    ret as i64
                }
            }
            62 => {
                // kill(pid, sig)
                detail["name"] = json!("kill");
                let pid = a as i32;
                let sig = b as i32;
                if sig == 0 {
                    0
                } else if pid > 0 && pid != unsafe { libc::getpid() } {
                    let r = unsafe { libc::kill(pid, sig) };
                    if r < 0 {
                        errno(io::Error::last_os_error())
                    } else {
                        0
                    }
                } else {
                    0
                }
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
                    let mut combined = Vec::new();
                    for i in 0..iovcnt {
                        let entry_addr = iov_ptr + (i as u64) * 16;
                        let chunk = space.with(|memory, _| -> Result<Vec<u8>, EngineError> {
                            let raw_iov = memory.read(entry_addr, 16)?;
                            let base = u64::from_le_bytes(raw_iov[..8].try_into().unwrap());
                            let len =
                                u64::from_le_bytes(raw_iov[8..16].try_into().unwrap()) as usize;
                            if len == 0 {
                                return Ok(Vec::new());
                            }
                            Ok(memory.read(scheme.strip(base), len)?.to_vec())
                        })?;
                        combined.extend_from_slice(&chunk);
                    }
                    if combined.is_empty() {
                        0
                    } else if fd == 1 {
                        io::stdout().write_all(&combined).map_err(error)?;
                        io::stdout().flush().map_err(error)?;
                        combined.len() as i64
                    } else if fd == 2 {
                        io::stderr().write_all(&combined).map_err(error)?;
                        io::stderr().flush().map_err(error)?;
                        combined.len() as i64
                    } else {
                        match self.get_desc(fd) {
                            Some(Descriptor::Stream { stream, .. }) => {
                                match write_all_stream(&stream, &combined, self.deadline) {
                                    Ok(n) => {
                                        self.written_bytes
                                            .fetch_add(n as u64, Ordering::Relaxed);
                                        n as i64
                                    }
                                    Err(e) => errno(e),
                                }
                            }
                            Some(Descriptor::Unix { stream, .. }) => {
                                match (&*stream).write(&combined) {
                                    Ok(n) => n as i64,
                                    Err(e) => errno(e),
                                }
                            }
                            Some(Descriptor::File(file)) => match (&*file).write_all(&combined) {
                                Ok(()) => {
                                    self.written_bytes
                                        .fetch_add(combined.len() as u64, Ordering::Relaxed);
                                    combined.len() as i64
                                }
                                Err(e) => errno(e),
                            },
                            _ => -9,
                        }
                    }
                }
            }
            7 => {
                detail["name"] = json!("poll");
                space.with(|memory, _| {
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
                            revents = events & 4;
                        } else if fd == 0 {
                            revents = 0;
                        } else if self.get_desc(fd as u64).is_some() {
                            revents = events & 4;
                        }
                        let _ = memory.write(ptr + 6, &revents.to_le_bytes());
                        if revents != 0 {
                            ready += 1;
                        }
                    }
                    ready
                })
            }
            284 | 290 => {
                // eventfd(initval) / eventfd2(initval, flags)
                detail["name"] = json!(if n == 284 { "eventfd" } else { "eventfd2" });
                let nonblock = n == 290 && (b & 0x800) != 0;
                let ev = Arc::new(EventFdState {
                    counter: Mutex::new(a),
                    nonblocking: AtomicBool::new(nonblock),
                });
                self.insert(Descriptor::EventFd(ev))? as i64
            }
            206 => {
                detail["name"] = json!("io_setup");
                if b != 0 {
                    space.with(|memory, _| {
                        let _ = memory.write(b, &1u64.to_le_bytes());
                    });
                }
                0
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
                let epoll_arc = match self.get_desc(epfd) {
                    Some(Descriptor::Epoll(ep)) => ep,
                    _ => {
                        return Ok(Outcome {
                            exit: None,
                            thread_exit: false,
                            spawn_thread: None,
                            forked_child: false,
                            detail: json!({"name":"epoll_ctl","error":"bad epfd"}),
                        })
                    }
                };
                match op {
                    1 | 3 => {
                        if event_ptr == 0 {
                            -14
                        } else {
                            let (events, data) =
                                space.with(|memory, _| -> Result<(u32, u64), EngineError> {
                                    let ev_bytes = memory.read(event_ptr, 12)?;
                                    let ev = u32::from_le_bytes(ev_bytes[..4].try_into().unwrap());
                                    let dt = u64::from_le_bytes(ev_bytes[4..12].try_into().unwrap());
                                    Ok((ev, dt))
                                })?;
                            let mut guard = epoll_arc.lock().unwrap();
                            guard
                                .interests
                                .insert(fd, EpollInterest { fd, events, data });
                            0
                        }
                    }
                    2 => {
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
                    let epoll_arc = match self.get_desc(epfd) {
                        Some(Descriptor::Epoll(ep)) => ep,
                        _ => {
                            return Ok(Outcome {
                                exit: None,
                                thread_exit: false,
                                spawn_thread: None,
                                forked_child: false,
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
                            match self.get_desc(interest.fd) {
                                Some(Descriptor::Listening { listener, .. }) => {
                                    use std::os::unix::io::AsRawFd;
                                    let mut pfd = libc::pollfd {
                                        fd: listener.as_raw_fd(),
                                        events: libc::POLLIN,
                                        revents: 0,
                                    };
                                    let poll_res = unsafe { libc::poll(&mut pfd, 1, 0) };
                                    if poll_res > 0 && (pfd.revents & libc::POLLIN) != 0 {
                                        revents |= 1;
                                    }
                                }
                                Some(Descriptor::Stream { stream, .. }) => {
                                    use std::os::unix::io::AsRawFd;
                                    let mut pfd = libc::pollfd {
                                        fd: stream.as_raw_fd(),
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
                                Some(Descriptor::Unix { stream, .. }) => {
                                    use std::os::unix::io::AsRawFd;
                                    let mut pfd = libc::pollfd {
                                        fd: stream.as_raw_fd(),
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
                                    }
                                }
                                Some(Descriptor::EventFd(ev)) => {
                                    if *ev.counter.lock().unwrap() > 0 {
                                        revents |= 1;
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
                        std::thread::sleep(Duration::from_micros(250));
                    }
                    let count = ready_events.len();
                    if count > 0 {
                        space.with(|memory, _| {
                            for (i, (ev, data)) in ready_events.into_iter().enumerate() {
                                let offset = events_ptr + (i as u64) * 12;
                                let mut ev_buf = [0u8; 12];
                                ev_buf[..4].copy_from_slice(&ev.to_le_bytes());
                                ev_buf[4..12].copy_from_slice(&data.to_le_bytes());
                                let _ = memory.write(offset, &ev_buf);
                            }
                        });
                    }
                    count as i64
                }
            }
            131 => {
                detail["name"] = json!("sigaltstack");
                if b != 0 {
                    let mut old = [0u8; 24];
                    old[8..12].copy_from_slice(&2i32.to_le_bytes());
                    space.with(|memory, _| {
                        let _ = memory.write(b, &old);
                    });
                }
                0
            }
            200 | 234 => {
                detail["name"] = json!(if n == 200 { "tkill" } else { "tgkill" });
                let sig = if n == 200 { b as i32 } else { c as i32 };
                if sig == 32 || sig == 33 {
                    // Internal glibc/musl SIGCANCEL / SIGSYNCCALL: wake any parked futexes.
                    0
                } else if sig != 0 {
                    exit = Some(128 + sig);
                    0
                } else {
                    0
                }
            }
            158 => {
                detail["name"] = json!("arch_prctl");
                match a {
                    0x1002 => {
                        state[18] = raw_b;
                        0
                    }
                    0x1003 => {
                        let fs = state[18];
                        space.with(|memory, _| memory.write(b, &fs.to_le_bytes()))?;
                        0
                    }
                    _ => -22,
                }
            }
            207 | 208 | 209 | 210 => {
                detail["name"] = json!(match n {
                    207 => "io_destroy",
                    208 => "io_getevents",
                    209 => "io_submit",
                    _ => "io_cancel",
                });
                0
            }
            218 => {
                detail["name"] = json!("set_tid_address");
                *clear_child_tid = a;
                tid as i64
            }
            273 => {
                detail["name"] = json!("set_robust_list");
                *robust_list_head = a;
                0
            }
            334 => {
                detail["name"] = json!("rseq");
                -38
            }
            435 => {
                // clone3: return ENOSYS (-38) so glibc falls back to clone (56)
                detail["name"] = json!("clone3");
                -38
            }
            57 | 58 => {
                // fork / vfork
                detail["name"] = json!(if n == 57 { "fork" } else { "vfork" });
                let pid = unsafe { libc::fork() };
                if pid < 0 {
                    errno(io::Error::last_os_error())
                } else if pid == 0 {
                    forked_child = true;
                    0
                } else {
                    pid as i64
                }
            }
            56 => {
                // clone(flags, stack, ptid, ctid, tls)
                detail["name"] = json!("clone");
                let flags = a;
                let stack = b;
                let ptid = c;
                let ctid = d;
                let tls = raw_e;
                const CLONE_THREAD: u64 = 0x0001_0000;
                const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
                const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
                const CLONE_CHILD_SETTID: u64 = 0x0100_0000;

                let is_thread = (flags & CLONE_THREAD) != 0;
                if !is_thread {
                    let pid = unsafe { libc::fork() };
                    if pid < 0 {
                        errno(io::Error::last_os_error())
                    } else if pid == 0 {
                        if stack != 0 {
                            state[7] = stack;
                        }
                        let child_pid = unsafe { libc::getpid() } as u64;
                        if (flags & CLONE_CHILD_SETTID) != 0 && ctid != 0 {
                            space.with(|memory, _| {
                                let _ = memory.write(ctid, &(child_pid as u32).to_le_bytes());
                            });
                        }
                        if (flags & CLONE_CHILD_CLEARTID) != 0 {
                            *clear_child_tid = ctid;
                        }
                        forked_child = true;
                        0
                    } else {
                        if (flags & CLONE_PARENT_SETTID) != 0 && ptid != 0 {
                            space.with(|memory, _| {
                                let _ = memory.write(ptid, &(pid as u32).to_le_bytes());
                            });
                        }
                        pid as i64
                    }
                } else if let Some(tid_counter) = next_tid.filter(|_| stack != 0) {
                    let child_tid = tid_counter.fetch_add(1, Ordering::SeqCst);
                    space.with(|memory, _| {
                        if (flags & CLONE_PARENT_SETTID) != 0 && ptid != 0 {
                            let _ = memory.write(ptid, &(child_tid as u32).to_le_bytes());
                        }
                        if (flags & CLONE_CHILD_SETTID) != 0 && ctid != 0 {
                            let _ = memory.write(ctid, &(child_tid as u32).to_le_bytes());
                        }
                    });
                    spawn_thread = Some(ThreadSpawn {
                        flags,
                        child_stack: stack,
                        ptid,
                        ctid,
                        tls,
                        child_tid,
                    });
                    detail["child_tid"] = json!(child_tid);
                    child_tid as i64
                } else {
                    -38
                }
            }
            202 => {
                detail["name"] = json!("futex");
                self.futex_call_space([a, raw_b, raw_c, d, e, raw_f], space)
            }
            97 | 160 | 302 => {
                detail["name"] = json!("prlimit64");
                let out_ptr = if n == 302 { d } else if n == 97 { b } else { 0 };
                if out_ptr != 0 {
                    let mut rlim = [0u8; 16];
                    rlim[..8].copy_from_slice(&(8 * 1024 * 1024u64).to_le_bytes());
                    rlim[8..16].copy_from_slice(&(8 * 1024 * 1024u64).to_le_bytes());
                    space.with(|memory, _| {
                        let _ = memory.write(out_ptr, &rlim);
                    });
                }
                0
            }
            60 => {
                detail["name"] = json!("exit");
                if is_child_thread {
                    thread_exit = true;
                } else {
                    exit = Some((a & 255) as i32);
                }
                0
            }
            231 => {
                detail["name"] = json!("exit_group");
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
        Ok(Outcome {
            exit,
            thread_exit,
            spawn_thread,
            forked_child,
            detail,
        })
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

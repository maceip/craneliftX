/* Minimal HTTP server for ISR benchmarking.
 * Linux freestanding version — uses raw x86-64 syscall ABI.
 *
 * This binary is designed to run under the ISR hardened runtime.
 * It uses only syscalls supported by the ISR bridge:
 *   socket(41), bind(49), listen(50), accept(43),
 *   read(0), write(1), close(3), exit(60)
 *
 * Build: zig cc -target x86_64-linux-gnu -static -O2 \
 *           -nostdlib -fno-stack-protector \
 *           -Wl,--entry=_start \
 *           -DREQUEST_COUNT=100 -DPORT=29876 \
 *           -o bench_server_linux.elf bench_server_linux.c
 */

#ifndef REQUEST_COUNT
#define REQUEST_COUNT 100
#endif
#ifndef PORT
#define PORT 8765
#endif

static inline long sys6(long n, long a, long b, long c, long d, long e, long f) {
    long r;
    __asm__ volatile (
        "movq %5, %%r10\n"
        "movq %6, %%r8\n"
        "movq %7, %%r9\n"
        "syscall"
        : "=a"(r)
        : "a"(n), "D"(a), "S"(b), "d"(c), "r"(d), "r"(e), "r"(f)
        : "rcx", "r11", "r10", "r8", "r9", "memory"
    );
    return r;
}
static inline long sys3(long n, long a, long b, long c) {
    return sys6(n, a, b, c, 0, 0, 0);
}

static const char response[] =
    "HTTP/1.1 200 OK\r\n"
    "Content-Type: text/plain\r\n"
    "Content-Length: 13\r\n"
    "\r\n"
    "Hello, World!";

void _start(void) {
    long sock = sys3(41, 2, 1, 0); // socket(AF_INET, SOCK_STREAM, 0)
    if (sock < 0) {
        sys3(60, 1, 0, 0);
    }

    // struct sockaddr_in { sa_family_t sin_family; in_port_t sin_port; struct in_addr sin_addr; char pad[8]; }
    // Linux sockaddr_in is 16 bytes: family=2, port=htons(PORT), addr=127.0.0.1
    unsigned char addr[16] = {
        0x02, 0x00,                     // sin_family = AF_INET
        ((PORT) >> 8) & 0xFF, (PORT) & 0xFF,  // sin_port = htons(PORT)
        0x7f, 0x00, 0x00, 0x01,         // sin_addr = 127.0.0.1
        0, 0, 0, 0, 0, 0, 0, 0          // padding
    };

    long r = sys3(49, sock, (long)addr, 16); // bind
    if (r < 0) {
        sys3(60, 2, 0, 0);
    }

    r = sys3(50, sock, 128, 0); // listen
    if (r < 0) {
        sys3(60, 3, 0, 0);
    }

    char req[1024];
    for (int i = 0; i < REQUEST_COUNT; i++) {
        long conn = sys3(43, sock, 0, 0); // accept
        if (conn < 0) continue;
        sys3(0, conn, (long)req, sizeof(req)); // read
        sys3(1, conn, (long)response, sizeof(response) - 1); // write
        sys3(3, conn, 0, 0); // close
    }

    sys3(3, sock, 0, 0); // close
    sys3(60, 0, 0, 0);   // exit(0)
}

/* Minimal HTTP server for ISR benchmarking.
 * Native macOS version — uses standard libc syscalls.
 *
 * Accepts connections on 127.0.0.1:PORT, responds with a fixed HTTP body,
 * and exits after serving REQUEST_COUNT requests.
 *
 * Build: cc -O2 -o bench_server_native bench_server_native.c
 */
#include <sys/socket.h>
#include <netinet/in.h>
#include <unistd.h>
#include <string.h>

#ifndef REQUEST_COUNT
#define REQUEST_COUNT 100
#endif

#ifndef PORT
#define PORT 8765
#endif

static const char response[] =
    "HTTP/1.1 200 OK\r\n"
    "Content-Type: text/plain\r\n"
    "Content-Length: 13\r\n"
    "\r\n"
    "Hello, World!";

int main(void) {
    int sock = socket(AF_INET, SOCK_STREAM, 0);
    if (sock < 0) return 1;

    int reuse = 1;
    setsockopt(sock, SOL_SOCKET, SO_REUSEADDR, &reuse, sizeof(reuse));

    struct sockaddr_in addr = {0};
    addr.sin_family = AF_INET;
    addr.sin_port = __builtin_bswap16(PORT);
    addr.sin_addr.s_addr = __builtin_bswap32(0x7f000001); // 127.0.0.1

    if (bind(sock, (struct sockaddr *)&addr, sizeof(addr)) < 0) return 2;
    if (listen(sock, 128) < 0) return 3;

    char req[1024];
    for (int i = 0; i < REQUEST_COUNT; i++) {
        int conn = accept(sock, NULL, NULL);
        if (conn < 0) continue;
        (void)read(conn, req, sizeof(req));
        (void)write(conn, response, sizeof(response) - 1);
        close(conn);
    }

    close(sock);
    return 0;
}

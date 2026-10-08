/*
 * sample_network.c -- a deliberately small, self-contained workload used to
 * exercise the ingest_tracer performance analyzer.
 *
 * The point is NOT to be a real server; it is to contain, in one binary, the
 * three kinds of code the lift/drop decision cares about:
 *
 *   1. TIGHT loop (crc32_tight)            -> should be KEPT NATIVE
 *      (super-hot inner loop; the lift/drop boundary overhead is not worth it)
 *
 *   2. NETWORK code that is NOT a tight loop (parse_packet, tcp_window_scaled,
 *      ip_id_hash)                         -> should be LIFTED to Pulley
 *      (frequently executed per-packet work, but not a hot inner loop)
 *
 *   3. ORCHESTRATION glue (handle_connection) -> should be KEPT NATIVE
 *      (mostly calls; ideal native dispatch point, nothing to gain by lifting)
 *
 * Functions are marked noinline so each survives as its own symbol for the
 * static analysis. Compiled with -O1 -fno-inline so the bounded header loop
 * in parse_packet stays a real backward branch remill can lift.
 */
#include <stdint.h>
#include <stddef.h>

/* (1) TIGHT loop: classic CRC32 with an inner bit loop. High loop density. */
__attribute__((noinline))
uint32_t crc32_tight(const uint8_t *p, uint32_t n) {
    uint32_t crc = 0xFFFFFFFFu;
    for (uint32_t i = 0; i < n; i++) {
        crc ^= (uint32_t)p[i];
        for (int k = 0; k < 8; k++) {
            crc = (crc >> 1) ^ (0xEDB88320u & (-(int32_t)(crc & 1)));
        }
    }
    return ~crc;
}

/* (2a) NETWORK, NOT tight, pure-compute: scaled window value. */
__attribute__((noinline))
uint32_t tcp_window_scaled(uint32_t seq, uint32_t win) {
    uint64_t v = (uint64_t)seq + (uint64_t)win;
    return (uint32_t)(v & 0xFFFFFFFFu);
}

/* (2b) NETWORK, NOT tight, pure-compute: IP-id hash. */
__attribute__((noinline))
uint32_t ip_id_hash(uint32_t dst, uint32_t src) {
    uint32_t h = (dst ^ src) * 2654435761u;
    return (h >> 16) ^ (h & 0xFFFF);
}

/* (2c) NETWORK, NOT tight, MEMORY-DEPENDENT: packet header parse.
 * Lifted with remill memory semantics (real loads and stores), not a stub. */
__attribute__((noinline))
uint32_t parse_packet(const uint8_t *buf, uint32_t len,
                      uint32_t *out_ver, uint32_t *out_len) {
    if (len < 4) return 0;
    uint32_t ver = (uint32_t)(buf[0] >> 4);
    uint32_t plen = ((uint32_t)buf[2] << 8) | buf[3];
    uint32_t sum = 0;
    for (uint32_t i = 0; i < (len > 16u ? 16u : len); i++) {
        sum += buf[i];           /* bounded, non-tight loop */
    }
    *out_ver = ver;
    *out_len = plen;
    return sum;
}

/* (3) ORCHESTRATION glue: per-connection dispatch. Mostly calls. */
__attribute__((noinline))
uint32_t handle_connection(const uint8_t *pkt, uint32_t len) {
    uint32_t ver, plen;
    uint32_t s = parse_packet(pkt, len, &ver, &plen);
    uint32_t w = tcp_window_scaled(plen, len);
    uint32_t h = ip_id_hash(plen, ver);
    uint32_t c = crc32_tight(pkt, len);   /* tight -> kept native */
    return s + w + h + c;
}

int main(void) {
    static const uint8_t pkt[20] = {
        0x45, 0, 0, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
    };
    volatile uint32_t r = handle_connection(pkt, 20);
    return (int)r;
}

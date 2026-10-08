// A stand-in for the critical ceremony routine (e.g. the CMS signing
// primitive inside OpenSSL, or the token-parsing path in a server).
//
// The point of this file is NOT the source. It is to show two *already
// compiled* artifacts that a lift/drop pipeline would operate on:
//   - sign_x86_64.o : native x86-64 machine code (what an attacker targets)
//   - sign.wasm     : a Wasm module (what Cranelift can directly consume)
// Cranelift has no decoder for the .o; it CAN drop the .wasm.

#include <stdint.h>

// f(a, b) = (a * b) + a  -- matches the CLIF function in the ceremony demo.
uint64_t ceremony_op(uint64_t a, uint64_t b) {
    uint64_t t = a * b;
    return t + a;
}

// A second routine so the artifact is not trivially small.
uint64_t ceremony_verify(uint64_t a, uint64_t b, uint64_t expected) {
    return (ceremony_op(a, b) == expected) ? 1u : 0u;
}

/*
 * sample_basic.c -- minimal ALU/arithmetic leaf functions ("Hello World" level).
 *
 * Demonstrates basic register-to-register lowering:
 *   - clamp_val: bounded value clamping (conditionals, registers)
 *   - abs_diff: absolute difference between two unsigned integers
 *   - main: entry point (kept native)
 */
#include <stdint.h>

__attribute__((noinline))
uint32_t clamp_val(uint32_t val, uint32_t min_v, uint32_t max_v) {
    if (val < min_v) return min_v;
    if (val > max_v) return max_v;
    return val;
}

__attribute__((noinline))
uint32_t abs_diff(uint32_t a, uint32_t b) {
    return (a > b) ? (a - b) : (b - a);
}

int main(void) {
    volatile uint32_t c = clamp_val(15, 10, 20);
    volatile uint32_t d = abs_diff(10, 4);
    return (int)(c + d);
}

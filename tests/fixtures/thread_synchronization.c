// Real libc pthread/TLS/futex paths plus C11 atomic RMW under contention.
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#define THREADS 4
#define ROUNDS 400
static pthread_mutex_t mutex = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t ready = PTHREAD_COND_INITIALIZER;
static pthread_barrier_t start;
static _Atomic unsigned counter;
static _Atomic unsigned cas_counter;
static _Atomic unsigned exchange_lock;
static unsigned protected_sum, exchange_sum, finished;
static _Thread_local unsigned identity;

static void *worker(void *arg) {
    identity = (unsigned)(uintptr_t)arg;
    pthread_barrier_wait(&start);
    for (unsigned i = 0; i < ROUNDS; ++i) {
        atomic_fetch_add_explicit(&counter, 1, memory_order_relaxed);
        unsigned expected = atomic_load_explicit(&cas_counter, memory_order_relaxed);
        while (!atomic_compare_exchange_weak_explicit(&cas_counter, &expected,
                expected + 1, memory_order_acq_rel, memory_order_relaxed)) {}
        while (atomic_exchange_explicit(&exchange_lock, 1, memory_order_acquire)) {}
        exchange_sum += identity;
        atomic_store_explicit(&exchange_lock, 0, memory_order_release);
        unsigned *block = malloc(32 * sizeof(unsigned));
        if (!block) abort();
        for (unsigned j = 0; j < 32; ++j) block[j] = identity + i + j;
        pthread_mutex_lock(&mutex);
        for (unsigned j = 0; j < 32; ++j)
            if (block[j] != identity + i + j) abort();
        protected_sum += identity;
        pthread_mutex_unlock(&mutex);
        free(block);
    }
    pthread_mutex_lock(&mutex);
    ++finished;
    pthread_cond_signal(&ready);
    pthread_mutex_unlock(&mutex);
    return (void *)(uintptr_t)identity;
}

int main(void) {
    pthread_t threads[THREADS];
    if (pthread_barrier_init(&start, 0, THREADS)) return 1;
    for (unsigned i = 0; i < THREADS; ++i)
        if (pthread_create(&threads[i], 0, worker, (void *)(uintptr_t)(i + 1))) return 2;
    pthread_mutex_lock(&mutex);
    while (finished != THREADS) pthread_cond_wait(&ready, &mutex);
    pthread_mutex_unlock(&mutex);
    for (unsigned i = 0; i < THREADS; ++i) {
        void *result = 0;
        if (pthread_join(threads[i], &result) || (uintptr_t)result != i + 1) return 3;
    }
    pthread_barrier_destroy(&start);
    if (counter != THREADS * ROUNDS || cas_counter != THREADS * ROUNDS ||
        protected_sum != 10 * ROUNDS || exchange_sum != 10 * ROUNDS) return 4;
    puts("THREAD_SYNC_OK atomic=1600 cas=1600 mutex=4000 exchange=4000 tls=4 joins=4");
    return 0;
}

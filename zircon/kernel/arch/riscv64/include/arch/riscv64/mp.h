// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_MP_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_MP_H_

#include <zircon/compiler.h>

#include <arch/riscv64.h>
#include <kernel/align.h>
#include <kernel/cpu.h>

__BEGIN_CDECLS

struct percpu;

struct riscv64_percpu {
  // The CPU number is used internally in Zircon
  cpu_num_t cpu_num;

  // The hart id is used by other components (SBI/PLIC etc...)
  uint hart_id;

  // Whether blocking is disallowed.  See arch_blocking_disallowed().
  uint32_t blocking_disallowed;

  // Number of spinlocks currently held.
  uint32_t num_spinlocks;

  // A pointer providing fast access to the high-level arch-agnostic per-cpu struct.
  percpu* high_level_percpu;
} __ALIGNED(MAX_CACHE_LINE);

static inline void riscv64_set_percpu(struct riscv64_percpu *ptr) {
  __asm__ volatile("mv gp, %0" :: "r"(ptr));
}

static inline struct riscv64_percpu* riscv64_read_percpu_ptr() {
  struct riscv64_percpu* p;
  __asm__("mv %0, gp" : "=r"(p));
  return p;
}

// Return a pointer to the high-level percpu struct for the calling CPU.
static inline struct percpu* arch_get_curr_percpu(void) {
  return riscv64_read_percpu_ptr()->high_level_percpu;
}

// TODO(ZX-3068) get num_cpus from topology.
// This needs to be set very early (before arch_init).
static inline void arch_set_num_cpus(uint cpu_count) {
  extern uint riscv64_num_cpus;
  riscv64_num_cpus = cpu_count;
}

static inline uint arch_max_num_cpus(void) {
  extern uint riscv64_num_cpus;

  return riscv64_num_cpus;
}

void riscv64_init_percpu_early(uint hart_id, uint cpu_num);
void arch_register_hart(uint cpu_num, uint64_t hart_id);

static inline uint32_t riscv64_read_percpu_u32(size_t offset) {
  uint32_t val;

  // mark as volatile to force a read of the field to make sure
  // the compiler always emits a read when asked and does not cache
  // a copy between
  __asm__ volatile("lw %[val], %[offset](gp)" : [val] "=r"(val) : [offset] "Ir"(offset));
  return val;
}

static inline void riscv64_write_percpu_u32(size_t offset, uint32_t val) {
  __asm__("sw %[val], %[offset](gp)" ::[val] "r"(val), [offset] "Ir"(offset) : "memory");
}

#define READ_PERCPU_FIELD32(field) riscv64_read_percpu_u32(offsetof(struct riscv64_percpu, field))
#define WRITE_PERCPU_FIELD32(field, value) riscv64_write_percpu_u32(offsetof(struct riscv64_percpu, field), (value))

// Setup the high-level percpu struct pointer for |cpu_num|.
void arch_setup_percpu(cpu_num_t cpu_num, struct percpu *percpu);

static inline cpu_num_t arch_curr_cpu_num(void) {
  return READ_PERCPU_FIELD32(cpu_num);
}

static inline cpu_num_t riscv64_curr_hart_id(void) {
  return READ_PERCPU_FIELD32(hart_id);
}

__END_CDECLS

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_MP_H_

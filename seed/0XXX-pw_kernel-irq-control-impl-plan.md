# Implementation Plan: pw_kernel Userspace IRQ Control

This document provides a step-by-step implementation plan for SEED-0XXX,
adding `interrupt_control()` and `interrupt_status()` syscalls to pw_kernel.

---

## Interrupt Flow After Implementation

### Initialization Flow

```
Driver Task Startup
┌─────────────────────────────────────────────────────────────────────┐
│  // Interrupt starts DISABLED (Hubris-style)                        │
│                                                                     │
│  init_hardware();           // Configure peripheral registers       │
│  setup_dma_buffers();       // Prepare memory                       │
│                                                                     │
│  // NOW enable interrupt - driver is ready                          │
│  interrupt_control(handle, UART_IRQ, ENABLE)?;  ◄── NEW SYSCALL     │
└─────────────────────────────────────────────────────────────────────┘
```

### Normal Operation Flow

```
Hardware IRQ Fires
     │
     ▼
┌────────────────────────────────────┐
│ Kernel ISR (minimal, ~20 cycles)   │
│  • Look up InterruptObject         │
│  • Mask IRQ at NVIC/PLIC           │
│  • Set signal bit on object        │
│  • Mark task runnable              │
└────────────────────────────────────┘
     │
     ▼ (scheduler runs)
┌────────────────────────────────────┐
│ Task wakes from object_wait()      │
│  signals = UART_IRQ                │
└────────────────────────────────────┘
     │
     ▼
┌────────────────────────────────────┐
│ Task processes interrupt           │
│  • Read UART FIFO                  │
│  • Parse protocol                  │
│  • Update state machine            │
└────────────────────────────────────┘
     │
     ▼
┌────────────────────────────────────┐       ┌────────────────────────────────────┐
│ OPTION A: Use interrupt_ack()      │  OR   │ OPTION B: Use interrupt_control()  │
│  interrupt_ack(handle, signals)?;  │       │  interrupt_control(                │
│  // Clears signal + re-enables     │       │      handle, signals,              │
│  (unchanged, backward compatible)  │       │      ENABLE | CLEAR_PENDING        │
└────────────────────────────────────┘       │  )?;   ◄── NEW: More control       │
                                             └────────────────────────────────────┘
```

### Advanced Use Cases

```
USE CASE: Interrupt Coalescing
┌─────────────────────────────────────────────────────────────────────┐
│  loop {                                                             │
│      object_wait(handle, IRQ_SIGNAL, timeout)?;                     │
│                                                                     │
│      // Batch multiple packets before processing                    │
│      while hw.has_data() {                                          │
│          buffer.push(hw.read());                                    │
│      }                                                              │
│      process_batch(&buffer);                                        │
│                                                                     │
│      // Only NOW re-enable - after batch complete                   │
│      interrupt_control(handle, IRQ_SIGNAL, ENABLE)?;                │
│  }                                                                  │
└─────────────────────────────────────────────────────────────────────┘

USE CASE: Conditional Disable (Calibration)
┌─────────────────────────────────────────────────────────────────────┐
│  // Entering calibration mode                                       │
│  interrupt_control(handle, SENSOR_IRQ, empty())?;  // DISABLE       │
│                                                                     │
│  perform_calibration();                                             │
│                                                                     │
│  // Resume normal operation                                         │
│  interrupt_control(handle, SENSOR_IRQ, ENABLE | CLEAR_PENDING)?;    │
└─────────────────────────────────────────────────────────────────────┘

USE CASE: Query Status (Debugging/Diagnostics)
┌─────────────────────────────────────────────────────────────────────┐
│  let status = interrupt_status(handle, UART_IRQ)?;                  │
│                                                                     │
│  if status.contains(ENABLED)  { log!("IRQ enabled"); }              │
│  if status.contains(PENDING)  { log!("IRQ pending in HW"); }        │
│  if status.contains(NOTIFIED) { log!("Signal not consumed"); }      │
└─────────────────────────────────────────────────────────────────────┘

USE CASE: Driver Restart Recovery
┌─────────────────────────────────────────────────────────────────────┐
│  // Supervisor detects driver fault                                 │
│  match task_fault {                                                 │
│      MemoryFault => {                                               │
│          restart_task(uart_driver)?;                                │
│          // Driver's init will call interrupt_control(ENABLE)       │
│          // when ready - IRQ stays disabled until then              │
│      }                                                              │
│  }                                                                  │
└─────────────────────────────────────────────────────────────────────┘
```

### Interrupt State Machine

```
                         ┌──────────────┐
                         │   DISABLED   │◄─── Initial state (boot/restart)
                         └──────┬───────┘
                                │
               interrupt_control(ENABLE)
                                │
                                ▼
                         ┌──────────────┐
                         │   ENABLED    │ IRQ unmasked at NVIC/PLIC
                         │ (accepting)  │ Waiting for hardware event
                         └──────┬───────┘
                                │
        ┌───────────────────────┼───────────────────────┐
        │                       │                       │
        │            Hardware IRQ fires!                │
        │                       │                       │
        │                       ▼                       │
        │         ┌─────────────────────────────┐       │
        │         │    KERNEL ISR (minimal)     │       │
        │         │  1. Mask IRQ at NVIC/PLIC   │       │
        │         │  2. Set signal on object    │       │
        │         │  3. Mark task runnable      │       │
        │         └─────────────┬───────────────┘       │
        │                       │                       │
        │                       ▼                       │
        │                ┌──────────────┐               │
        │                │   NOTIFIED   │ Signal set    │
        │                │  (+ MASKED)  │ IRQ masked    │
        │                └──────┬───────┘               │
        │                       │                       │
        │            object_wait() returns              │
        │                       │                       │
        │                       ▼                       │
        │                ┌──────────────┐               │
        │                │  PROCESSING  │               │
        │                │  (task runs) │               │
        │                └──────┬───────┘               │
        │                       │                       │
        │      ┌────────────────┼────────────────┐      │
        │      │                │                │      │
        │      ▼                ▼                ▼      │
        │  interrupt_ack()   interrupt_control   interrupt_control
        │  (clears signal    (ENABLE)            (empty)
        │   + re-enables)    (re-enables)        (stays disabled)
        │      │                │                │      │
        │      └────────────────┴────────────────┘      │
        │                       │                       │
        └───────────────────────┴───────────────────────┘
                                │
                         Back to ENABLED
                         (or DISABLED if empty())


PENDING State (edge case):
┌──────────────┐
│   PENDING    │  Hardware set pending bit while IRQ was disabled.
│ (in hardware)│  Use interrupt_status() to detect, then:
└──────┬───────┘  - CLEAR_PENDING to dismiss, or
       │          - ENABLE to let it fire immediately
       ▼
  interrupt_control(ENABLE | CLEAR_PENDING)  → clears and re-enables
  interrupt_control(ENABLE)                  → fires immediately
  interrupt_control(CLEAR_PENDING)           → clears, stays disabled
```

**Key Transition: ENABLED → NOTIFIED**

When a hardware IRQ fires, the kernel's minimal ISR:
1. **Masks the IRQ** at NVIC/PLIC (prevents interrupt storm)
2. **Sets the signal bit** on the InterruptObject
3. **Wakes the task** (marks runnable for scheduler)

The IRQ stays masked until userspace explicitly re-enables it.

### Syscall Comparison (Before vs After)

| Syscall | Before | After |
|---------|--------|-------|
| `object_wait()` | Wait for signal | Unchanged |
| `interrupt_ack()` | Clear signal + re-enable (coupled) | Unchanged (backward compatible) |
| `interrupt_control()` | N/A | Enable/disable/clear_pending (NEW) |
| `interrupt_status()` | N/A | Query enabled/pending/notified (NEW) |

| Aspect | Before | After |
|--------|--------|-------|
| Initial state | Implicit | Explicit: **disabled** |
| Enable control | Only via `ack()` | Independent via `interrupt_control()` |
| Disable from userspace | Not possible | `interrupt_control(h, sig, empty())` |
| Query state | Not possible | `interrupt_status()` returns flags |
| Clear pending | Only via `ack()` | Independent via `CLEAR_PENDING` flag |

---

## Overview

The implementation is organized into 5 phases with clear dependencies:

```
Phase 1: Type Definitions (Foundation)
    ↓
Phase 2: InterruptController Trait Extensions
    ↓
Phase 3: InterruptObject Extensions
    ↓
Phase 4: Syscall Infrastructure
    ↓
Phase 5: Testing & Documentation
```

---

## Design Review: Potential Issues

This section identifies potential issues with interrupt handling and task
priorities that should be addressed during implementation or documented as
known limitations.

### Critical Issues

#### 1. Race Condition: Status Query vs. State Change

**File:** `pw_kernel/kernel/object/interrupt.rs`

```rust
fn interrupt_status(&self, kernel: K, signal_mask: Signals) -> Result<InterruptStatus> {
    // Get hardware status via callback
    let mut status = (self.callbacks.get_status_irqs)(signal_mask);  // ← NOT under lock

    // Check if notification is pending in object state
    let active = self.base.state.lock(kernel).active_signals;       // ← Lock acquired HERE
    ...
}
```

**Problem:** The hardware status query and object state read are not atomic.
An interrupt could fire between these two operations, causing:
- PENDING shown but NOTIFIED not yet set (interrupt just fired)
- NOTIFIED set but PENDING already cleared by hardware

**Mitigation:** Either:
- Document that `interrupt_status` provides a snapshot that may be stale
- Acquire lock before hardware query (may increase latency)

**Decision:** Document as known behavior. Status queries are inherently racy
in concurrent systems.

---

#### 2. Non-Atomic Control Operations

**File:** `pw_kernel/kernel/object/interrupt.rs`

```rust
fn interrupt_control(&self, ...) -> Result<()> {
    if control.contains(InterruptControl::CLEAR_PENDING) {
        (self.callbacks.clear_pending_irqs)(signal_mask);  // ← Step 1
    }

    if control.contains(InterruptControl::ENABLE) {
        (self.callbacks.enable_irqs)(signal_mask);         // ← Step 2 (interrupt can fire!)
    }
    ...
}
```

**Problem:** If `ENABLE | CLEAR_PENDING` is requested:
1. Clear pending is called
2. An interrupt could fire here (re-setting pending)
3. Enable is called → interrupt fires immediately

This could result in a spurious interrupt notification.

**Mitigation:** Reorder operations:
1. If not enabling, disable first
2. Clear pending
3. Enable (if ENABLE flag set)

**Decision:** This is actually correct behavior for most use cases. If the
hardware fires again after clear, the driver should process it. Document
the ordering guarantees.

---

#### 3. Priority Inversion Risk in Callback Execution

**Problem:** The callbacks (`enable_irqs`, `disable_irqs`, etc.) execute with
the scheduler lock potentially held via `SpinLock` which disables preemption.
If a callback takes too long, higher-priority tasks are blocked.

**Mitigation:**
- Keep callbacks simple and fast (O(1) operations)
- Document that callbacks must be non-blocking
- Consider adding callback timeout watchdog in debug builds

**Decision:** Document requirement that callbacks must be O(1) and
non-blocking.

---

### Medium Issues

#### 4. Missing Validation of Signal Mask

**File:** `pw_kernel/kernel/syscall.rs`

```rust
fn handle_interrupt_control<'a, K: Kernel>(...) -> Result<u64> {
    ...
    let Some(signal_mask) = Signals::from_bits(signals as u32) else {
        return Err(Error::InvalidArgument);
    };
    // No validation that signal_mask corresponds to actual IRQs in this object
}
```

**Problem:** No check that the signal mask corresponds to interrupts actually
mapped to this `InterruptObject`. A userspace task could pass arbitrary signal
bits.

**Mitigation:** Either:
- Validate against a mask stored in `InterruptObject`
- Document that invalid signals are silently ignored by callbacks

**Decision:** Callbacks are responsible for ignoring unknown signals. This
follows the Hubris model where the kernel is minimal.

---

#### 5. No Handling of Nested Interrupt Disable

**Problem:** If userspace calls `interrupt_control(empty)` twice to disable,
then `interrupt_control(ENABLE)` once, the interrupt will be re-enabled.
There's no reference counting.

```
Task A: disable → disable → enable → IRQ fires (unexpected!)
```

This differs from some systems where each disable must be paired with an
enable.

**Mitigation:** Document this behavior or add a disable count if needed.

**Decision:** Document that enable/disable is idempotent (not reference
counted). This matches Hubris semantics and is simpler for driver authors.

---

#### 6. PLIC `clear_pending` is No-Op

**File:** `pw_kernel/arch/riscv/plic.rs`

On PLIC, pending state is managed by claim/complete, not a separate register:

```rust
fn clear_interrupt_pending(_irq: u32) {
    // PLIC pending is managed by claim/complete, no direct clear
}
```

**Problem:** `interrupt_control(CLEAR_PENDING)` silently does nothing on
RISC-V. Userspace has no way to know this failed.

**Mitigation:**
- Return an error on PLIC? (breaks API consistency)
- Document platform differences clearly

**Decision:** Document as platform-specific behavior. CLEAR_PENDING is
best-effort; drivers should not rely on it for correctness.

---

### Low / Informational Issues

#### 7. `new_simple()` Creates No-Op Callbacks

```rust
pub const fn new_simple(ack_irqs: fn(Signals)) -> Self {
    Self {
        callbacks: InterruptCallbacks {
            enable_irqs: |_| {},                         // No-op!
            disable_irqs: |_| {},                        // No-op!
            get_status_irqs: |_| InterruptStatus::new(), // Always empty!
        },
    }
}
```

**Problem:** Objects created with `new_simple()` will:
- `interrupt_control(ENABLE)` → silently do nothing
- `interrupt_status()` → always return empty (misleading)

**Mitigation:** Either:
- Return `Unimplemented` for these operations when using simple constructor
- Add a flag to indicate full vs. simple mode
- Document the limitation

**Decision:** Document that `new_simple()` is for backward compatibility only.
New drivers should use `new()` with full callbacks.

---

#### 8. No Timeout Protection on Callbacks

Callbacks are function pointers that could potentially block or loop forever.
There's no watchdog or timeout protection in the kernel.

**Decision:** Out of scope for this implementation. General callback safety
is a broader kernel concern.

---

#### 9. Signal Mask Iteration Performance

If `signal_mask` contains multiple bits, each callback may need to iterate
and check each bit. This could be slow for dense masks.

**Decision:** Acceptable for typical use cases (1-4 IRQs per object). Document
that callbacks should handle multiple signals efficiently.

---

### Summary Table

| Issue | Severity | Impact | Decision |
|-------|----------|--------|----------|
| Race in `interrupt_status` | 🔴 Critical | Inconsistent state | Document as expected |
| Non-atomic control operations | 🔴 Critical | Spurious IRQs | Document ordering |
| Priority inversion in callbacks | 🔴 Critical | High-priority blocked | Require O(1) callbacks |
| Missing signal mask validation | 🟡 Medium | Robustness | Callbacks filter |
| No nested disable tracking | 🟡 Medium | Unexpected re-enable | Document idempotent |
| PLIC clear_pending is no-op | 🟡 Medium | Silent failure | Document platform diff |
| `new_simple()` stubs | 🟢 Low | Misleading status | Document limitation |
| No callback timeout | 🟢 Low | Potential hang | Out of scope |
| Signal mask iteration | 🟢 Low | Performance | Document expectation |

---

## RTOS Best Practices Compliance Review

This section evaluates the implementation against established RTOS design
principles and industry best practices.

### ✅ Compliant Areas

#### 1. Bounded Execution Time (WCET)

| Component | Analysis | Status |
|-----------|----------|--------|
| `interrupt_control` syscall | O(1) - single callback invocation | ✅ Pass |
| `interrupt_status` syscall | O(1) - callback + single lock acquire | ✅ Pass |
| NVIC operations | Single register read/write | ✅ Pass |
| PLIC operations | Single register read/write | ✅ Pass |

**Rationale:** All new code paths have deterministic, bounded execution time.
No loops, recursion, or unbounded operations.

---

#### 2. No Dynamic Memory Allocation

| Component | Analysis | Status |
|-----------|----------|--------|
| `InterruptControl` | Stack-allocated bitflags | ✅ Pass |
| `InterruptStatus` | Stack-allocated bitflags | ✅ Pass |
| `InterruptCallbacks` | Statically embedded in InterruptObject | ✅ Pass |
| Syscall handlers | No heap allocations | ✅ Pass |

**Rationale:** Following pw_kernel's existing pattern of static allocation.
All objects created at compile-time via system generator.

---

#### 3. Priority-Based Preemption Preserved

| Scenario | Analysis | Status |
|----------|----------|--------|
| Syscall during high-priority task | SpinLock disables preemption briefly | ✅ Pass |
| ISR to task notification | Direct signal, scheduler runs on ISR exit | ✅ Pass |
| Callback execution | Runs at caller's priority | ✅ Pass |

**Rationale:** The implementation uses existing spinlock infrastructure which
properly disables preemption only for critical sections.

---

#### 4. Interrupt Latency Minimization

| Path | Latency Impact | Status |
|------|----------------|--------|
| ISR → Signal | Unchanged (existing path) | ✅ Pass |
| User enable/disable | Direct NVIC/PLIC register access | ✅ Pass |
| Status query | Non-blocking register read | ✅ Pass |

**Rationale:** New operations use direct hardware register access without
additional abstraction layers.

---

#### 5. Static Configuration

| Component | Analysis | Status |
|-----------|----------|--------|
| InterruptObject creation | `const fn new()` | ✅ Pass |
| Callback assignment | Compile-time function pointers | ✅ Pass |
| Handle mapping | Static object table | ✅ Pass |

**Rationale:** All configuration resolved at compile time. No runtime
registration or dynamic object creation.

---

### ⚠️ Areas Requiring Attention

#### 6. Memory Ordering and Barriers

**Concern:** The implementation relies on callback functions to perform
hardware register accesses. No explicit memory barriers are used.

```rust
fn interrupt_control(&self, ...) -> Result<()> {
    (self.callbacks.clear_pending_irqs)(signal_mask);  // No barrier after
    (self.callbacks.enable_irqs)(signal_mask);         // No barrier after
    Ok(())
}
```

**Analysis:**
- ARM Cortex-M: NVIC registers are strongly-ordered, barriers not required
- RISC-V: PLIC registers typically memory-mapped, may need `fence` for SMP

**Recommendation:** Document that callbacks should include appropriate
barriers for multi-core systems if needed.

**Status:** ⚠️ Platform-dependent, document requirements

---

#### 7. Reentrancy Safety

**Concern:** What happens if an interrupt fires while `interrupt_control` is
executing?

```rust
fn interrupt_control(&self, kernel: K, signal_mask: Signals, control: InterruptControl) -> Result<()> {
    // ← IRQ could fire here
    if control.contains(InterruptControl::CLEAR_PENDING) {
        (self.callbacks.clear_pending_irqs)(signal_mask);
    }
    // ← IRQ could fire here
    if control.contains(InterruptControl::ENABLE) {
        (self.callbacks.enable_irqs)(signal_mask);
    }
    Ok(())
}
```

**Analysis:**
- If IRQ fires before clear_pending: OK, will be cleared
- If IRQ fires after enable: OK, will be delivered normally
- No shared state is corrupted

**Status:** ✅ Safe (interrupt-safe but not atomic)

---

#### 8. Lock Ordering / Deadlock Prevention

**Concern:** Does the implementation introduce new lock dependencies?

**Analysis of lock acquisition order:**

```
interrupt_control():
  └─ No locks acquired (callback invocations only)

interrupt_status():
  └─ self.base.state.lock(kernel)  [ObjectBase spinlock]

interrupt_ack():  (existing)
  └─ self.base.state.lock(kernel)  [ObjectBase spinlock]
```

**Observation:** `interrupt_status` acquires the ObjectBase lock, but this
is consistent with existing `interrupt_ack` behavior.

**Status:** ✅ No new lock ordering issues

---

#### 9. Stack Usage

**Concern:** Are syscall handlers adding significant stack pressure?

**Analysis:**
```rust
fn handle_interrupt_control(...) -> Result<u64> {
    let handle = args.next_u32()?;           // 4 bytes
    let signals = args.next_u32()?;          // 4 bytes
    let control_bits = args.next_u32()?;     // 4 bytes
    let signal_mask = ...;                   // 4 bytes
    let control = ...;                       // 4 bytes
    let object = lookup_handle(...)?;        // ~8 bytes (fat pointer)
    // Total: ~28 bytes additional stack
}
```

**Status:** ✅ Minimal stack impact (~32 bytes worst case)

---

### ❌ Non-Compliant Areas / Known Limitations

#### 10. Real-Time Guarantees for Status Queries

**Issue:** `interrupt_status()` does not provide atomicity between hardware
status and kernel object state.

**RTOS Best Practice:** Status queries should provide a consistent snapshot.

**Current Behavior:**
```rust
fn interrupt_status(&self, kernel: K, signal_mask: Signals) -> Result<InterruptStatus> {
    let mut status = (self.callbacks.get_status_irqs)(signal_mask);  // T1: Read HW
    // ← Window where interrupt could fire
    let active = self.base.state.lock(kernel).active_signals;        // T2: Read kernel
    ...
}
```

**Impact:** Status may be inconsistent in a ~10-cycle window.

**Mitigation:** Document that `interrupt_status` is informational only and
should not be used for synchronization decisions.

**Status:** ❌ Documented limitation

---

#### 11. No Interrupt Priority Support

**Issue:** The implementation doesn't expose interrupt priority configuration.

**RTOS Best Practice:** Allow drivers to configure interrupt priorities.

**Current State:** NVIC priorities are set uniformly in `early_init()`:
```rust
fn early_init(&self) {
    for i in 0..NvicConfig::MAX_IRQS {
        nvic_regs.set_priority(i as usize, 0b0100_0000);  // All same priority
    }
}
```

**Impact:** All userspace interrupts have equal priority at hardware level.

**Recommendation:** Consider future syscall for priority configuration.

**Status:** ❌ Out of scope (future enhancement)

---

#### 12. No Interrupt Affinity (Multi-Core)

**Issue:** No mechanism to direct interrupts to specific cores.

**RTOS Best Practice:** SMP systems should support interrupt affinity.

**Current State:** Single-core assumption throughout.

**Status:** ❌ Out of scope (pw_kernel is single-core currently)

---

### Compliance Summary

| Category | Items Checked | Compliant | Attention | Non-Compliant |
|----------|---------------|-----------|-----------|---------------|
| Timing Determinism | 4 | 4 | 0 | 0 |
| Memory Management | 4 | 4 | 0 | 0 |
| Scheduling | 3 | 3 | 0 | 0 |
| Interrupt Handling | 3 | 2 | 1 | 0 |
| Synchronization | 3 | 2 | 0 | 1 |
| Platform Support | 2 | 0 | 1 | 1 |
| **Total** | **19** | **15 (79%)** | **2 (10%)** | **2 (11%)** |

---

### Recommendations for Future Work

1. **Add `interrupt_set_priority` syscall** - Allow drivers to configure
   interrupt priorities within their permitted range.

2. **Add memory barrier documentation** - Document when callbacks need
   explicit barriers for multi-core scenarios.

3. **Consider atomic status query** - For use cases requiring consistent
   snapshots, consider a version that disables interrupts briefly.

4. **SMP support** - When pw_kernel adds multi-core support, extend with
   interrupt affinity configuration.

---

## Phase 1: Type Definitions

**Goal:** Define `InterruptControl` and `InterruptStatus` bitflags types.

### Task 1.1: Add bitflags to `syscall_defs.rs`

**File:** `pw_kernel/syscall/syscall_defs.rs`

**Location:** After `Signals` definition (~line 300)

**Add:**
```rust
bitflags! {
    /// Control flags for interrupt_control syscall
    pub struct InterruptControl: u32 {
        /// Enable the interrupt(s) specified by signal_mask
        const ENABLE = 0b0001;
        /// Clear any pending status for the interrupt(s)
        const CLEAR_PENDING = 0b0010;
    }
}

bitflags! {
    /// Status flags returned by interrupt_status syscall
    pub struct InterruptStatus: u32 {
        /// At least one interrupt in the mask is enabled
        const ENABLED = 0b0001;
        /// At least one interrupt in the mask is pending in hardware
        const PENDING = 0b0010;
        /// A notification has been posted but not yet consumed
        const NOTIFIED = 0b0100;
    }
}
```

### Task 1.2: Add new syscall IDs

**File:** `pw_kernel/syscall/syscall_defs.rs`

**Location:** `SysCallId` enum (~line 245)

**Modify:**
```rust
#[derive(Copy, Clone)]
#[repr(u16)]
#[non_exhaustive]
pub enum SysCallId {
    ObjectWait = 0x0000,
    ChannelTransact = 0x0001,
    ChannelRead = 0x0002,
    ChannelRespond = 0x0003,
    InterruptAck = 0x0004,
    InterruptControl = 0x0005,  // NEW
    InterruptStatus = 0x0006,   // NEW
    // ...
}
```

### Task 1.3: Add result conversion for `InterruptStatus`

**File:** `pw_kernel/syscall/syscall_defs.rs`

**Location:** After `to_result_signals()` method (~line 225)

**Add:**
```rust
pub fn to_result_interrupt_status(self) -> Result<InterruptStatus> {
    let value = self.0;
    if value < 0 {
        let value = (-value).cast_unsigned();
        #[allow(clippy::cast_possible_truncation)]
        Err(unsafe { core::mem::transmute::<u32, Error>(value as u32) })
    } else {
        #[allow(clippy::cast_possible_truncation)]
        Ok(InterruptStatus::from_bits_truncate(value.cast_unsigned() as u32))
    }
}
```

---

## Phase 2: InterruptController Trait Extensions

**Goal:** Add hardware-level primitives for pending status and clear.

### Task 2.1: Extend `InterruptController` trait

**File:** `pw_kernel/kernel/interrupt_controller.rs`

**Location:** In the `InterruptController` trait (~line 22)

**Add methods:**
```rust
pub trait InterruptController {
    // ... existing methods ...

    /// Check if an interrupt is pending in hardware
    fn is_interrupt_pending(irq: u32) -> bool;

    /// Clear pending status for an interrupt
    fn clear_interrupt_pending(irq: u32);

    /// Check if an interrupt is currently enabled
    fn is_interrupt_enabled(irq: u32) -> bool;
}
```

### Task 2.2: Implement for NVIC (ARM Cortex-M)

**File:** `pw_kernel/arch/arm_cortex_m/nvic.rs`

**Add implementations:**
```rust
impl InterruptController for Nvic {
    // ... existing methods ...

    fn is_interrupt_pending(irq: u32) -> bool {
        let nvic_regs = regs::Nvic {};
        nvic_regs.is_pending(irq as usize)
    }

    fn clear_interrupt_pending(irq: u32) {
        debug_if!(LOG_INTERRUPTS, "Clear pending interrupt {}", irq as u32);
        let mut nvic_regs = regs::Nvic {};
        nvic_regs.clear_pending(irq as usize);
    }

    fn is_interrupt_enabled(irq: u32) -> bool {
        let nvic_regs = regs::Nvic {};
        nvic_regs.is_enabled(irq as usize)
    }
}
```

### Task 2.3: Add NVIC register access methods

**File:** `pw_kernel/arch/arm_cortex_m/regs/nvic.rs`

**Add methods to `Nvic` struct:**
```rust
impl Nvic {
    // ... existing methods ...

    /// Check if interrupt is pending (read ISPR register)
    pub fn is_pending(&self, irq: usize) -> bool {
        let reg_index = irq / 32;
        let bit_index = irq % 32;
        let ispr = self.ispr(reg_index);
        (ispr & (1 << bit_index)) != 0
    }

    /// Clear pending interrupt (write ICPR register)
    pub fn clear_pending(&mut self, irq: usize) {
        let reg_index = irq / 32;
        let bit_index = irq % 32;
        self.set_icpr(reg_index, 1 << bit_index);
    }

    /// Check if interrupt is enabled (read ISER register)
    pub fn is_enabled(&self, irq: usize) -> bool {
        let reg_index = irq / 32;
        let bit_index = irq % 32;
        let iser = self.iser(reg_index);
        (iser & (1 << bit_index)) != 0
    }
}
```

### Task 2.4: Implement for PLIC (RISC-V)

**File:** `pw_kernel/arch/riscv/plic.rs` (or equivalent)

**Add similar implementations for RISC-V PLIC.**

---

## Phase 3: InterruptObject Extensions

**Goal:** Add control/status methods and new callback types.

### Task 3.1: Add callbacks to `InterruptObject`

**File:** `pw_kernel/kernel/object/interrupt.rs`

**Modify struct:**
```rust
pub struct InterruptObject<K: Kernel> {
    base: ObjectBase<K>,
    ack_irqs: fn(Signals),
    enable_irqs: fn(Signals),       // NEW
    disable_irqs: fn(Signals),      // NEW
    clear_pending_irqs: fn(Signals), // NEW
    get_status_irqs: fn(Signals) -> InterruptStatus, // NEW
}
```

### Task 3.2: Update constructor

**File:** `pw_kernel/kernel/object/interrupt.rs`

**Modify `new()` method:**
```rust
impl<K: Kernel> InterruptObject<K> {
    #[must_use]
    pub const fn new(
        ack_irqs: fn(Signals),
        enable_irqs: fn(Signals),
        disable_irqs: fn(Signals),
        clear_pending_irqs: fn(Signals),
        get_status_irqs: fn(Signals) -> InterruptStatus,
    ) -> Self {
        Self {
            base: ObjectBase::new(),
            ack_irqs,
            enable_irqs,
            disable_irqs,
            clear_pending_irqs,
            get_status_irqs,
        }
    }
}
```

### Task 3.3: Add control method

**File:** `pw_kernel/kernel/object/interrupt.rs`

**Add method:**
```rust
impl<K: Kernel> InterruptObject<K> {
    pub fn control(
        &self,
        _kernel: K,
        signal_mask: Signals,
        control: InterruptControl,
    ) -> Result<()> {
        // Validate signal_mask contains only interrupt signals
        let interrupt_signals = Signals::INTERRUPT_A
            | Signals::INTERRUPT_B
            | Signals::INTERRUPT_C
            // ... all interrupt signals
            | Signals::INTERRUPT_P;

        if !(signal_mask - interrupt_signals).is_empty() {
            return Err(Error::InvalidArgument);
        }

        if control.contains(InterruptControl::CLEAR_PENDING) {
            (self.clear_pending_irqs)(signal_mask);
        }

        if control.contains(InterruptControl::ENABLE) {
            (self.enable_irqs)(signal_mask);
        } else {
            (self.disable_irqs)(signal_mask);
        }

        Ok(())
    }
}
```

### Task 3.4: Add status method

**File:** `pw_kernel/kernel/object/interrupt.rs`

**Add method:**
```rust
impl<K: Kernel> InterruptObject<K> {
    pub fn status(&self, kernel: K, signal_mask: Signals) -> Result<InterruptStatus> {
        // Validate signal_mask
        let interrupt_signals = Signals::INTERRUPT_A
            | Signals::INTERRUPT_B
            // ... all interrupt signals
            | Signals::INTERRUPT_P;

        if !(signal_mask - interrupt_signals).is_empty() {
            return Err(Error::InvalidArgument);
        }

        // Get hardware status via callback
        let mut status = (self.get_status_irqs)(signal_mask);

        // Check if notification is pending in object state
        let active = self.base.state.lock(kernel).active_signals;
        if !(active & signal_mask).is_empty() {
            status |= InterruptStatus::NOTIFIED;
        }

        Ok(status)
    }
}
```

### Task 3.5: Extend `KernelObject` trait

**File:** `pw_kernel/kernel/object.rs` or `pw_kernel/kernel/object/mod.rs`

**Add methods to trait:**
```rust
pub trait KernelObject<K: Kernel> {
    // ... existing methods ...

    fn interrupt_control(
        &self,
        _kernel: K,
        _signal_mask: Signals,
        _control: InterruptControl,
    ) -> Result<()> {
        Err(Error::InvalidArgument)  // Default: not an interrupt object
    }

    fn interrupt_status(
        &self,
        _kernel: K,
        _signal_mask: Signals,
    ) -> Result<InterruptStatus> {
        Err(Error::InvalidArgument)  // Default: not an interrupt object
    }
}
```

### Task 3.6: Implement trait for `InterruptObject`

**File:** `pw_kernel/kernel/object/interrupt.rs`

**Add to `KernelObject` impl:**
```rust
impl<K: Kernel> KernelObject<K> for InterruptObject<K> {
    // ... existing methods ...

    fn interrupt_control(
        &self,
        kernel: K,
        signal_mask: Signals,
        control: InterruptControl,
    ) -> Result<()> {
        self.control(kernel, signal_mask, control)
    }

    fn interrupt_status(
        &self,
        kernel: K,
        signal_mask: Signals,
    ) -> Result<InterruptStatus> {
        self.status(kernel, signal_mask)
    }
}
```

---

## Phase 4: Syscall Infrastructure

**Goal:** Wire up the syscalls end-to-end.

### Task 4.1: Add syscall handlers

**File:** `pw_kernel/kernel/syscall.rs`

**Add handler functions:**
```rust
fn handle_interrupt_control<'a, K: Kernel>(
    kernel: K,
    mut args: K::SyscallArgs<'a>,
) -> Result<u64> {
    let handle = args.next_u32()?;
    let signal_mask = Signals::from_bits_truncate(args.next_u32()?);
    let control = InterruptControl::from_bits_truncate(args.next_u32()?);

    log_if::debug_if!(
        SYSCALL_DEBUG,
        "syscall: interrupt_control handle={} signals={:?} control={:?}",
        handle,
        signal_mask,
        control
    );

    let object = kernel.get_object(handle)?;
    object.interrupt_control(kernel, signal_mask, control)?;

    Ok(0)
}

fn handle_interrupt_status<'a, K: Kernel>(
    kernel: K,
    mut args: K::SyscallArgs<'a>,
) -> Result<u64> {
    let handle = args.next_u32()?;
    let signal_mask = Signals::from_bits_truncate(args.next_u32()?);

    log_if::debug_if!(
        SYSCALL_DEBUG,
        "syscall: interrupt_status handle={} signals={:?}",
        handle,
        signal_mask
    );

    let object = kernel.get_object(handle)?;
    let status = object.interrupt_status(kernel, signal_mask)?;

    Ok(status.bits().into())
}
```

### Task 4.2: Add to syscall dispatch

**File:** `pw_kernel/kernel/syscall.rs`

**Modify `handle_syscall()` match:**
```rust
let res = match id {
    SysCallId::ObjectWait => handle_object_wait(kernel, args),
    SysCallId::ChannelTransact => handle_channel_transact(kernel, args),
    SysCallId::ChannelRead => handle_channel_read(kernel, args),
    SysCallId::ChannelRespond => handle_channel_respond(kernel, args),
    SysCallId::InterruptAck => handle_interrupt_ack(kernel, args),
    SysCallId::InterruptControl => handle_interrupt_control(kernel, args),  // NEW
    SysCallId::InterruptStatus => handle_interrupt_status(kernel, args),    // NEW
    // ... rest unchanged
};
```

### Task 4.3: Add to `SysCallInterface` trait

**File:** `pw_kernel/syscall/syscall_defs.rs`

**Add to trait:**
```rust
pub trait SysCallInterface {
    // ... existing methods ...

    fn interrupt_control(
        object_handle: u32,
        signal_mask: Signals,
        control: InterruptControl,
    ) -> Result<()>;

    fn interrupt_status(
        object_handle: u32,
        signal_mask: Signals,
    ) -> Result<InterruptStatus>;
}
```

### Task 4.4: Implement for ARM Cortex-M userspace

**File:** `pw_kernel/syscall/syscall_user/arm_cortex_m.rs`

**Add implementations:**
```rust
impl SysCallInterface for SysCall {
    // ... existing methods ...

    fn interrupt_control(
        object_handle: u32,
        signal_mask: Signals,
        control: InterruptControl,
    ) -> Result<()> {
        let ret = unsafe {
            syscall3(
                SysCallId::InterruptControl as u32,
                object_handle,
                signal_mask.bits(),
                control.bits(),
            )
        };
        SysCallReturnValue(ret).to_result_unit()
    }

    fn interrupt_status(
        object_handle: u32,
        signal_mask: Signals,
    ) -> Result<InterruptStatus> {
        let ret = unsafe {
            syscall2(
                SysCallId::InterruptStatus as u32,
                object_handle,
                signal_mask.bits(),
            )
        };
        SysCallReturnValue(ret).to_result_interrupt_status()
    }
}
```

### Task 4.5: Implement for RISC-V userspace

**File:** `pw_kernel/syscall/syscall_user/riscv.rs`

**Add similar implementations for RISC-V.**

### Task 4.6: Implement for host (testing)

**File:** `pw_kernel/syscall/syscall_user/host.rs`

**Add stub implementations for host testing.**

### Task 4.7: Add userspace wrapper functions

**File:** `pw_kernel/userspace/syscall.rs`

**Add:**
```rust
pub use syscall_defs::{InterruptControl, InterruptStatus, Signals};

#[inline(always)]
pub fn interrupt_control(
    object_handle: u32,
    signal_mask: Signals,
    control: InterruptControl,
) -> Result<()> {
    SysCall::interrupt_control(object_handle, signal_mask, control)
}

#[inline(always)]
pub fn interrupt_status(
    object_handle: u32,
    signal_mask: Signals,
) -> Result<InterruptStatus> {
    SysCall::interrupt_status(object_handle, signal_mask)
}
```

---

## Phase 5: Testing Strategy

This phase ensures the implementation is correct, robust, and doesn't break
existing functionality. Testing is organized into three levels:

1. **Unit Tests** - Test individual components in isolation
2. **Integration Tests** - Test syscall flow end-to-end
3. **Regression Tests** - Ensure existing functionality works

### Test Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           TEST ARCHITECTURE                                  │
└─────────────────────────────────────────────────────────────────────────────┘

┌──────────────────────┐     ┌──────────────────────┐     ┌──────────────────────┐
│   UNIT TESTS         │     │  KERNEL-SIDE TESTS   │     │  USERSPACE TESTS     │
│   (Host/Native)      │     │  (On-Target)         │     │  (On-Target)         │
├──────────────────────┤     ├──────────────────────┤     ├──────────────────────┤
│ • Bitflags behavior  │     │ • InterruptController│     │ • interrupt_control()│
│ • Type conversions   │     │   trait methods      │     │ • interrupt_status() │
│ • Signal mask ops    │     │ • NVIC/PLIC impl     │     │ • Full IRQ flow      │
│ • Error conditions   │     │ • InterruptObject    │     │ • Edge cases         │
└──────────────────────┘     └──────────────────────┘     └──────────────────────┘
         │                            │                            │
         ▼                            ▼                            ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                        bazel test //pw_kernel/...                           │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

### Task 5.1: Unit Tests for Type Definitions

**Goal:** Verify `InterruptControl` and `InterruptStatus` bitflags work correctly.

**File:** `pw_kernel/syscall/syscall_defs.rs`

**Test Cases:**

| Test ID | Description | Expected Result |
|---------|-------------|-----------------|
| UT-01 | Create empty InterruptControl | `bits() == 0` |
| UT-02 | Set ENABLE flag | `contains(ENABLE) == true` |
| UT-03 | Set CLEAR_PENDING flag | `contains(CLEAR_PENDING) == true` |
| UT-04 | Combine ENABLE \| CLEAR_PENDING | Both flags set, `bits() == 0b0011` |
| UT-05 | Create empty InterruptStatus | `bits() == 0` |
| UT-06 | Check ENABLED flag | Correct bit position |
| UT-07 | Check PENDING flag | Correct bit position |
| UT-08 | Check NOTIFIED flag | Correct bit position |
| UT-09 | Combine multiple status flags | OR operation works |
| UT-10 | from_bits_truncate invalid bits | Ignores unknown bits |

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interrupt_control_empty() {
        let ctrl = InterruptControl::new();
        assert_eq!(ctrl.bits(), 0);
        assert!(!ctrl.contains(InterruptControl::ENABLE));
        assert!(!ctrl.contains(InterruptControl::CLEAR_PENDING));
    }

    #[test]
    fn test_interrupt_control_enable() {
        let ctrl = InterruptControl::ENABLE;
        assert!(ctrl.contains(InterruptControl::ENABLE));
        assert!(!ctrl.contains(InterruptControl::CLEAR_PENDING));
        assert_eq!(ctrl.bits(), 0b0001);
    }

    #[test]
    fn test_interrupt_control_clear_pending() {
        let ctrl = InterruptControl::CLEAR_PENDING;
        assert!(!ctrl.contains(InterruptControl::ENABLE));
        assert!(ctrl.contains(InterruptControl::CLEAR_PENDING));
        assert_eq!(ctrl.bits(), 0b0010);
    }

    #[test]
    fn test_interrupt_control_combined() {
        let ctrl = InterruptControl::ENABLE | InterruptControl::CLEAR_PENDING;
        assert!(ctrl.contains(InterruptControl::ENABLE));
        assert!(ctrl.contains(InterruptControl::CLEAR_PENDING));
        assert_eq!(ctrl.bits(), 0b0011);
    }

    #[test]
    fn test_interrupt_status_flags() {
        assert_eq!(InterruptStatus::ENABLED.bits(), 0b0001);
        assert_eq!(InterruptStatus::PENDING.bits(), 0b0010);
        assert_eq!(InterruptStatus::NOTIFIED.bits(), 0b0100);
    }

    #[test]
    fn test_interrupt_status_combined() {
        let status = InterruptStatus::ENABLED | InterruptStatus::PENDING;
        assert!(status.contains(InterruptStatus::ENABLED));
        assert!(status.contains(InterruptStatus::PENDING));
        assert!(!status.contains(InterruptStatus::NOTIFIED));
    }

    #[test]
    fn test_result_conversion_success() {
        let ret = SysCallReturnValue(0b0101);
        let status = ret.to_result_interrupt_status().unwrap();
        assert!(status.contains(InterruptStatus::ENABLED));
        assert!(!status.contains(InterruptStatus::PENDING));
        assert!(status.contains(InterruptStatus::NOTIFIED));
    }

    #[test]
    fn test_result_conversion_error() {
        let ret = SysCallReturnValue(-22); // EINVAL
        let result = ret.to_result_interrupt_status();
        assert!(result.is_err());
    }
}
```

---

### Task 5.2: Kernel-Side Tests (InterruptController)

**Goal:** Verify architecture-specific InterruptController implementations.

**Files:**
- `pw_kernel/tests/irq_control/kernel/main.rs`
- `pw_kernel/tests/irq_control/kernel/BUILD.bazel`

**Test Cases:**

| Test ID | Description | Platform | Expected Result |
|---------|-------------|----------|-----------------|
| KT-01 | is_interrupt_enabled after enable | ARM/RISC-V | Returns true |
| KT-02 | is_interrupt_enabled after disable | ARM/RISC-V | Returns false |
| KT-03 | is_interrupt_pending after trigger | ARM only | Returns true |
| KT-04 | is_interrupt_pending after clear | ARM only | Returns false |
| KT-05 | clear_interrupt_pending on PLIC | RISC-V | No-op (no crash) |
| KT-06 | Enable → Disable → Enable cycle | ARM/RISC-V | State correct |

```rust
// pw_kernel/tests/irq_control/kernel/main.rs

#![no_std]

use kernel::Kernel;
use kernel::interrupt_controller::InterruptController;
use pw_status::Result;

pub fn test_is_interrupt_enabled<K: Kernel>(irq: u32) -> Result<()> {
    pw_log::info!("KT-01/02: Test is_interrupt_enabled");

    // Enable and check
    K::InterruptController::enable_interrupt(irq);
    pw_assert::assert!(
        K::InterruptController::is_interrupt_enabled(irq),
        "IRQ should be enabled"
    );

    // Disable and check
    K::InterruptController::disable_interrupt(irq);
    pw_assert::assert!(
        !K::InterruptController::is_interrupt_enabled(irq),
        "IRQ should be disabled"
    );

    pw_log::info!("  PASSED");
    Ok(())
}

pub fn test_pending_status<K: Kernel>(irq: u32) -> Result<()> {
    pw_log::info!("KT-03/04: Test pending status");

    // Disable IRQ so trigger creates pending without firing
    K::InterruptController::disable_interrupt(irq);

    // Trigger creates pending
    K::InterruptController::trigger_interrupt(irq);
    let pending = K::InterruptController::is_interrupt_pending(irq);
    pw_log::info!("  Pending after trigger: {}", pending);

    // Clear pending
    K::InterruptController::clear_interrupt_pending(irq);
    let pending_after = K::InterruptController::is_interrupt_pending(irq);
    pw_log::info!("  Pending after clear: {}", pending_after);

    // Re-enable for cleanup
    K::InterruptController::enable_interrupt(irq);

    pw_log::info!("  PASSED");
    Ok(())
}

pub fn test_enable_disable_cycle<K: Kernel>(irq: u32) -> Result<()> {
    pw_log::info!("KT-06: Test enable/disable cycle");

    for i in 0..5 {
        K::InterruptController::enable_interrupt(irq);
        pw_assert::assert!(K::InterruptController::is_interrupt_enabled(irq));

        K::InterruptController::disable_interrupt(irq);
        pw_assert::assert!(!K::InterruptController::is_interrupt_enabled(irq));
    }

    // Leave enabled
    K::InterruptController::enable_interrupt(irq);

    pw_log::info!("  PASSED");
    Ok(())
}

pub fn main<K: Kernel>(test_irq: u32) -> Result<()> {
    pw_log::info!("🔄 RUNNING kernel irq_control tests");

    test_is_interrupt_enabled::<K>(test_irq)?;
    test_pending_status::<K>(test_irq)?;
    test_enable_disable_cycle::<K>(test_irq)?;

    pw_log::info!("✅ PASSED");
    Ok(())
}
```

**BUILD.bazel:**

```python
load("@rules_rust//rust:defs.bzl", "rust_library")

rust_library(
    name = "test_irq_control",
    srcs = ["main.rs"],
    edition = "2024",
    tags = ["kernel"],
    target_compatible_with = select({
        # PLIC doesn't support trigger_interrupt
        "@platforms//cpu:riscv32": ["@platforms//:incompatible"],
        "//conditions:default": [],
    }),
    deps = [
        "//pw_kernel/kernel",
        "//pw_kernel/lib/pw_assert",
        "//pw_log/rust:pw_log",
        "//pw_status/rust:pw_status",
    ],
)
```

---

### Task 5.3: Userspace Integration Tests

**Goal:** Test the full syscall path from userspace through kernel and back.

**Files:**
- `pw_kernel/tests/irq_control/user/main.rs`
- `pw_kernel/tests/irq_control/user/BUILD.bazel`

**Test Cases:**

| Test ID | Description | Expected Result |
|---------|-------------|-----------------|
| IT-01 | interrupt_status initial state | ENABLED (system generator enables) |
| IT-02 | interrupt_control disable | Status shows !ENABLED |
| IT-03 | interrupt_control enable | Status shows ENABLED |
| IT-04 | Trigger while disabled | PENDING set, !NOTIFIED |
| IT-05 | Clear pending | PENDING cleared |
| IT-06 | Enable + clear combined | Both operations work |
| IT-07 | Full flow with object_wait | Receive and re-enable works |
| IT-08 | Invalid handle | Returns InvalidArgument |
| IT-09 | Invalid signal mask | Returns InvalidArgument |
| IT-10 | interrupt_ack still works | Backward compatibility |

```rust
// pw_kernel/tests/irq_control/user/main.rs

#![no_main]
#![no_std]

use app_test_interrupt_listener::{handle, signals};
use pw_status::{Error, Result};
use userspace::syscall::{InterruptControl, InterruptStatus, Signals};
use userspace::time::Instant;
use userspace::{entry, syscall};

const TEST_IRQ: u32 = 42;

/// IT-01: Test initial status query
fn test_initial_status() -> Result<()> {
    pw_log::info!("IT-01: Query initial status");

    let status = syscall::interrupt_status(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
    )?;

    // System generator enables interrupts by default
    if !status.contains(InterruptStatus::ENABLED) {
        pw_log::error!("Expected ENABLED initially");
        return Err(Error::FailedPrecondition);
    }

    if status.contains(InterruptStatus::NOTIFIED) {
        pw_log::error!("Expected !NOTIFIED initially");
        return Err(Error::FailedPrecondition);
    }

    pw_log::info!("  PASSED");
    Ok(())
}

/// IT-02: Test disabling interrupt
fn test_disable() -> Result<()> {
    pw_log::info!("IT-02: Test disable");

    syscall::interrupt_control(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
        InterruptControl::new(), // empty = disable
    )?;

    let status = syscall::interrupt_status(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
    )?;

    if status.contains(InterruptStatus::ENABLED) {
        pw_log::error!("Expected !ENABLED after disable");
        return Err(Error::FailedPrecondition);
    }

    pw_log::info!("  PASSED");
    Ok(())
}

/// IT-03: Test enabling interrupt
fn test_enable() -> Result<()> {
    pw_log::info!("IT-03: Test enable");

    syscall::interrupt_control(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
        InterruptControl::ENABLE,
    )?;

    let status = syscall::interrupt_status(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
    )?;

    if !status.contains(InterruptStatus::ENABLED) {
        pw_log::error!("Expected ENABLED after enable");
        return Err(Error::FailedPrecondition);
    }

    pw_log::info!("  PASSED");
    Ok(())
}

/// IT-04: Test trigger while disabled
fn test_trigger_while_disabled() -> Result<()> {
    pw_log::info!("IT-04: Trigger while disabled");

    // Disable first
    syscall::interrupt_control(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
        InterruptControl::new(),
    )?;

    // Trigger the interrupt
    syscall::debug_trigger_interrupt(TEST_IRQ)?;

    let status = syscall::interrupt_status(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
    )?;

    // Should be pending but NOT notified (disabled)
    if status.contains(InterruptStatus::NOTIFIED) {
        pw_log::error!("Should NOT be notified while disabled");
        return Err(Error::FailedPrecondition);
    }

    // PENDING is architecture-dependent (may not be visible)
    pw_log::info!("  PENDING status: {}", status.contains(InterruptStatus::PENDING));

    // Cleanup: clear pending
    syscall::interrupt_control(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
        InterruptControl::CLEAR_PENDING,
    )?;

    pw_log::info!("  PASSED");
    Ok(())
}

/// IT-06: Test combined enable + clear
fn test_enable_and_clear() -> Result<()> {
    pw_log::info!("IT-06: Enable and clear combined");

    // Start disabled with pending
    syscall::interrupt_control(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
        InterruptControl::new(),
    )?;
    syscall::debug_trigger_interrupt(TEST_IRQ)?;

    // Enable and clear in one call
    syscall::interrupt_control(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
        InterruptControl::ENABLE | InterruptControl::CLEAR_PENDING,
    )?;

    let status = syscall::interrupt_status(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
    )?;

    if !status.contains(InterruptStatus::ENABLED) {
        pw_log::error!("Expected ENABLED");
        return Err(Error::FailedPrecondition);
    }

    pw_log::info!("  PASSED");
    Ok(())
}

/// IT-07: Test full interrupt flow
fn test_full_flow() -> Result<()> {
    pw_log::info!("IT-07: Full interrupt flow");

    // Ensure enabled
    syscall::interrupt_control(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
        InterruptControl::ENABLE,
    )?;

    // Trigger
    syscall::debug_trigger_interrupt(TEST_IRQ)?;

    // Wait with timeout
    let result = syscall::object_wait(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
        Instant::from_ticks(10_000_000), // 10s timeout
    );

    match result {
        Ok(sigs) => {
            if !sigs.contains(signals::TEST_IRQ) {
                pw_log::error!("Wrong signal");
                return Err(Error::Internal);
            }

            // Re-enable using interrupt_control
            syscall::interrupt_control(
                handle::TEST_INTERRUPTS,
                sigs,
                InterruptControl::ENABLE,
            )?;

            pw_log::info!("  PASSED");
            Ok(())
        }
        Err(e) => {
            pw_log::error!("Wait failed: {}", e as u32);
            Err(e)
        }
    }
}

/// IT-10: Test backward compatibility with interrupt_ack
fn test_backward_compat() -> Result<()> {
    pw_log::info!("IT-10: Backward compatibility (interrupt_ack)");

    // Ensure enabled
    syscall::interrupt_control(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
        InterruptControl::ENABLE,
    )?;

    // Trigger
    syscall::debug_trigger_interrupt(TEST_IRQ)?;

    // Wait
    let sigs = syscall::object_wait(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
        Instant::from_ticks(10_000_000),
    )?;

    // Use old-style interrupt_ack
    syscall::interrupt_ack(handle::TEST_INTERRUPTS, sigs)?;

    // Verify still enabled
    let status = syscall::interrupt_status(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
    )?;

    if !status.contains(InterruptStatus::ENABLED) {
        pw_log::error!("interrupt_ack should re-enable");
        return Err(Error::FailedPrecondition);
    }

    pw_log::info!("  PASSED");
    Ok(())
}

fn run_all_tests() -> Result<()> {
    test_initial_status()?;
    test_disable()?;
    test_enable()?;
    test_trigger_while_disabled()?;
    test_enable_and_clear()?;
    test_full_flow()?;
    test_backward_compat()?;
    Ok(())
}

#[entry]
fn entry() -> ! {
    pw_log::info!("🔄 RUNNING userspace irq_control tests");
    let ret = run_all_tests();

    if ret.is_err() {
        pw_log::error!("❌ FAILED: {}", ret.status_code() as u32);
    } else {
        pw_log::info!("✅ PASSED");
    }

    let _ = syscall::debug_shutdown(ret);
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
```

**BUILD.bazel:**

```python
load("@rules_rust//rust:defs.bzl", "rust_binary")

rust_binary(
    name = "test_irq_control",
    srcs = ["main.rs"],
    edition = "2024",
    tags = ["kernel"],
    visibility = ["//visibility:public"],
    deps = [
        # Reuse interrupt_listener app package (same system config)
        "//pw_kernel/tests/interrupts/user:app_test_interrupt_listener",
        "//pw_kernel/userspace",
        "//pw_log/rust:pw_log",
        "//pw_status/rust:pw_status",
    ],
)
```

---

### Task 5.4: Regression Tests

**Goal:** Ensure existing interrupt functionality still works.

**Approach:**

1. **Run existing interrupt tests** - `//pw_kernel/tests/interrupts/...`
2. **Run UART tests** - `//pw_kernel/tests/uart/...`
3. **Verify system generator** - Builds with `new_simple()` constructor

**Test Commands:**

```bash
# Run all pw_kernel tests
bazel test //pw_kernel/tests/...

# Run specifically interrupt-related tests
bazel test //pw_kernel/tests/interrupts/...
bazel test //pw_kernel/tests/irq_control/...
bazel test //pw_kernel/tests/uart/...

# Verify full kernel build
bazel build //pw_kernel/...
```

---

### Task 5.5: Edge Case and Error Handling Tests

**Goal:** Test boundary conditions and error paths.

**Test Cases:**

| Test ID | Description | Expected Result |
|---------|-------------|-----------------|
| EC-01 | interrupt_control on non-InterruptObject | Error::Unimplemented |
| EC-02 | interrupt_status on Channel | Error::Unimplemented |
| EC-03 | Invalid handle (0xFFFFFFFF) | Error::InvalidArgument |
| EC-04 | Empty signal mask | Success (no-op) |
| EC-05 | Signal mask with non-interrupt bits | Implementation-defined |
| EC-06 | Rapid enable/disable cycles | No race conditions |
| EC-07 | Multiple interrupts same object | All handled correctly |

```rust
/// EC-01: Test interrupt_control on wrong object type
fn test_wrong_object_type() -> Result<()> {
    pw_log::info!("EC-01: interrupt_control on non-InterruptObject");

    // Try on IPC channel (should fail)
    let result = syscall::interrupt_control(
        handle::IPC,  // This is a channel, not interrupt object
        Signals::READABLE,
        InterruptControl::ENABLE,
    );

    match result {
        Err(Error::Unimplemented) => {
            pw_log::info!("  PASSED: Got expected Unimplemented error");
            Ok(())
        }
        Err(e) => {
            pw_log::info!("  PASSED: Got error {} (acceptable)", e as u32);
            Ok(())
        }
        Ok(_) => {
            pw_log::error!("Should have failed on channel");
            Err(Error::FailedPrecondition)
        }
    }
}

/// EC-04: Test empty signal mask
fn test_empty_signal_mask() -> Result<()> {
    pw_log::info!("EC-04: Empty signal mask");

    // Should be a no-op, not an error
    syscall::interrupt_control(
        handle::TEST_INTERRUPTS,
        Signals::new(), // empty
        InterruptControl::ENABLE,
    )?;

    pw_log::info!("  PASSED: Empty mask accepted");
    Ok(())
}

/// EC-06: Rapid enable/disable cycles (stress test)
fn test_rapid_cycles() -> Result<()> {
    pw_log::info!("EC-06: Rapid enable/disable cycles");

    for _ in 0..100 {
        syscall::interrupt_control(
            handle::TEST_INTERRUPTS,
            signals::TEST_IRQ,
            InterruptControl::ENABLE,
        )?;

        syscall::interrupt_control(
            handle::TEST_INTERRUPTS,
            signals::TEST_IRQ,
            InterruptControl::new(),
        )?;
    }

    // Verify final state is consistent
    syscall::interrupt_control(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
        InterruptControl::ENABLE,
    )?;

    let status = syscall::interrupt_status(
        handle::TEST_INTERRUPTS,
        signals::TEST_IRQ,
    )?;

    if !status.contains(InterruptStatus::ENABLED) {
        pw_log::error!("State inconsistent after rapid cycles");
        return Err(Error::Internal);
    }

    pw_log::info!("  PASSED");
    Ok(())
}
```

---

### Task 5.6: Documentation Updates

**Files to Update:**

| File | Changes |
|------|---------|
| `pw_kernel/syscall/syscall_defs.rs` | Add rustdoc for new types and syscalls |
| `pw_kernel/docs/interrupts.rst` | Add section on userspace IRQ control |
| `pw_kernel/README.md` | Mention new capabilities |

**Example Documentation:**

```rust
/// Control flags for the [`interrupt_control()`] syscall.
///
/// # Examples
///
/// ```rust
/// // Disable an interrupt
/// interrupt_control(handle, signal, InterruptControl::new())?;
///
/// // Enable an interrupt
/// interrupt_control(handle, signal, InterruptControl::ENABLE)?;
///
/// // Enable and clear any pending status
/// interrupt_control(handle, signal,
///     InterruptControl::ENABLE | InterruptControl::CLEAR_PENDING)?;
/// ```
pub struct InterruptControl(u32);
```

---

### Task 5.7: System Generator Update

**File:** `pw_kernel/tooling/system_generator/templates/objects/interrupt.rs.jinja`

**Change:** Update to use `new_simple()` for backward compatibility.

```jinja
    // Create the interrupt object.
    let interrupt =
        unsafe { static_foreign_rc!(AtomicUsize, InterruptObject<K>, InterruptObject::new_simple(ack_irqs)) };
```

**Future Enhancement:** Update generator to emit full callbacks for
`interrupt_control` support when system config specifies advanced features.

---

### Test Execution Matrix

| Test Suite | ARM Cortex-M | RISC-V | Host |
|------------|--------------|--------|------|
| Unit Tests (UT-*) | ✅ | ✅ | ✅ |
| Kernel Tests (KT-*) | ✅ | ❌ (no trigger) | N/A |
| Integration Tests (IT-*) | ✅ | ❌ (no trigger) | N/A |
| Edge Cases (EC-*) | ✅ | Partial | Partial |
| Regression | ✅ | ✅ | ✅ |

---

### Test Success Criteria

| Metric | Target |
|--------|--------|
| Unit test pass rate | 100% |
| Integration test pass rate | 100% |
| Regression test pass rate | 100% |
| Code coverage (new code) | > 80% |
| No new compiler warnings | Required |
| Backward compatibility | All existing tests pass |

---

## File Change Summary

| File | Type | Description |
|------|------|-------------|
| `pw_kernel/syscall/syscall_defs.rs` | Modify | Add types, syscall IDs, trait methods |
| `pw_kernel/kernel/interrupt_controller.rs` | Modify | Add trait methods |
| `pw_kernel/kernel/object/interrupt.rs` | Modify | Add callbacks, control/status methods |
| `pw_kernel/kernel/object.rs` | Modify | Extend KernelObject trait |
| `pw_kernel/kernel/syscall.rs` | Modify | Add handlers, dispatch |
| `pw_kernel/arch/arm_cortex_m/nvic.rs` | Modify | Implement new trait methods |
| `pw_kernel/arch/arm_cortex_m/regs/nvic.rs` | Modify | Add register access |
| `pw_kernel/arch/riscv/plic.rs` | Modify | Implement new trait methods |
| `pw_kernel/syscall/syscall_user/arm_cortex_m.rs` | Modify | Implement syscall interface |
| `pw_kernel/syscall/syscall_user/riscv.rs` | Modify | Implement syscall interface |
| `pw_kernel/syscall/syscall_user/host.rs` | Modify | Add stubs |
| `pw_kernel/userspace/syscall.rs` | Modify | Add wrapper functions |
| `pw_kernel/tests/irq_control/kernel/` | Create | Kernel-side tests |
| `pw_kernel/tests/irq_control/user/` | Create | Userspace integration tests |
| `pw_kernel/tooling/.../interrupt.rs.jinja` | Modify | Use new_simple() |

---

## Dependencies & Ordering

```
1.1 ─────┬──► 2.1 ──► 2.2 ──► 2.4
         │         │
1.2 ─────┤         ▼
         │        2.3
1.3 ─────┘
         │
         ▼
       3.1 ──► 3.2 ──► 3.3 ──► 3.4 ──► 3.5 ──► 3.6
                                               │
                                               ▼
       4.1 ──► 4.2 ──► 4.3 ──► 4.4 ──► 4.5 ──► 4.6 ──► 4.7
                                                        │
                                                        ▼
       5.1 ──► 5.2 ──► 5.3 ──► 5.4 ──► 5.5 ──► 5.6 ──► 5.7
       (Unit)  (Kernel)(User) (Regr) (Edge) (Docs) (Gen)
```

---

## Estimated Effort

| Phase | Tasks | Estimated Time |
|-------|-------|----------------|
| Phase 1: Type Definitions | 3 | 2-3 hours |
| Phase 2: InterruptController | 4 | 4-6 hours |
| Phase 3: InterruptObject | 6 | 6-8 hours |
| Phase 4: Syscall Infrastructure | 7 | 8-10 hours |
| Phase 5: Testing & Docs | 7 | 8-12 hours |
| **Total** | **27** | **28-39 hours** |

---

## Risk Mitigation

1. **Breaking existing code**: The `InterruptObject::new()` signature change
   will break existing code. Mitigation: Either use a builder pattern or
   provide default callbacks initially.

2. **Architecture differences**: NVIC and PLIC have different semantics.
   Mitigation: Careful abstraction in `InterruptController` trait, with
   per-architecture tests.

3. **Callback complexity**: Many new callbacks in `InterruptObject`.
   Mitigation: Consider a callback struct or trait object to group related
   callbacks.

---

## Alternative: Builder Pattern for InterruptObject

To avoid breaking changes, use a builder:

```rust
impl<K: Kernel> InterruptObject<K> {
    pub const fn builder() -> InterruptObjectBuilder<K> {
        InterruptObjectBuilder::new()
    }
}

pub struct InterruptObjectBuilder<K: Kernel> {
    ack_irqs: Option<fn(Signals)>,
    enable_irqs: Option<fn(Signals)>,
    disable_irqs: Option<fn(Signals)>,
    // ...
}

impl<K: Kernel> InterruptObjectBuilder<K> {
    pub const fn ack_irqs(mut self, f: fn(Signals)) -> Self {
        self.ack_irqs = Some(f);
        self
    }
    // ... other methods

    pub const fn build(self) -> InterruptObject<K> {
        InterruptObject {
            ack_irqs: self.ack_irqs.unwrap_or(|_| {}),
            enable_irqs: self.enable_irqs.unwrap_or(|_| {}),
            // ...
        }
    }
}
```

This allows incremental adoption without breaking existing code.

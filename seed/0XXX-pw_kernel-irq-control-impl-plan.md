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

## Phase 5: Testing & Documentation

### Task 5.1: Unit tests for new types

**File:** `pw_kernel/syscall/syscall_defs.rs` (or separate test file)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interrupt_control_bits() {
        let ctrl = InterruptControl::ENABLE | InterruptControl::CLEAR_PENDING;
        assert!(ctrl.contains(InterruptControl::ENABLE));
        assert!(ctrl.contains(InterruptControl::CLEAR_PENDING));
        assert_eq!(ctrl.bits(), 0b0011);
    }

    #[test]
    fn test_interrupt_status_bits() {
        let status = InterruptStatus::ENABLED | InterruptStatus::PENDING;
        assert!(status.contains(InterruptStatus::ENABLED));
        assert!(status.contains(InterruptStatus::PENDING));
        assert!(!status.contains(InterruptStatus::NOTIFIED));
    }
}
```

### Task 5.2: Integration test

**File:** `pw_kernel/tests/irq_control/` (new test directory)

Create a test that:
1. Creates an interrupt object
2. Verifies interrupt starts disabled (if policy adopted)
3. Enables interrupt via `interrupt_control()`
4. Verifies status shows enabled
5. Triggers interrupt
6. Waits for signal
7. Checks pending status
8. Disables and clears pending

### Task 5.3: Update existing interrupt tests

**File:** `pw_kernel/tests/uart/` and other interrupt-using tests

Ensure existing tests still work with extended `InterruptObject` constructor.

### Task 5.4: Update documentation

**Files:**
- `pw_kernel/syscall/syscall_defs.rs`: Update module docs
- `pw_kernel/docs/api.rst`: Add new syscall documentation
- `pw_kernel/docs/interrupts.rst`: Update interrupt handling docs

### Task 5.5: Update system generator

**Files:** (location TBD based on code generator implementation)

Update the system generator to emit the additional callbacks when generating
interrupt object initialization code.

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
| `pw_kernel/tests/irq_control/` | Create | New integration test |

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
                                        5.1 ──► 5.2 ──► 5.3 ──► 5.4 ──► 5.5
```

---

## Estimated Effort

| Phase | Tasks | Estimated Time |
|-------|-------|----------------|
| Phase 1: Type Definitions | 3 | 2-3 hours |
| Phase 2: InterruptController | 4 | 4-6 hours |
| Phase 3: InterruptObject | 6 | 6-8 hours |
| Phase 4: Syscall Infrastructure | 7 | 8-10 hours |
| Phase 5: Testing & Docs | 5 | 6-8 hours |
| **Total** | **25** | **26-35 hours** |

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

// Copyright 2025 The Pigweed Authors
//
// Licensed under the Apache License, Version 2.0 (the "License"); you may not
// use this file except in compliance with the License. You may obtain a copy of
// the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS, WITHOUT
// WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied. See the
// License for the specific language governing permissions and limitations under
// the License.

use pw_status::Result;
use syscall_defs::{InterruptControl, InterruptStatus};
use time::Instant;

use crate::Kernel;
use crate::object::{KernelObject, ObjectBase, Signals};

/// Callbacks for interrupt control operations.
///
/// These are provided at construction time and map signal masks to
/// hardware-specific interrupt control operations.
pub struct InterruptCallbacks {
    /// Acknowledge interrupts (clear signals + re-enable).
    pub ack_irqs: fn(Signals),
    /// Enable interrupts.
    pub enable_irqs: fn(Signals),
    /// Disable interrupts.
    pub disable_irqs: fn(Signals),
    /// Clear pending interrupt status.
    pub clear_pending_irqs: fn(Signals),
    /// Get interrupt status (enabled/pending).
    pub get_status_irqs: fn(Signals) -> InterruptStatus,
}

/// Object for handling userspace interrupts.
pub struct InterruptObject<K: Kernel> {
    base: ObjectBase<K>,
    callbacks: InterruptCallbacks,
}

impl<K: Kernel> InterruptObject<K> {
    /// Create a new InterruptObject with full control callbacks.
    #[must_use]
    pub const fn new(callbacks: InterruptCallbacks) -> Self {
        Self {
            base: ObjectBase::new(),
            callbacks,
        }
    }

    /// Create a new InterruptObject with only the ack callback (backward compatible).
    ///
    /// The other callbacks will be no-ops.
    #[must_use]
    pub const fn new_simple(ack_irqs: fn(Signals)) -> Self {
        Self {
            base: ObjectBase::new(),
            callbacks: InterruptCallbacks {
                ack_irqs,
                enable_irqs: |_| {},
                disable_irqs: |_| {},
                clear_pending_irqs: |_| {},
                get_status_irqs: |_| InterruptStatus::new(),
            },
        }
    }

    pub fn interrupt(&self, kernel: K, signal_mask: Signals) {
        self.base.signal(kernel, signal_mask);
    }
}

impl<K: Kernel> KernelObject<K> for InterruptObject<K> {
    fn object_wait(
        &self,
        kernel: K,
        signal_mask: Signals,
        deadline: Instant<K::Clock>,
    ) -> Result<Signals> {
        self.base.wait_until(kernel, signal_mask, deadline)
    }

    fn interrupt_ack(&self, kernel: K, signal_mask: Signals) -> Result<()> {
        // Clear the signaled interrupts.
        self.base.state.lock(kernel).active_signals -= signal_mask;
        (self.callbacks.ack_irqs)(signal_mask);
        Ok(())
    }

    fn interrupt_control(
        &self,
        _kernel: K,
        signal_mask: Signals,
        control: InterruptControl,
    ) -> Result<()> {
        // Clear pending if requested
        if control.contains(InterruptControl::CLEAR_PENDING) {
            (self.callbacks.clear_pending_irqs)(signal_mask);
        }

        // Enable or disable based on ENABLE flag
        if control.contains(InterruptControl::ENABLE) {
            (self.callbacks.enable_irqs)(signal_mask);
        } else {
            (self.callbacks.disable_irqs)(signal_mask);
        }

        Ok(())
    }

    fn interrupt_status(&self, kernel: K, signal_mask: Signals) -> Result<InterruptStatus> {
        // Get hardware status via callback
        let mut status = (self.callbacks.get_status_irqs)(signal_mask);

        // Check if notification is pending in object state
        let active = self.base.state.lock(kernel).active_signals;
        if !(active & signal_mask).is_empty() {
            status |= InterruptStatus::NOTIFIED;
        }

        Ok(status)
    }
}

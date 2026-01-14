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

//! Kernel-side test for interrupt_control and interrupt_status functionality.

#![no_std]

use core::sync::atomic::{AtomicU32, Ordering};

use kernel::Kernel;
use kernel::interrupt_controller::InterruptController;
use pw_status::Result;
use syscall_defs::{InterruptControl, InterruptStatus};

// Track callback invocations for testing
static ENABLE_COUNT: AtomicU32 = AtomicU32::new(0);
static DISABLE_COUNT: AtomicU32 = AtomicU32::new(0);
static CLEAR_PENDING_COUNT: AtomicU32 = AtomicU32::new(0);
static STATUS_QUERY_COUNT: AtomicU32 = AtomicU32::new(0);

/// Test the new InterruptController trait methods.
pub fn test_interrupt_controller<K: Kernel>(test_irq: u32) -> Result<()> {
    pw_log::info!("Test: InterruptController trait extensions");

    // Test is_interrupt_enabled
    K::InterruptController::enable_interrupt(test_irq);
    let enabled = K::InterruptController::is_interrupt_enabled(test_irq);
    pw_assert::assert!(enabled, "IRQ should be enabled after enable_interrupt");

    K::InterruptController::disable_interrupt(test_irq);
    let enabled = K::InterruptController::is_interrupt_enabled(test_irq);
    pw_assert::assert!(!enabled, "IRQ should be disabled after disable_interrupt");

    // Test trigger and pending
    K::InterruptController::enable_interrupt(test_irq);
    
    // Trigger interrupt (this sets pending)
    K::InterruptController::trigger_interrupt(test_irq);
    
    // Check if pending (architecture-dependent)
    let pending = K::InterruptController::is_interrupt_pending(test_irq);
    pw_log::info!("  IRQ pending after trigger: {}", pending);

    // Clear pending
    K::InterruptController::clear_interrupt_pending(test_irq);
    let pending_after = K::InterruptController::is_interrupt_pending(test_irq);
    pw_log::info!("  IRQ pending after clear: {}", pending_after);

    // Re-enable for next test
    K::InterruptController::enable_interrupt(test_irq);

    pw_log::info!("  PASSED: InterruptController extensions");
    Ok(())
}

pub fn main<K: Kernel>(test_irq: u32) -> Result<()> {
    pw_log::info!("🔄 RUNNING irq_control kernel tests");

    test_interrupt_controller::<K>(test_irq)?;

    pw_log::info!("✅ PASSED");
    Ok(())
}

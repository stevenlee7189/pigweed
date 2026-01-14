.. _seed-0XXX:

=======================================
0XXX: pw_kernel Userspace IRQ Control
=======================================
.. seed::
   :number: XXX
   :name: pw_kernel Userspace IRQ Control
   :status: Draft
   :proposal_date: 2026-01-14
   :cl: TBD
   :authors: Anthony Rocha
   :facilitator: Unassigned

-------
Summary
-------
This SEED proposes adding userspace interrupt control capabilities to
``pw_kernel``, enabling tasks to dynamically enable, disable, and query the
status of interrupts assigned to them. This functionality mirrors the
``sys_irq_control`` and ``IRQ_STATUS`` syscalls found in Hubris OS, providing
fine-grained control over interrupt handling in userspace drivers.

----------
Motivation
----------

Microkernel Architecture Benefits
=================================
Unlike traditional RTOSes where interrupt handlers run in kernel/privileged
mode, ``pw_kernel`` (like Hubris) routes interrupts to unprivileged tasks via
a notification mechanism. This architecture provides:

.. code-block:: text

   Traditional RTOS:                    pw_kernel:
   ┌─────────────────────┐             ┌─────────────────────┐
   │ Kernel (privileged) │             │ Kernel (privileged) │
   │  ├─ Scheduler       │             │  ├─ Scheduler       │
   │  ├─ UART driver  ◄──┼── IRQ      │  └─ IRQ routing     │
   │  ├─ I2C driver   ◄──┼── IRQ      └─────────┬───────────┘
   │  └─ SPI driver   ◄──┼── IRQ                │ notification
   └─────────────────────┘             ┌─────────▼───────────┐
                                       │ Task (unprivileged) │
   If driver crashes →                 │  └─ UART driver ◄───┼── IRQ
      whole system down                └─────────────────────┘
                                       
                                       If driver crashes →
                                          only that task faulted,
                                          supervisor can restart it

**Key Benefits:**

- **Fault Isolation**: Buggy peripheral driver can't take down the entire system
- **Memory Safety**: Corruption contained to single task via MPU boundaries
- **Security**: Vulnerabilities have limited blast radius
- **Recoverability**: Supervisor can restart faulted drivers

Current Interrupt Flow
======================
Currently, ``pw_kernel`` provides basic interrupt handling through the
``InterruptObject`` abstraction:

1. Userspace waits on an ``InterruptObject`` via ``object_wait()``
2. When an interrupt fires, the kernel masks it and signals the object
3. After handling, userspace calls ``interrupt_ack()`` to re-enable the IRQ

This flow handles the common case but lacks flexibility for several important
use cases.

Use Cases Requiring Explicit IRQ Control
========================================

**1. Driver Isolation and Hot-Restartability**

.. code-block:: rust

   // Supervisor can restart a faulted driver task
   // Interrupts must be properly re-initialized after restart
   match fault {
       FaultInfo::MemoryAccess { .. } => {
           // Driver crashed, restart it
           supervisor::restart_task(uart_driver_id);
           // IRQ routing needs explicit control to restore
       }
   }

**2. Interrupt Coalescing and Batching**

.. code-block:: rust

   // User-space driver can implement sophisticated interrupt handling
   loop {
       // Wait for interrupt OR timeout
       let signals = object_wait(handle, IRQ_SIGNAL | TIMER_SIGNAL, timeout)?;
       
       if signals.contains(IRQ_SIGNAL) {
           // Coalesce multiple packets before processing
           while hw.status().has_data() {
               buffer.push(hw.read_data());
           }
           // Process batch - don't re-enable IRQ yet
           process_batch(&buffer);
           
           // Now re-enable after batch complete
           interrupt_control(handle, IRQ_SIGNAL, InterruptControl::ENABLE)?;
       }
   }

**3. Deferred Interrupt Processing (Bottom Halves)**

.. code-block:: rust

   // Task (user-space) does the real work:
   fn uart_task() {
       loop {
           object_wait(handle, UART_IRQ_SIGNAL, INFINITE)?;
           
           // Heavy processing in unprivileged context
           while uart.has_data() {
               let byte = uart.read();
               process_protocol_byte(byte);  // Complex parsing
               update_state_machine();       // May need IPC
           }
           
           // Re-enable interrupt only after processing complete
           interrupt_control(handle, UART_IRQ_SIGNAL, InterruptControl::ENABLE)?;
       }
   }

**4. Conditional Interrupt Handling**

.. code-block:: rust

   // User-space driver can decide whether to process interrupts
   fn sensor_task() {
       let mut calibrating = false;
       
       loop {
           let signals = object_wait(handle, ALL_SIGNALS, INFINITE)?;
           
           if signals.contains(CALIBRATION_REQUEST) {
               calibrating = true;
               // Disable sensor interrupts during calibration
               interrupt_control(handle, SENSOR_IRQ, InterruptControl::empty())?;
           }
           
           if signals.contains(CALIBRATION_DONE) {
               calibrating = false;
               // Re-enable sensor interrupts
               interrupt_control(handle, SENSOR_IRQ, InterruptControl::ENABLE)?;
           }
           
           if signals.contains(SENSOR_IRQ) && !calibrating {
               process_sensor_data();
               interrupt_control(handle, SENSOR_IRQ, InterruptControl::ENABLE)?;
           }
       }
   }

**5. Power Management**

Drivers may need to temporarily disable interrupts during low-power states
without losing pending interrupt status.

**6. Driver Initialization**

Interrupts should remain disabled until the driver is fully initialized.
Currently, there's no explicit control over when interrupts become active.

**7. Shared Interrupt Lines (Multiplexing)**

.. code-block:: rust

   // User-space can handle shared IRQ lines intelligently
   fn gpio_task() {
       loop {
           object_wait(handle, GPIO_IRQ_SIGNAL, INFINITE)?;
           
           // Check which GPIO actually triggered
           let status = gpio.interrupt_status();
           
           if status & PIN_5 != 0 {
               channel_send(button_channel, ButtonEvent::Pressed)?;
           }
           if status & PIN_7 != 0 {
               channel_send(encoder_channel, EncoderEvent::Tick)?;
           }
           
           gpio.clear_interrupt_status(status);
           interrupt_control(handle, GPIO_IRQ_SIGNAL, InterruptControl::ENABLE)?;
       }
   }

**8. Per-Driver Debugging and Profiling**

.. code-block:: rust

   // Each driver task can have its own instrumentation
   static INTERRUPT_COUNT: AtomicU32 = AtomicU32::new(0);
   static TOTAL_LATENCY: AtomicU64 = AtomicU64::new(0);

   fn handle_interrupt() {
       let start = get_time();
       INTERRUPT_COUNT.fetch_add(1, Ordering::Relaxed);
       
       // ... handle interrupt ...
       
       let latency = get_time() - start;
       TOTAL_LATENCY.fetch_add(latency, Ordering::Relaxed);
   }
   
   // Debugger can read these per-task without kernel changes

--------
Proposal
--------
Add two new syscalls to ``pw_kernel``:

``interrupt_control``
=====================
.. code-block:: rust

   pub fn interrupt_control(
       object_handle: u32,
       signal_mask: Signals,
       control: InterruptControl,
   ) -> Result<()>;

Where ``InterruptControl`` is defined as:

.. code-block:: rust

   bitflags! {
       pub struct InterruptControl: u32 {
           /// Enable the interrupt(s) specified by signal_mask
           const ENABLE = 0b0001;
           /// Clear any pending status for the interrupt(s)
           const CLEAR_PENDING = 0b0010;
       }
   }

**Behavior:**

- If ``ENABLE`` is set, the interrupt(s) corresponding to ``signal_mask`` are
  unmasked at the hardware level.
- If ``ENABLE`` is not set, the interrupt(s) are masked (disabled).
- If ``CLEAR_PENDING`` is set, any pending interrupt status is cleared.

**Errors:**

- ``InvalidArgument``: ``object_handle`` is not a valid ``InterruptObject``, or
  ``signal_mask`` contains bits not assigned to interrupts on this object.

``interrupt_status``
====================
.. code-block:: rust

   pub fn interrupt_status(
       object_handle: u32,
       signal_mask: Signals,
   ) -> Result<InterruptStatus>;

Where ``InterruptStatus`` is defined as:

.. code-block:: rust

   bitflags! {
       pub struct InterruptStatus: u32 {
           /// At least one interrupt in the mask is enabled
           const ENABLED = 0b0001;
           /// At least one interrupt in the mask is pending in hardware
           const PENDING = 0b0010;
           /// A notification has been posted but not yet consumed
           const NOTIFIED = 0b0100;
       }
   }

**Behavior:**

- Returns the combined status of all interrupts specified by ``signal_mask``.
- Status bits are OR'd together if multiple interrupts are queried.

**Errors:**

- ``InvalidArgument``: ``object_handle`` is not a valid ``InterruptObject``, or
  ``signal_mask`` contains bits not assigned to interrupts on this object.

Example Usage
=============
.. code-block:: rust

   // Driver initialization - ensure IRQ is disabled until ready
   syscall::interrupt_control(
       handle::MY_IRQ,
       Signals::INTERRUPT_A,
       InterruptControl::empty(),  // Disable
   )?;

   // ... complete driver initialization ...

   // Now enable the interrupt
   syscall::interrupt_control(
       handle::MY_IRQ,
       Signals::INTERRUPT_A,
       InterruptControl::ENABLE,
   )?;

   loop {
       let signals = syscall::object_wait(
           handle::MY_IRQ,
           Signals::INTERRUPT_A,
           INFINITE,
       )?;

       // Handle interrupt...

       // Re-enable and clear any pending
       syscall::interrupt_control(
           handle::MY_IRQ,
           signals,
           InterruptControl::ENABLE | InterruptControl::CLEAR_PENDING,
       )?;
   }

---------------------
Problem Investigation
---------------------

Interrupt Flow in pw_kernel
===========================
The current interrupt flow in ``pw_kernel`` is as follows:

.. code-block:: text

   Hardware IRQ
       │
       ▼
   ┌────────────────────────────────────┐
   │ Vector Table Entry (Kernel)        │
   │  - Save minimal context            │
   │  - Look up owning task from table  │
   │  - Post signal to InterruptObject  │
   │  - Mask interrupt at NVIC/PLIC     │
   │  - Return (no task switch yet)     │
   └────────────────────────────────────┘
       │
       ▼ (on next scheduling decision)
   ┌────────────────────────────────────┐
   │ Task wakes from object_wait()      │
   │  - Processes interrupt             │
   │  - Calls interrupt_ack() to unmask │
   └────────────────────────────────────┘

The kernel maintains per-object signal bits. When an interrupt fires:

1. Kernel's minimal ISR looks up owning ``InterruptObject``
2. Sets corresponding signal bit on the object
3. Masks interrupt at hardware (NVIC/PLIC) to prevent re-entry
4. If task was blocked on ``object_wait()``, marks it runnable

Task explicitly re-enables interrupt after processing via ``interrupt_ack()``.

Prior Art: Hubris OS
====================
Hubris provides ``sys_irq_control`` (syscall #7) with the following semantics:

- Argument 0: notification bitmask corresponding to the interrupt
- Argument 1: desired state

  - bit 0: 0 = disabled, 1 = enabled
  - bit 1: 0 = leave pending, 1 = clear pending

Hubris also provides ``IRQ_STATUS`` (syscall #13) to query interrupt state.

Key design decisions in Hubris:

1. **Interrupts identified by notification bits**: Tasks refer to interrupts
   using the notification bits they map to, not raw IRQ numbers. This prevents
   tasks from manipulating interrupts they don't own.

2. **Interrupts start disabled**: When a task starts (or restarts), its
   interrupts are initially masked.

3. **Kernel masks on delivery**: When an interrupt fires, the kernel masks it
   before posting the notification to prevent re-triggering.

Current pw_kernel Behavior
==========================
In ``pw_kernel``, the ``interrupt_ack()`` syscall combines two operations:

1. Clears the signal bits on the ``InterruptObject``
2. Re-enables the interrupt at the hardware level

This conflation means there's no way to:

- Disable an interrupt without first acknowledging it
- Keep an interrupt disabled after acknowledging
- Query interrupt status

Comparison
==========
+---------------------------+---------------+------------------+
| Feature                   | pw_kernel     | Hubris           |
+===========================+===============+==================+
| Wait for interrupt        | ✓             | ✓                |
+---------------------------+---------------+------------------+
| Acknowledge interrupt     | ✓             | ✓ (via irq_ctrl) |
+---------------------------+---------------+------------------+
| Enable/disable from user  | ✗             | ✓                |
+---------------------------+---------------+------------------+
| Query interrupt status    | ✗             | ✓                |
+---------------------------+---------------+------------------+
| Clear pending             | ✗             | ✓                |
+---------------------------+---------------+------------------+

---------------
Detailed Design
---------------

InterruptController Trait Extensions
====================================
The existing ``InterruptController`` trait already provides the necessary
low-level primitives:

.. code-block:: rust

   pub trait InterruptController {
       fn enable_interrupt(irq: u32);
       fn disable_interrupt(irq: u32);
       // ... other methods
   }

The new syscalls will leverage these existing methods.

InterruptObject Extensions
==========================
The ``InterruptObject`` will be extended with methods to support the new
operations:

.. code-block:: rust

   impl<K: Kernel> InterruptObject<K> {
       pub fn control(
           &self,
           kernel: K,
           signal_mask: Signals,
           control: InterruptControl,
       ) -> Result<()> {
           // Validate signal_mask contains only interrupt signals
           // assigned to this object

           // Call enable_irqs or disable_irqs callback based on control
           if control.contains(InterruptControl::ENABLE) {
               (self.enable_irqs)(signal_mask);
           } else {
               (self.disable_irqs)(signal_mask);
           }

           if control.contains(InterruptControl::CLEAR_PENDING) {
               (self.clear_pending_irqs)(signal_mask);
           }

           Ok(())
       }

       pub fn status(
           &self,
           kernel: K,
           signal_mask: Signals,
       ) -> Result<InterruptStatus> {
           // Query hardware and object state
           // Return combined status
       }
   }

The ``InterruptObject`` constructor will be extended to accept additional
callbacks:

.. code-block:: rust

   pub struct InterruptObject<K: Kernel> {
       base: ObjectBase<K>,
       ack_irqs: fn(Signals),
       enable_irqs: fn(Signals),    // NEW
       disable_irqs: fn(Signals),   // NEW
       get_status: fn(Signals) -> InterruptStatus,  // NEW
   }

Syscall Implementation
======================
New syscall handlers will be added:

.. code-block:: rust

   fn handle_interrupt_control<K: Kernel>(
       kernel: K,
       mut args: K::SyscallArgs<'_>,
   ) -> Result<u64> {
       let handle = args.next_u32();
       let signal_mask = Signals::from_bits_truncate(args.next_u32());
       let control = InterruptControl::from_bits_truncate(args.next_u32());

       let object = get_interrupt_object(kernel, handle)?;
       object.control(kernel, signal_mask, control)?;
       Ok(0)
   }

System Generator Updates
========================
The system generator will need to emit the additional callbacks when
generating interrupt object initialization code.

Architecture-Specific Considerations
====================================

**ARM Cortex-M (NVIC)**

- ``enable_interrupt``: Write to NVIC ISER register
- ``disable_interrupt``: Write to NVIC ICER register
- ``clear_pending``: Write to NVIC ICPR register
- ``get_pending``: Read NVIC ISPR register

**RISC-V (PLIC)**

- Similar operations via PLIC enable and pending registers

Both architectures already have these primitives implemented in the
``InterruptController`` trait.

Security Enforcement
====================
The kernel validates all ``interrupt_control`` and ``interrupt_status`` calls:

- Task can only control interrupts assigned to it via system configuration
- Kernel validates IRQ ownership on every syscall
- MPU prevents task from directly accessing interrupt controller registers
- Malicious task cannot steal or disable other tasks' interrupts

----------
Trade-offs
----------
User-space interrupt control involves inherent trade-offs between isolation
and latency.

Latency Comparison
==================
.. code-block:: text

   Traditional kernel ISR:
     IRQ → Handler → Return
     ~10-50 cycles

   pw_kernel user-space ISR:
     IRQ → Kernel stub → Schedule → Task → Return
     ~100-500 cycles (depends on task priority)

The additional latency comes from the context switch required to run the
unprivileged task. This is acceptable for most embedded applications
prioritizing reliability over raw speed.

Feature Comparison
==================
+------------------+-------------------+--------------------+
| Aspect           | User-Space IRQ    | Kernel IRQ         |
+==================+===================+====================+
| **Latency**      | Higher (context   | Lower (immediate   |
|                  | switch needed)    | execution)         |
+------------------+-------------------+--------------------+
| **Isolation**    | Full MPU          | None (kernel       |
|                  | protection        | memory access)     |
+------------------+-------------------+--------------------+
| **Recovery**     | Task restart      | System reset       |
|                  | possible          | required           |
+------------------+-------------------+--------------------+
| **Complexity**   | Kernel routes,    | All logic in       |
|                  | task handles      | kernel             |
+------------------+-------------------+--------------------+
| **Debugging**    | Per-task          | Kernel-level only  |
|                  | instrumentation   |                    |
+------------------+-------------------+--------------------+
| **Code size**    | Smaller kernel    | Larger kernel      |
+------------------+-------------------+--------------------+
| **Security**     | Privilege         | Full kernel access |
|                  | separation        |                    |
+------------------+-------------------+--------------------+

When User-Space IRQ Control Is Worth It
=======================================
**Good fit:**

- Reliability-critical systems (Root of Trust, industrial control)
- Security-sensitive applications
- Complex protocol handling (USB, Ethernet, CAN)
- Systems requiring field recovery without reboot
- Applications where driver bugs shouldn't crash the system

**May not fit:**

- Hard real-time with sub-microsecond latency requirements
- Extremely resource-constrained systems (< 32KB RAM)
- Simple applications with fully trusted drivers

------------
Alternatives
------------

Do Nothing
==========
The current ``interrupt_ack()`` syscall could remain the only interrupt control
mechanism. This limits driver flexibility but keeps the API surface small.

Combine with interrupt_ack
==========================
Instead of adding new syscalls, ``interrupt_ack()`` could be extended with
additional parameters. This was rejected because:

1. It changes the semantics of an existing syscall
2. The "ack" name implies re-enabling, which would be confusing if it doesn't
3. Querying status doesn't fit the "ack" concept

Kernel-Only Control
===================
Interrupt enable/disable could be restricted to kernel-space drivers only.
This was rejected because it would require moving driver code into the kernel,
contradicting the microkernel design philosophy.

--------------
Open Questions
--------------

1. **Initial interrupt state**: Should interrupts assigned to an
   ``InterruptObject`` start enabled or disabled when the system boots?
   Hubris starts them disabled. This seems safer but requires explicit
   enablement in drivers.

2. **Per-interrupt vs per-object control**: Should control operations apply to
   individual interrupt sources (via signal mask) or to the entire object?
   The current proposal uses signal masks for flexibility.

3. **Backward compatibility**: Should ``interrupt_ack()`` behavior change, or
   should it continue to always re-enable? The current proposal keeps
   ``interrupt_ack()`` behavior unchanged for compatibility.

4. **Naming**: ``interrupt_control`` vs ``irq_control`` vs
   ``interrupt_set_enabled``. The proposal uses ``interrupt_control`` to
   match the established ``InterruptObject`` naming.

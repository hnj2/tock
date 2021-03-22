System Calls
========================================

**TRD:** <br/>
**Working Group:** Kernel<br/>
**Type:** Documentary<br/>
**Status:** Draft <br/>
**Author:** Guillaume Endignoux, Jon Flatley, Philip Levis, Amit Levy, Leon Schuermann, Johnathan Van Why <br/>
**Draft-Created:** August 31, 2020<br/>
**Draft-Modified:** Nov 1, 2020<br/>
**Draft-Version:** 2<br/>
**Draft-Discuss:** tock-dev@googlegroups.com</br>

Abstract
-------------------------------

This document describes the system call application binary interface (ABI)
between user space processes and the Tock kernel for 32-bit ARM Cortex-M
and RISC-V RV32I platforms.

1 Introduction
===============================

The Tock operating system can run multiple independent userspace applications.
Because these applications are untrusted, the kernel uses hardware memory
protection to isolate them from it. This allows applications written in C
(or even assembly) to safely run on Tock.

Each application image is a separate process: it has its own address space
and thread stack. Applications invoke operations on and receive callbacks
from the Tock kernel through the system call programming interface.

This document describes Tock's system call programming interface (API)
and application binary interface (ABI) for 32-bit ARM Cortex-M and RISC-V
RV32I platforms. It describes the system calls
that Tock implements, their semantics, and how a userspace process
invokes them. The ABI for other architectures, if supported, will be
described in other documents.

2 Design Considerations
===============================

Three design considerations guide the design of Tock's system call API and
ABI.

  1. Tock is currently supported on ARM CortexM and RISCV and may support
  others in the future. Its ABI must support both architectures and be
  flexible enough to support future ones.
  2. Tock userspace applications can be written in any language. The system
  call API must support their calling semantics in a safe way. Rust is
  especially important.
  3. Both the API and ABI must be efficient and support common call
  patterns in an efficient way.

2.1 Architectural Support and ABIs
--------------------------------

The primary question for the ABI is how many and which registers transfer
data between the kernel and userspace. Passing more registers has the benefit
of the kernel and userspace being able to transfer more information
without relying on pointers to memory structures. It has the cost of requiring
every system call to transfer and manipulate more registers.

2.2 Programming Language APIs
---------------------------------

Userspace support for Rust is an important requirement for Tock. A key
invariant in Rust is that a given memory object can either have multiple
references or a single mutable reference. If userspace
passes a writeable (mutable) buffer into the kernel, it must relinquish
any references to that buffer. As a result, the only way for userspace
to regain a reference to the buffer is for the kernel to pass it back.

2.3 Efficiency
---------------------------------

Programming language calling conventions are
another consideration because they affect efficiency. For
example the C calling convention in ARM says that the first four arguments
to a function are stored in r0-r3. Additional arguments are stored on
the stack. Therefore, if the system call ABI says that arguments are stored
in different registers than r0-r3, a C function call that invokes a system
call will need to move the C arguments into those registers.

3 System Call ABI
=================================

This section describes the ABI for Tock on 32-bit platforms. The ABI for
64-bit platforms is currently undefined but may be specified in a future TRD.


3.1 Registers
---------------------------------

When userspace invokes a system call, it passes 4 registers to the
kernel as arguments. It also pass an 8-bit value of which type of
system call (see Section 4) is being invoked (the Syscall Class
ID). When the system call returns, it returns 4 registers as return
values. When the kernel invokes a callback on userspace, it passes 4
registers to userspace as arguments and has no return value.

|                        | CortexM | RISC-V |
|------------------------|---------|--------|
| Syscall Arguments      | r0-r3   | a0-a3  |
| Syscall Return Values  | r0-r3   | a0-a3  |
| Syscall Class ID       | svc     | a4     |
| Callback Arguments     | r0-r3   | a0-a3  |
| Callback Return Values | None    | None   |

How registers are mapped to arguments can affect performance and code size.
For system calls implemented by capsules and drivers (`command`, `subcribe`,
and `allow`), arguments that are passed to these calls should be placed
in the same registers that will be used to invoke those calls. This allows
the system call handlers in the kernel to pass them unchanged, rather than
have to move them between registers.

For example, `command` has this signature:

```rust
fn command(&self, minor_num: usize, r2: usize, r3: usize, caller_id: AppId) -> ReturnCode
```

This means that the value which will be passed as `r2` to the command
should be placed in register r2 when userspace invokes the system
call. That way, the system call handler can just leave register r2
unchanged. If, instead, the argument `r2` were passed in register r3,
the system call handler would have to spend an instruction moving
register r3 to register r2.

Driver system call implementations in the Tock kernel typically pass a reference
to `self` as their first argument. Therefore, `r0` is usually used to dispatch
onto the correct driver; this argument is consumed by the system call handler
and replaced with `&self` when the actual system call method is invoked.

3.2 Return Values
----------------------------------

All system calls have the same return value format. A system call can
return one of several variants, having different associated value types,
which are shown here. `r0`-`r3` refer to the return value registers:
for CortexM they are `r0`-`r3` and for RISC-V they are `a0`-`a3`.

| System call return variant | `r0` | `r1`               | `r2`               | `r3`               |
|----------------------------|------|--------------------|--------------------|--------------------|
| Failure                    | 0    | Error code         | -                  | -                  |
| Failure with u32           | 1    | Error code         | Return Value 0     |                    |
| Failure with 2 u32         | 2    | Error code         | Return Value 0     | Return Value 1     |
| Failure with u64           | 3    | Error code         | Return Value 0 LSB | Return Value 0 MSB |
| Success                    | 128  |                    |                    |                    |
| Success with u32           | 129  | Return Value 0     |                    |                    |
| Success with 2 u32         | 130  | Return Value 0     | Return Value 1     |                    |
| Success with u64           | 131  | Return Value 0 LSB | Return Value 0 MSB |                    |
| Success with 3 u32         | 132  | Return Value 0     | Return Value 1     | Return Value 2     |
| Success with u32 and u64   | 133  | Return Value 0     | Return Value 1 LSB | Return Value 1 MSB |

There are a wide variety of failure and success variants because
different system calls need to pass different amounts of data. A
command that requests a 64-bit timestamp, for example, needs its
success to return a `u64`, but its failure can return nothing. In
contrast, a system call that passes a pointer into the kernel may have
a simple success return value but requires a failure with one 32-bit
value so the pointer can be passed back.

Every system call MUST return only one failure and only one success
variant. Different system calls may use different failure and success
variants, but any specific system call returns exactly one of each. If an
operation might have different success return variants or failure return
variants, then it should be split into multiple system calls.

This requirement of a single failure variant and a single success variant is to simplify
userspace implementations and preclude them from having to handle many different cases.
The presence of many difference cases suggests that the operation should be split up --
there is non-determinism in its execution or its meaning is overloaded. It also fits
well with Rust's `Result` type.

All 32-bit values not specified for `r0` in the above table are reserved.
Reserved `r0` values MAY be used by a future TRD and MUST NOT be returned by the
kernel unless specified in a TRD. Therefore, for future compatibility, userspace
code MUST tolerate `r0` values that it does not recognize.

3.3 Error Codes
---------------------------------

All system call failures return an error code. These error codes are a superset of
kernel error codes. They include all kernel error codes so errors from calls
on kernel HILs can be easily mapped to userspace system calls when suitable. There
are additional error codes to include errors related to userspace.

| Value | Error Code  | Meaning                                                                                 |
|-------|-------------|-----------------------------------------------------------------------------------------|
| 1     | FAIL        | General failure condition: no further information available.                            |
| 2     | BUSY        | The driver or kernel is busy: retry later.                                              |
| 3     | ALREADY     | This operation is already ongoing can cannot be executed more times in parallel.        |
| 4     | OFF         | This subsystem is powered off and must be turned on before issuing operations.          |
| 5     | RESERVE     | Making this call requires some form of prior reservation, which has not been performed. |
| 6     | INVALID     | One of the parameters passed to the operation was invalid.                              |
| 7     | SIZE        | The size specified is too large or too small.                                           |
| 8     | CANCEL      | The operation was actively cancelled by a call to a cancel() method or function.        |
| 9     | NOMEM       | The operation required memory that was not available (e.g. a grant region or a buffer). |
| 10    | NOSUPPORT   | The operation is not supported/implemented.                                             |
| 11    | NODEVICE    | The specified device is not implemented by the kernel.                                  |
| 12    | UNINSTALLED | The resource was removed or uninstalled (e.g., an SD card).                             |
| 13    | NOACK       | The packet transmission was sent but not acknowledged.                                  |
| 1024  | BADRVAL     | The variant of the return value did not match what the system call should return.       |

Values in the range 1-1023 reflect kernel return value error codes. Kernel error
codes not specified above are currently reserved. TRDs MAY specify reserved
kernel error codes, but MUST NOT specify kernel error codes greater than 1023.
The Tock kernel MUST NOT return an error code unless the error code is specified
in a TRD.

Values greater than 1023 are reserved for userspace library use. Value 1024
(BADRVAL) is for when a system call returns a different failure or success
variant than the userspace library expects.


4 System Call API
=================================

Tock has 7 classes or types of system calls. When a system call is invoked, the
class is encoded as the Syscall Class ID. Some system call classes are implemented
by the core kernel and so always have the same operations. Others are implemented
by peripheral syscall drivers and so the set of valid operations depends on what peripherals
the platform has and which have drivers installed in the kernel.

The 6 classes are:

| Syscall Class    | Syscall Class ID |
|------------------|------------------|
| Yield            |        0         |
| Subscribe        |        1         |
| Command          |        2         |
| Read-Write Allow |        3         |
| Read-Only Allow  |        4         |
| Memop            |        5         |
| Exit             |        6         |

All of the system call classes except Yield and Exit are non-blocking. When a userspace
process calls a Subscribe, Command, Allow, Read-Only Allow, or Memop syscall,
the kernel will not put it on a wait queue. Instead, it will return immediately
upon completion. The kernel scheduler may decide to suspend the process due to
a timeslice expiration or the kernel thread being runnable, but the system calls
themselves do not block. If an operation is long-running (e.g., I/O), its completion
is signaled by a callback (see the Subscribe call in 4.2).

Successful calls to Exit system calls do not return (the process exits).

Peripheral driver-specific system calls (Subscribe, Command, Allow, Read-Only Allow)
all include two arguments, a driver identifier and a syscall identifier. The driver identifier
specifies which peripheral system call driver to invoke. The syscall identifier (which
is different than the Syscall Class ID in the table above)
specifies which instance of that system call on that driver to invoke. Both
arguments are unsigned 32-bit integers. For example, the
Console driver has driver identifier `0x1` and a Command to the console driver with
syscall identifier `0x2` starts receiving console data into a buffer.

If userspace invokes a system call on a peripheral driver that is not installed in
the kernel, the kernel MUST return the corresponding failure result with an error
of `NOSUPPORT`.

4.1 Yield (Class ID: 0)
--------------------------------

The Yield system call class is how a userspace process handles
callbacks, relinquishes the processor to other processes, or waits for
one of its long-running calls to complete.  The Yield system call
class implements the only blocking system call in Tock, `yield-wait`.

When a process calls a Yield system call, the kernel schedules one
pending callback (if any) to execute on the userspace stack.  If there
are multiple pending callbacks, each one requires a separate Yield
call to invoke. The kernel invokes callbacks only in response to Yield
system calls.  This form of very limited preemption allows userspace
to manage concurrent access to its variables.

There are two Yield system calls:
  - `yield-wait`
  - `yield-no-wait`

The first call, `yield-wait`, blocks until a callback executes. This is the
only blocking system call in Tock. It is commonly used to provide a blocking
I/O interface to userspace. A userspace library starts a long-running operation
that has a callback, then calls `yield-wait` to wait for a callback. When the
`yield-wait` returns, the process checks if the resuming callback was the one
it was expecting, and if not calls `yield-wait` again. 

The second call, `yield-no-wait`, executes a single callback if any is pending.
If no callbacks are pending it returns immediately. 

The register arguments for Yield system calls are as follows. The registers
r0-r3 correspond to r0-r3 on CortexM and a0-a3 on RISC-V.

| Argument               | Register |
|------------------------|----------|
| Yield identifer        | r0       |
| No wait field          | r1       |
| unused                 | r2       |
| unused                 | r3       |


The yield identifier specifies which call is invoked.

| System call     | Yield identifier value |
|-----------------|------------------------|
| yield-no-wait   |                      0 |
| yield-wait      |                      1 |


All other yield identifier values are reserved. If an invalid
yield indentifier is passed the kernel returns immediately.

The no wait field is only used by `yield-no-wait`. It contains the
memory address of an 8-bit byte that `yield-no-wait` writes to
indicate whether a callback was invoked. If invoking `yield-no-wait`
resulted in a callback executing, `yield-no-wait` writes 1 to the
field address. If invoking `yield-no-wait` resulted in no callback
executing, `yield-no-wait` writes 0 to the field address. This field
allows userspace loops that want to flush the callback queue to
execute `yield-no-wait` until the queue is empty.

The Yield system call class has no return value. This is because
invoking a callback pushes that function call onto the stack, such
that the return value of a call to yield system call may be the
return value of the callback. This is why the no wait field exists,
so that `yield-no-wait` can return a result to the caller. Allowing
the kernel to pass a return value in register back to userspace
would require either re-entering the kernel or expensive
execution architectures (e.g., additonal stacks or additional
stack frames) for callbacks.

4.2 Subscribe (Class ID: 1)
--------------------------------

The Subscribe system call class is how a userspace process registers callbacks
with the kernel. Subscribe system calls are implemented by peripheral syscall
drivers, so the set of valid Subscribe calls depends on the platform and what
drivers were compiled into the kernel.

The register arguments for Subscribe system calls are as follows. The registers
r0-r3 correspond to r0-r3 on CortexM and a0-a3 on RISC-V.

| Argument               | Register |
|------------------------|----------|
| Driver identifer       | r0       |
| Subscribe identifier   | r1       |
| Callback pointer       | r2       |
| Application data       | r3       |


The `callback pointer` is the address of the first instruction of
the callback function. The `application data` argument is a parameter
that an application passes in and the kernel passes back in callbacks
unmodified.

If the passed callback is not valid (is outside process executable
memory and is not the Null Callback described below), the kernel MUST
NOT invoke the requested driver and MUST immediately return a failure
with a return code of EINVAL. The currently registered callback
remains registered and the kernel does not cancel any pending invocations
of the existing callback.

Any callback passed from a process MUST remain valid until the next successful invocation of
`subscribe` by that process with the same syscall and driver identifier. When
a process makes a successful subscribe system call (one which results
in the `Success with 2 u32` return variant), the kernel MUST cancel
all pending callbacks on that process for that driver and subscribe identifier: it
MUST NOT invoke the previous callback after the call to `subscribe`, and
MUST NOT invoke the new callback for events that the kernel handled before the
call to `subscribe`.

Note that these semantics create a period over which callbacks might
be lost: any callbacks that were pending when `subscribe` was called
will not be invoked. On one hand, losing callbacks can create strange
behavior in userspace.  On the other, ensuring correctness is
difficult. If the pending callbacks are invoked on the old function,
there is a safety/liveness issue; this means that a callback function
must exist after it has been removed, and so for safety may need to be
static (exist for the lifetime of the process). Therefore, to allow
dynamic callbacks, a callback can't be invoked after it's
unregistered.

At the same time, invoking the new callback in response to prior
events has its own correctness issues. For example, suppose that
userspace registers a callback for receiving a certain type of
event (e.g., a rising edge on a GPIO pin). It then changes the
type of event (to falling edge) and registers a new callback.
Invoking the new callback on the previous events will be
incorrect.

If userspace requires that it not lose any callbacks, it should
not re-subcribe and instead use some form of userspace dispatch.

The return variants for Subscribe system calls are `Failure with 2 u32`
and `Success with 2 u32`. For success, the first `u32` is the callback
pointer passed in the previous call to Subscribe (the existing
callback) and the second `u32` is the application data pointer passed
in the previous call to Subscribe (the existing application data). For
failure, the first `u32` is the passed callback pointer and the second
`u32` is the passed application data pointer. For the first successful
call to Subscribe for a given callback, the callback pointer and
application data pointer returned MUST be the Null Callback (described
below).

4.2.1 The Null Callback
---------------------------------

The Tock kernel defines a callback pointer as the Null Callback.
The Null Callback denotes a callback that the kernel will never invoke.
The Null Callback is used for two reasons. First, a userspace process
passing the Null Callback as the callback pointer for Subscribe
indicates that there should be no more callbacks. Second, the first
time a userspace process calls Subscribe for a particular callback,
the kernel needs to return callback and application pointers indicating
the current configuration; in this case, the kernel returns the Null
Callback. The Tock kernel MUST NOT invoke the Null Callback.

The Null Callback MUST be 0x0. This means it is not possible for userspace
to pass address 0x0 as a valid code entry point. Unlike systems with
virtual memory, where 0x0 can be reserved a special meaning, in
microcontrollers with only physical memory 0x0 is a valid memory location.
It is possible that a Tock kernel is configured so its applications
start at address 0x0. However, even if they do begin at 0x0, the
Tock Binary Format for application images mean that the first address
will not be executable code and so 0x0 will not be a valid function.
In the case that 0x0 is valid application code and where the
linker places a callback function, the first instruction of the function
should be a no-op and the address of the second instruction passed
instead.

If a userspace process invokes subscribe on a driver ID that is not
installed in the kernel, the kernel MUST return a failure with an
error code of NOSUPPORT and a callback of the Null Callback.

4.3 Command (Class ID: 2)
---------------------------------

The Command system call class is how a userspace process calls a function
in the kernel, either to return an immediate result or start a long-running
operation. Command system calls are implemented by peripheral syscall drivers,
so the set of valid Command calls depends on the platform and what drivers were
compiled into the kernel.

The register arguments for Command system calls are as follows. The registers
r0-r3 correspond to r0-r3 on CortexM and a0-a3 on RISC-V.

| Argument               | Register |
|------------------------|----------|
| Driver identifer       | r0       |
| Command identifier     | r1       |
| Argument 0             | r2       |
| Argument 1             | r3       |

Argument 0 and argument 1 are unsigned 32-bit integers. Command calls should
never pass pointers: those are passed with Allow calls, as they can adjust
memory protection to allow the kernel to access them.

The return variants of Command are instance-specific. Each specific
Command instance (combination of major and minor identifier) specifies
its failure variant and success variant. If userspace invokes a
command on a peripheral that is not installed, the kernel returns a
failure variant of `Failure`, with an associated error code of
`NOSUPPORT`.

4.3.1 Command Identifier 0
--------------------------------

Every device driver MUST implement command identifier 0 as the
"exists" command.  This command always returns `Success`. This command
allows userspace to determine if a particular system call driver is
installed; if it is, the command returns `Success`. If it is not, the
kernel returns `Failure` with an error code of `NOSUPPORT`.

4.4 Read-Write Allow (Class ID: 3)
---------------------------------

The Read-Write Allow system call class is how a userspace process
shares a read-write buffer with the kernel. When userspace shares a
buffer, it can no longer access it. Calling a Read-Write Allow system
call also returns a buffer (address and length).  On the first call to
a Read-Write Allow system call, the kernel returns a zero-length
buffer. Subsequent calls to Read-Write Allow return the previous
buffer passed. Therefore, to regain access to the buffer, the process
must call the same Read-Write Allow system call again.

The register arguments for Read-Write Allow system calls are as
follows. The registers r0-r3 correspond to r0-r3 on CortexM and a0-a3
on RISC-V.

| Argument               | Register |
|------------------------|----------|
| Driver identifer       | r0       |
| Buffer identifier      | r1       |
| Address                | r2       |
| Size                   | r3       |

The return variants for Read-Write Allow system calls are `Failure
with 2 u32` and `Success with 2 u32`.  In both cases, `Argument 0`
contains an address and `Argument 1` contains a length.  In the case
of failure, the address and length are those that were passed in the
call.  In the case of success, the address and length are those that
were passed in the previous call. On the first successful invocation
of a particular Read-Write Allow system call, the kernel returns
address 0 and size 0.

The buffer identifier specifies which buffer this is. A driver may
support multiple allowed buffers.

The Tock kernel MUST check that the passed buffer is contained within
the calling process's writeable address space. Every byte of the
passed buffer must be readable and writeable by the
process. Zero-length buffers may therefore have abitrary addresses. If
the passed buffer is not complete within the calling process's
writeable address space, the kernel MUST return a failure result with
an error code of INVAL (invalid value).

Because a process relinquishes access to a buffer when it makes a
Read-Write Allow call with it, the buffer passed on the subsequent
Read-Write Allow call cannot overlap with the first passed buffer.
This is because the application does not have access to that
memory. If an application needs to extend a buffer, it must first call
Read-Write Allow to reclaim the buffer, then call Read-Write Allow
again to re-allow it with a different size.

4.5 Read-Only Allow (Class ID: 4)
---------------------------------

The Read-Only Allow class is identical to the Read-Write Allow class
with two exceptions: the buffer it passes to the kernel is read-only,
and the process retains read access to the buffer. The kernel cannot
write to the buffer. The semantics and calling conventions of
Read-Only Allow are otherwise identical to Read-Write Allow.

The Read-Only Allow class exists so that userspace can pass references
to constant data to the kernel. This is useful, for example, when a
process prints a constant string to the console; it wants to allow the
constant string to the kernel as an application slice, then call
a command that transmits the allowed slice. Constant strings are usually
stored in flash, rather than RAM, which Tock's memory protection marks
as read-only memory. Therefore, if a process tries to pass a constant
string stored in flash through a Read-Write Allow, the allow will fail
because the kernel detects that the passed slice is not writeable.

Another common use case for Read-Only allow is passing test or
diagnostic data. A U2F authentication key, for example, will often
run some [cryptographic tests at boot](https://github.com/google/tock-on-titan/blob/master/userspace/u2f_app/fips_crypto_tests.c) to ensure correct
operation. These tests store input data, keys, and expected output data
as constants in flash. An encrypt operation, for example, wants to be
able to pass a read-only input and read-only key to obtain a
ciphertext. Without a read-only allow, all of this read-only data
has to be copied into RAM, and for software engineering reasons
these RAM buffers may be difficult to reuse.

Having a Read-Only Allow allows a system call driver to clearly
specify whether data is read-only or read-write and also saves
processes the RAM overhead of having to copy read-only data into
RAM so it can be passed with a Read-Write Allow.

The Tock kernel MUST check that the passed buffer is contained within
the calling process's readable address space. Every byte of the passed
buffer must be readable and writeable by the process. Zero-length
buffers may therefore have abitrary addresses. If the passed buffer is
not complete within the calling process's readable address space, the
kernel MUST return a failure result with an error code of INVAL
(invalid value).

4.6 Memop (Class ID: 5)
---------------------------------

The Memop class is how a userspace process requests and provides
information about its address space.  The register arguments for
Allow system calls are as follows. The registers r0-r3 correspond
to r0-r3 on CortexM and a0-a3 on RISC-V.

| Argument               | Register |
|------------------------|----------|
| Operation              | r0       |
| Operation argument     | r1       |
| unused                 | r2       |
| unused                 | r3       |

The operation argument specifies which memory operation to perform. There
are 12:

| Memop Operation | Operation                                               | Success          |
|-----------------|---------------------------------------------------------|------------------|
| 0               | Break                                                   | Success          |
| 1               | SBreak                                                  | Success with u32 |
| 2               | Get process RAM start address                           | Success with u32 |
| 3               | Get address immediately after process RAM allocation    | Success with u32 |
| 4               | Get process flash start address                         | Success with u32 |
| 5               | Get address immediately after process flash region      | Success with u32 |
| 6               | Get lowest address (end) of the grant region            | Success with u32 |
| 7               | Get number of writeable flash regions in process header | Success with u32 |
| 8               | Get start address of a writeable flash region           | Success with u32 |
| 9               | Get end adddress of a writeable flash region            | Success with u32 |
| 10              | Set the start of the process stack                      | Success          |
| 11              | Set the start of the process heap                       | Success          |

The success return variant is Memop class system call specific and
specified in the table above. All Memop class system calls have a
_Failure_ failure type.

4.7 Exit (Class ID: 6)
--------------------------------

The Exit system call class is how a userspace process terminates. 
Successful calls to Exit system calls do not return.

There are two Exit system calls:
  - `exit-terminate`
  - `exit-restart`

The first call, `exit-terminate`, terminates the process and tells the
kernel that it may reclaim and reallocate the process as well as all of its
resources. Usually this indicates that the process has completed
its work. 

The second call, `exit-restart`, terminates the process and tells the kernel
that the application would like to restart if possible. If the kernel 
restarts the application, it MUST assign it a new process identifier. The
kernel MAY reuse existing process resources (e.g., RAM regions) or MAY
allocate new ones.

The register arguments for Exit system calls are as follows. The registers
r0-r3 correspond to r0-r3 on CortexM and a0-a3 on RISC-V.

| Argument          | Register |
|-------------------|----------|
| Exit identifer    | r0       |
| Completion code   | r1       |

The exit identifier specifies which call is invoked.

| System call     | Exit identifier value |
|-----------------|-----------------------|
| exit-terminate  |                     0 |
| exit-restart    |                     1 |

The difference between `exit-terminate` and `exit-restart` is what behavior
the application asks from the kernel. With `exit-terminate`, the application
tells the kernel that it considers itself completed and does not need to run
again. With `exit-restart`, it tells the kernel that it would like to be
rebooted and run again. For example, `exit-terminate` might be used by a 
process that stores some one-time data on flash, while `exit-restart` might
be used if the process runs out of memory.

The completion code is an unsigned 32-bit number which indicates status. This
information can be stored in the kernel and used in management or policy decisions.
The definition of these status codes is outside the scope of this document.

If an exit syscall is successful, it does not return. Therefore, the return
value of an exit syscall is always _Failure_. `exit-restart` and 
`exit-terminate` MUST always succeed and so never return. 

5 Userspace Library Methods
=================================

This section describes the method signatures for system calls and callbacks in C and Rust.
Because C allows a single return value but Tock system calls can return multiple values,
the low-level APIs are not idiomatic C. These low-level APIs are translated into standard C
code by the userspace library.

5.1 libtock-c
---------------------------------

5.1 libtock-rs
---------------------------------

6 The Driver Trait
=================================

The core kernel, in response to userspace system calls, invokes methods on the Driver
trait implemented by system call drivers.  This section describes the Driver trait API
and how it interacts with the core kernel's system calls. Note that


6.1 Return Types
---------------------------------

Methods in the Driver trait have return values of Rust types that
correspond to their allowed return values and which have corresponding
in encodings desribed in Section 3.2.

6.1.1 Return Type Values
---------------------------------



6.1.2 Yield Return Type
---------------------------------

6.1.3 Subscribe Return Type
---------------------------------

6.1.4 Command  Return Type
---------------------------------

6.1.5 Allow Return Type
---------------------------------

6.1.6 Memop Return Type
---------------------------------



7 Authors' Address
=================================
```
email - Guillaume Endignoux <guillaumee@google.com>
email - Jon Flatley <jflat@google.com>
email - Philip Levis <pal@cs.stanford.edu>
email - Amit Levy <aalevy@cs.princeton.edu>
email - Leon Schuermann <leon@is.currently.online>
email - Johnathan Van Why <jrvanwhy@google.com>
```

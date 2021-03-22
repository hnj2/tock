//! System call interface for userspace processes.
//!
//! Drivers implement these interfaces to expose operations to processes.
//!
//! # System-call Overview
//!
//! Tock supports six system calls. The `yield` and `memop` system calls are
//! handled by the core kernel, while four others are implemented by drivers:
//!
//!   * `subscribe` passes a callback to the driver which it can
//!   invoke on the process later, when an event has occurred or data
//!   of interest is available.
//!
//!   * `command` tells the driver to do something immediately.
//!
//!   * `allow read-write` provides the driver read-write access to an
//!   application buffer.
//!
//!   * `allow read-only` provides the driver read-only access to an
//!   application buffer.
//!
//! ## Mapping system-calls to drivers
//!
//! Each of these three system calls takes at least two
//! parameters. The first is a _driver identifier_ and tells the
//! scheduler which driver to forward the system call to. The second
//! parameters is a __syscall identifer_ and is used by the driver to
//! differentiate instances of the call with different driver-specific
//! meanings (e.g. `subscribe` for "data received" vs `subscribe` for
//! "send completed"). The mapping between _driver identifiers_ and
//! drivers is determined by a particular platform, while the _syscall
//! identifier_ is driver-specific.
//!
//! One convention in Tock is that _driver minor number_ 0 for the `command`
//! syscall can always be used to determine if the driver is supported by
//! the running kernel by checking the return code. If the return value is
//! greater than or equal to zero then the driver is present. Typically this is
//! implemented by a null command that only returns 0, but in some cases the
//! command can also return more information, like the number of supported
//! devices (useful for things like the number of LEDs).
//!
//! # The `yield` system call class
//!
//! While drivers do not handle `yield` system calls, it is important
//! to understand them and how they interacts with `subscribe`, which
//! registers callback functions with the kernel. When a process calls
//! a `yield` system call, the kernel checks if there are any pending
//! callbacks for the process. If there are pending callbacks, it
//! pushes one callback onto the process stack. If there are no
//! pending callbacks, `yield-wait` will cause the process to sleep
//! until a callback is trigered, while `yield-no-wait` returns
//! immediately.
//!
//! # Method result types
//!
//! Each driver method has a limited set of valid return types. Every
//! method has a single return type corresponding to success and a
//! single return type corresponding to failure. For the `subscribe`
//! and `allow` system calls, these return types are the same for
//! every instance of those calls. Each instance of the `command`
//! system call, however, has its own specified return types. A
//! command that requests a timestamp, for example, might return a
//! 32-bit number on success and an error code on failure, while a
//! command that requests time of day in microsecond granularity might
//! return a 64-bit number and a 32-bit timezone encoding on success,
//! and an error code on failure.
//!
//! These result types are represented as safe Rust types. The core
//! kernel (the scheduler and syscall dispatcher) is responsible for
//! encoding these types into the Tock system call ABI specification.

use crate::callback::{AppId, Callback};
use crate::errorcode::ErrorCode;
use crate::mem::legacy::{AppSlice, SharedReadWrite};
use crate::mem::{ReadOnlyAppSlice, ReadWriteAppSlice};
use crate::process;
use crate::returncode::ReturnCode;
use crate::syscall::GenericSyscallReturnValue;

/// Possible return values of a `command` driver method
///
/// This is just a wrapper around
/// [`GenericSyscallReturnValue`](GenericSyscallReturnValue) since a
/// `command` driver method may only return primitve integer types as
/// payload.
///
/// It is important for this wrapper to only be constructable over
/// variants of
/// [`GenericSyscallReturnValue`](GenericSyscallReturnValue) that are
/// deemed safe for a capsule to construct and return to an
/// application (e.g. not
/// [`SubscribeSuccess`](crate::syscall::GenericSyscallReturnValue::SubscribeSuccess)).
/// This means that the inner value **must** remain private.
pub struct CommandResult(GenericSyscallReturnValue);
impl CommandResult {
    pub(crate) fn into_inner(self) -> GenericSyscallReturnValue {
        self.0
    }

    /// Command error
    pub fn failure(rc: ErrorCode) -> Self {
        CommandResult(GenericSyscallReturnValue::Failure(rc))
    }

    /// Command error with an additional 32-bit data field
    pub fn failure_u32(rc: ErrorCode, data0: u32) -> Self {
        CommandResult(GenericSyscallReturnValue::FailureU32(rc, data0))
    }

    /// Command error with two additional 32-bit data fields
    pub fn failure_u32_u32(rc: ErrorCode, data0: u32, data1: u32) -> Self {
        CommandResult(GenericSyscallReturnValue::FailureU32U32(rc, data0, data1))
    }

    /// Command error with an additional 64-bit data field
    pub fn failure_u64(rc: ErrorCode, data0: u64) -> Self {
        CommandResult(GenericSyscallReturnValue::FailureU64(rc, data0))
    }

    /// Successful command
    pub fn success() -> Self {
        CommandResult(GenericSyscallReturnValue::Success)
    }

    /// Successful command with an additional 32-bit data field
    pub fn success_u32(data0: u32) -> Self {
        CommandResult(GenericSyscallReturnValue::SuccessU32(data0))
    }

    /// Successful command with two additional 32-bit data fields
    pub fn success_u32_u32(data0: u32, data1: u32) -> Self {
        CommandResult(GenericSyscallReturnValue::SuccessU32U32(data0, data1))
    }

    /// Successful command with three additional 32-bit data fields
    pub fn success_u32_u32_u32(data0: u32, data1: u32, data2: u32) -> Self {
        CommandResult(GenericSyscallReturnValue::SuccessU32U32U32(
            data0, data1, data2,
        ))
    }

    /// Successful command with an additional 64-bit data field
    pub fn success_u64(data0: u64) -> Self {
        CommandResult(GenericSyscallReturnValue::SuccessU64(data0))
    }

    /// Successful command with an additional 64-bit and 32-bit data field
    pub fn success_u64_u32(data0: u64, data1: u32) -> Self {
        CommandResult(GenericSyscallReturnValue::SuccessU64U32(data0, data1))
    }
}

use core::convert::TryFrom;
impl From<ReturnCode> for CommandResult {
    fn from(rc: ReturnCode) -> Self {
        match rc {
            ReturnCode::SUCCESS => CommandResult::success(),
            _ => CommandResult::failure(ErrorCode::try_from(rc).unwrap()),
        }
    }
}

impl From<process::Error> for CommandResult {
    fn from(perr: process::Error) -> Self {
        CommandResult::failure(perr.into())
    }
}

#[allow(unused_variables)]
pub trait Driver {
    fn subscribe(
        &self,
        which: usize,
        callback: Callback,
        app_id: AppId,
    ) -> Result<Callback, (Callback, ErrorCode)> {
        Err((callback, ErrorCode::NOSUPPORT))
    }

    fn command(&self, which: usize, r2: usize, r3: usize, caller_id: AppId) -> CommandResult {
        CommandResult::failure(ErrorCode::NOSUPPORT)
    }

    fn allow_readwrite(
        &self,
        app: AppId,
        which: usize,
        slice: ReadWriteAppSlice,
    ) -> Result<ReadWriteAppSlice, (ReadWriteAppSlice, ErrorCode)> {
        Err((slice, ErrorCode::NOSUPPORT))
    }

    fn allow_readonly(
        &self,
        app: AppId,
        which: usize,
        slice: ReadOnlyAppSlice,
    ) -> Result<ReadOnlyAppSlice, (ReadOnlyAppSlice, ErrorCode)> {
        Err((slice, ErrorCode::NOSUPPORT))
    }
}

/// Tock 1.x "legacy" system call interface
///
/// This is included for compatibility with capsules not ported to the
/// new system call interface. It will be removed prior to a Tock 2.0
/// release.
// TODO: Remove prior to Tock 2.0
pub trait LegacyDriver {
    /// `subscribe` lets an application pass a callback to the driver to be
    /// called later. This returns `ENOSUPPORT` if not used.
    ///
    /// Calls to subscribe should do minimal synchronous work.  Instead, they
    /// should defer most work and returns results to the application via the
    /// callback. For example, a subscribe call might setup a DMA transfer to
    /// read from a sensor, and asynchronously respond to the application by
    /// passing the result to the application via the callback.
    ///
    /// Drivers should allow each application to register a single callback for
    /// each minor number subscription. Thus, a second call to subscribe from
    /// the same application would replace a previous callback.
    ///
    /// This pushes most per-application virtualization to the application
    /// itself. For example, a timer driver exposes only one timer to each
    /// application, and the application is responsible for virtualizing that
    /// timer if it needs to.
    ///
    /// The driver should signal success or failure through the sign of the
    /// return value from `subscribe`. A negative return value signifies an
    /// error, while positive a return values signifies success. In addition,
    /// the magnitude of the return value of can signify extra information such
    /// as error type.
    #[allow(unused_variables)]
    fn subscribe(&self, minor_num: usize, callback: Option<Callback>, app_id: AppId) -> ReturnCode {
        ReturnCode::ENOSUPPORT
    }

    /// `command` instructs a driver to perform some action synchronously. This
    /// returns `ENOSUPPORT` if not used.
    ///
    /// The return value should reflect the result of an action. For example,
    /// enabling/disabling a peripheral should return a success or error code.
    /// Reading the current system time should return the time as an integer.
    ///
    /// Commands should not execute long running tasks synchronously. However,
    /// commands might "kick-off" asynchronous tasks in coordination with a
    /// `subscribe` call.
    ///
    /// All drivers must support the command with `minor_num` 0, and return 0
    /// or greater if the driver is supported. This command should not have any
    /// side effects. This convention ensures that applications can query the
    /// kernel for supported drivers on a given platform.
    #[allow(unused_variables)]
    fn command(&self, minor_num: usize, r2: usize, r3: usize, caller_id: AppId) -> ReturnCode {
        ReturnCode::ENOSUPPORT
    }

    /// `allow_readwrite` lets an application give the driver
    /// read-write access to a buffer in the application's
    /// memory. This returns `ENOSUPPORT` if not used.
    ///
    /// The buffer is __shared__ between the application and driver, meaning the
    /// driver should not rely on the contents of the buffer to remain
    /// unchanged.
    #[allow(unused_variables)]
    fn allow_readwrite(
        &self,
        app: AppId,
        minor_num: usize,
        slice: Option<AppSlice<SharedReadWrite, u8>>,
    ) -> ReturnCode {
        ReturnCode::ENOSUPPORT
    }
}

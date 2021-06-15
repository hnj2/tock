//! Read Only Systems calls
//!
//! This capsule provides read only system calls to userspace applications.
//! This is similar to the Linux vDSO syscalls.
//!
//! The benefit of using these is that applications can avoid the context
//! switch overhead of traditional syscalls by just reading the value from
//! memory.
//!
//! The value will only be as accurate as the last time the application was
//! switched to by the kernel.
//!
//! The layout of the read only syscalls in the allow region depends on the
//! version. Userspace can use `command 0` to get the version information.
//!
//! Versions are backwards compatible, that is new versions will only add
//! fields, not remove existing ones or change the order.

use crate::grant::Grant;
use crate::process::ProcessId;
use crate::upcall::Upcall;
use crate::{CommandReturn, Driver, ErrorCode, ReadWriteAppSlice};

/// Syscall driver number.
pub const DRIVER_NUM: usize = 0x10001;
const VERSION: u32 = 1;

pub struct ROSDriver {
    apps: Grant<App>,
}

impl ROSDriver {
    pub fn new(grant: Grant<App>) -> ROSDriver {
        ROSDriver { apps: grant }
    }

    pub fn update_values(&self, _appid: ProcessId) {}
}

impl Driver for ROSDriver {
    /// Specify memory regions to be used.
    ///
    /// ### `allow_num`
    ///
    /// - `0`: Allow a buffer for the kernel to stored syscall values.
    ///        This should only be read by the app and written by the capsule.
    fn allow_shared(
        &self,
        app: ProcessId,
        which: usize,
        mut slice: ReadWriteAppSlice,
    ) -> Result<ReadWriteAppSlice, (ReadWriteAppSlice, ErrorCode)> {
        if which == 0 {
            let res = self.apps.enter(app, |data| {
                core::mem::swap(&mut data.mem_region, &mut slice);
            });
            match res {
                Ok(_) => Ok(slice),
                Err(e) => Err((slice, e.into())),
            }
        } else {
            Err((slice, ErrorCode::NOSUPPORT))
        }
    }

    /// Subscribe to ROSDriver events.
    ///
    /// No subscribe events are supported
    fn subscribe(
        &self,
        _subscribe_num: usize,
        upcall: Upcall,
        _app_id: ProcessId,
    ) -> Result<Upcall, (Upcall, ErrorCode)> {
        Err((upcall, ErrorCode::NOSUPPORT))
    }

    /// Commands for ROSDriver.
    ///
    /// ### `command_num`
    ///
    /// - `0`: get version
    fn command(
        &self,
        command_number: usize,
        _target_id: usize,
        _: usize,
        _appid: ProcessId,
    ) -> CommandReturn {
        match command_number {
            // get version
            0 => CommandReturn::success_u32(VERSION),

            // default
            _ => CommandReturn::failure(ErrorCode::NOSUPPORT),
        }
    }
}

#[derive(Default)]
pub struct App {
    mem_region: ReadWriteAppSlice,
}

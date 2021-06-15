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
//!
//! Version 1:
//!   |-------------------------|
//!   |       Count (u32)       |
//!   |-------------------------|
//!   |   Pending Tasks (u32)   |
//!   |-------------------------|
//!   |                         |
//!   |     Time Ticks (u64)    |
//!   |-------------------------|
//!

use crate::grant::Grant;
use crate::hil::time::{Ticks, Time};
use crate::mem::ReadWrite;
use crate::process::ProcessId;
use crate::upcall::Upcall;
use crate::{CommandReturn, Driver, ErrorCode, ReadWriteAppSlice};
use core::cell::Cell;

/// Syscall driver number.
pub const DRIVER_NUM: usize = 0x10001;
const VERSION: u32 = 1;

pub struct ROSDriver<'a, T: Time> {
    timer: &'a T,

    count: Cell<u32>,
    apps: Grant<App>,
}

impl<'a, T: Time> ROSDriver<'a, T> {
    pub fn new(timer: &'a T, grant: Grant<App>) -> ROSDriver<'a, T> {
        ROSDriver {
            timer,
            count: Cell::new(0),
            apps: grant,
        }
    }

    pub fn update_values(&self, appid: ProcessId, pending_tasks: usize) {
        let count = self.count.get();
        self.apps
            .enter(appid, |app| {
                app.mem_region.mut_map_or((), |buf| {
                    if buf.len() >= 4 {
                        buf.as_mut()[0..4].copy_from_slice(&count.to_le_bytes());
                    }
                    if buf.len() >= 8 {
                        buf.as_mut()[4..8].copy_from_slice(&(pending_tasks as u32).to_le_bytes());
                    }
                    if buf.len() >= 16 {
                        let now = self.timer.now().into_usize() as u64;
                        buf.as_mut()[8..16].copy_from_slice(&now.to_le_bytes());
                    }
                })
            })
            .unwrap();

        self.count.set(count.wrapping_add(1));
    }
}

impl<'a, T: Time> Driver for ROSDriver<'a, T> {
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

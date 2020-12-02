//! Provides userspace with access to a serial interface.
//!
//! Setup
//! -----
//!
//! You need a device that provides the `hil::uart::UART` trait.
//!
//! ```rust
//! # use kernel::static_init;
//! # use capsules::console::Console;
//!
//! let console = static_init!(
//!     Console<usart::USART>,
//!     Console::new(&usart::USART0,
//!                  115200,
//!                  &mut console::WRITE_BUF,
//!                  &mut console::READ_BUF,
//!                  board_kernel.create_grant(&grant_cap)));
//! hil::uart::UART::set_client(&usart::USART0, console);
//! ```
//!
//! Usage
//! -----
//!
//! The user must perform three steps in order to write a buffer:
//!
//! ```c
//! // (Optional) Set a callback to be invoked when the buffer has been written
//! subscribe(CONSOLE_DRIVER_NUM, 1, my_callback);
//! // Share the buffer from userspace with the driver
//! allow(CONSOLE_DRIVER_NUM, buffer, buffer_len_in_bytes);
//! // Initiate the transaction
//! command(CONSOLE_DRIVER_NUM, 1, len_to_write_in_bytes)
//! ```
//!
//! When the buffer has been written successfully, the buffer is released from
//! the driver. Successive writes must call `allow` each time a buffer is to be
//! written.

use core::convert::TryFrom;
use core::{cmp, mem};

use kernel::common::cells::{OptionalCell, TakeCell};
use kernel::hil::uart;
use kernel::{AppId, Callback, ErrorCode, Grant};
use kernel::{CommandResult, Driver, ReturnCode, SharedReadWrite};
use kernel::{Read, ReadOnlyAppSlice, ReadWrite, ReadWriteAppSlice};

/// Syscall driver number.
use crate::driver;
pub const DRIVER_NUM: usize = driver::NUM::Console as usize;

#[derive(Default)]
pub struct App {
    write_callback: Callback,
    write_buffer: ReadOnlyAppSlice,
    write_len: usize,
    write_remaining: usize, // How many bytes didn't fit in the buffer and still need to be printed.
    pending_write: bool,

    read_callback: Callback,
    read_buffer: ReadWriteAppSlice,
    read_len: usize,
}

pub static mut WRITE_BUF: [u8; 64] = [0; 64];
pub static mut READ_BUF: [u8; 64] = [0; 64];

pub struct Console<'a> {
    uart: &'a dyn uart::UartData<'a>,
    apps: Grant<App>,
    tx_in_progress: OptionalCell<AppId>,
    tx_buffer: TakeCell<'static, [u8]>,
    rx_in_progress: OptionalCell<AppId>,
    rx_buffer: TakeCell<'static, [u8]>,
}

impl<'a> Console<'a> {
    pub fn new(
        uart: &'a dyn uart::UartData<'a>,
        tx_buffer: &'static mut [u8],
        rx_buffer: &'static mut [u8],
        grant: Grant<App>,
    ) -> Console<'a> {
        Console {
            uart: uart,
            apps: grant,
            tx_in_progress: OptionalCell::empty(),
            tx_buffer: TakeCell::new(tx_buffer),
            rx_in_progress: OptionalCell::empty(),
            rx_buffer: TakeCell::new(rx_buffer),
        }
    }

    /// Internal helper function for setting up a new send transaction
    fn send_new(&self, app_id: AppId, app: &mut App, len: usize) -> ReturnCode {
        app.write_len = cmp::min(len, app.write_buffer.len());
        app.write_remaining = app.write_len;
        self.send(app_id, app);
        ReturnCode::SUCCESS
    }

    /// Internal helper function for continuing a previously set up transaction
    /// Returns true if this send is still active, or false if it has completed
    fn send_continue(&self, app_id: AppId, app: &mut App) -> Result<bool, ReturnCode> {
        if app.write_remaining > 0 {
            self.send(app_id, app);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Internal helper function for sending data for an existing transaction.
    /// Cannot fail. If can't send now, it will schedule for sending later.
    fn send(&self, app_id: AppId, app: &mut App) {
        if self.tx_in_progress.is_none() {
            self.tx_in_progress.set(app_id);
            self.tx_buffer.take().map(|buffer| {
                let transaction_len = app.write_buffer.map_or(0, |data| {
                    for (i, c) in data[data.len() - app.write_remaining..data.len()]
                        .iter()
                        .enumerate()
                    {
                        if buffer.len() <= i {
                            return i;
                        }
                        buffer[i] = *c;
                    }
                    app.write_remaining
                });

                app.write_remaining -= transaction_len;
                let (_err, _opt) = self.uart.transmit_buffer(buffer, transaction_len);
            });
        } else {
            app.pending_write = true;
        }
    }

    /// Internal helper function for starting a receive operation
    fn receive_new(&self, app_id: AppId, app: &mut App, len: usize) -> ReturnCode {
        if self.rx_buffer.is_none() {
            // For now, we tolerate only one concurrent receive operation on this console.
            // Competing apps will have to retry until success.
            return ReturnCode::EBUSY;
        }

        let read_len = cmp::min(len, app.read_buffer.len());
        if read_len > self.rx_buffer.map_or(0, |buf| buf.len()) {
            // For simplicity, impose a small maximum receive length
            // instead of doing incremental reads
            ReturnCode::EINVAL
        } else {
            // Note: We have ensured above that rx_buffer is present
            app.read_len = read_len;
            self.rx_buffer.take().map(|buffer| {
                self.rx_in_progress.set(app_id);
                let (_err, _opt) = self.uart.receive_buffer(buffer, app.read_len);
            });
            ReturnCode::SUCCESS
        }
    }
}

impl Driver for Console<'_> {
    /// Setup shared buffers.
    ///
    /// ### `allow_num`
    ///
    /// - `1`: Writeable buffer for read buffer
    fn allow_readwrite(
        &self,
        appid: AppId,
        allow_num: usize,
        mut slice: ReadWriteAppSlice,
    ) -> Result<ReadWriteAppSlice, (ReadWriteAppSlice, ErrorCode)> {
        let res = match allow_num {
            1 => self
                .apps
                .enter(appid, |app, _| {
                    mem::swap(&mut app.read_buffer, &mut slice);
                })
                .map_err(ErrorCode::from),
            _ => Err(ErrorCode::NOSUPPORT),
        };

        if let Err(e) = res {
            Err((slice, e))
        } else {
            Ok(slice)
        }
    }

    /// Setup shared buffers.
    ///
    /// ### `allow_num`
    ///
    /// - `1`: Readonly buffer for write buffer
    fn allow_readonly(
        &self,
        appid: AppId,
        allow_num: usize,
        mut slice: ReadOnlyAppSlice,
    ) -> Result<ReadOnlyAppSlice, (ReadOnlyAppSlice, ErrorCode)> {
        let res = match allow_num {
            1 => self
                .apps
                .enter(appid, |app, _| {
                    mem::swap(&mut app.write_buffer, &mut slice);
                })
                .map_err(ErrorCode::from),
            _ => Err(ErrorCode::NOSUPPORT),
        };

        if let Err(e) = res {
            Err((slice, e))
        } else {
            Ok(slice)
        }
    }
    /// Setup callbacks.
    ///
    /// ### `subscribe_num`
    ///
    /// - `1`: Write buffer completed callback
    fn subscribe(
        &self,
        subscribe_num: usize,
        mut callback: Callback,
        app_id: AppId,
    ) -> Result<Callback, (Callback, ErrorCode)> {
        let res = match subscribe_num {
            1 => {
                // putstr/write done
                self.apps
                    .enter(app_id, |app, _| {
                        mem::swap(&mut app.write_callback, &mut callback);
                    })
                    .map_err(ErrorCode::from)
            }
            2 => {
                // getnstr/read done
                self.apps
                    .enter(app_id, |app, _| {
                        mem::swap(&mut app.read_callback, &mut callback);
                    })
                    .map_err(ErrorCode::from)
            }
            _ => Err(ErrorCode::NOSUPPORT),
        };

        if let Err(e) = res {
            Err((callback, e))
        } else {
            Ok(callback)
        }
    }

    /// Initiate serial transfers
    ///
    /// ### `command_num`
    ///
    /// - `0`: Driver check.
    /// - `1`: Transmits a buffer passed via `allow`, up to the length
    ///        passed in `arg1`
    /// - `2`: Receives into a buffer passed via `allow`, up to the length
    ///        passed in `arg1`
    /// - `3`: Cancel any in progress receives and return (via callback)
    ///        what has been received so far.
    fn command(&self, cmd_num: usize, arg1: usize, _: usize, appid: AppId) -> CommandResult {
        let res = match cmd_num {
            0 => Ok(ReturnCode::SUCCESS),
            1 => {
                // putstr
                let len = arg1;
                self.apps
                    .enter(appid, |app, _| self.send_new(appid, app, len))
                    .map_err(ErrorCode::from)
            }
            2 => {
                // getnstr
                let len = arg1;
                self.apps
                    .enter(appid, |app, _| self.receive_new(appid, app, len))
                    .map_err(ErrorCode::from)
            }
            3 => {
                // Abort RX
                self.uart.receive_abort();
                Ok(ReturnCode::SUCCESS)
            }
            _ => Err(ErrorCode::NOSUPPORT),
        };
        match res {
            Ok(r) => {
                let res = ErrorCode::try_from(r);
                match res {
                    Err(_) => CommandResult::success(),
                    Ok(e) => CommandResult::failure(e),
                }
            }
            Err(e) => CommandResult::failure(e),
        }
    }
}

impl uart::TransmitClient for Console<'_> {
    fn transmitted_buffer(&self, buffer: &'static mut [u8], _tx_len: usize, _rcode: ReturnCode) {
        // Either print more from the AppSlice or send a callback to the
        // application.
        self.tx_buffer.replace(buffer);
        self.tx_in_progress.take().map(|appid| {
            self.apps.enter(appid, |app, _| {
                match self.send_continue(appid, app) {
                    Ok(more_to_send) => {
                        if !more_to_send {
                            // Go ahead and signal the application
                            let written = app.write_len;
                            app.write_len = 0;
                            app.write_callback.schedule(written, 0, 0);
                        }
                    }
                    Err(return_code) => {
                        // XXX This shouldn't ever happen?
                        app.write_len = 0;
                        app.write_remaining = 0;
                        app.pending_write = false;
                        let r0 = isize::from(return_code) as usize;
                        app.write_callback.schedule(r0, 0, 0);
                    }
                }
            })
        });

        // If we are not printing more from the current AppSlice,
        // see if any other applications have pending messages.
        if self.tx_in_progress.is_none() {
            for cntr in self.apps.iter() {
                let started_tx = cntr.enter(|app, _| {
                    if app.pending_write {
                        app.pending_write = false;
                        match self.send_continue(app.appid(), app) {
                            Ok(more_to_send) => more_to_send,
                            Err(return_code) => {
                                // XXX This shouldn't ever happen?
                                app.write_len = 0;
                                app.write_remaining = 0;
                                app.pending_write = false;
                                let r0 = isize::from(return_code) as usize;
                                app.write_callback.schedule(r0, 0, 0);
                                false
                            }
                        }
                    } else {
                        false
                    }
                });
                if started_tx {
                    break;
                }
            }
        }
    }
}

impl uart::ReceiveClient for Console<'_> {
    fn received_buffer(
        &self,
        buffer: &'static mut [u8],
        rx_len: usize,
        rcode: ReturnCode,
        error: uart::Error,
    ) {
        self.rx_in_progress
            .take()
            .map(|appid| {
                self.apps
                    .enter(appid, |app, _| {
                        // An iterator over the returned buffer yielding only the first `rx_len`
                        // bytes
                        let rx_buffer = buffer.iter().take(rx_len);
                        match error {
                            uart::Error::None | uart::Error::Aborted => {
                                // Receive some bytes, signal error type and return bytes to process buffer
                                let count = app.read_buffer.mut_map_or(-1, |data| {
                                    let mut c = 0;
                                    for (a, b) in data.iter_mut().zip(rx_buffer) {
                                        c = c + 1;
                                        *a = *b;
                                    }
                                    c
                                });

                                // Make sure we report the same number
                                // of bytes that we actually copied into
                                // the app's buffer. This is defensive:
                                // we shouldn't ever receive more bytes
                                // than will fit in the app buffer since
                                // we use the app_buffer's length when
                                // calling `receive()`. However, a buggy
                                // lower layer could return more bytes
                                // than we asked for, and we don't want
                                // to propagate that length error to
                                // userspace. However, we do return an
                                // error code so that userspace knows
                                // something went wrong.
                                //
                                // If count < 0 this means the buffer
                                // disappeared: return ENOMEM.
                                let (ret, received_length) = if count < 0 {
                                    (ReturnCode::ENOMEM, 0)
                                } else if rx_len > app.read_buffer.len() {
                                    // Return `ESIZE` indicating that
                                    // some received bytes were dropped.
                                    // We report the length that we
                                    // actually copied into the buffer,
                                    // but also indicate that there was
                                    // an issue in the kernel with the
                                    // receive.
                                    (ReturnCode::ESIZE, app.read_buffer.len())
                                } else {
                                    // This is the normal and expected
                                    // case.
                                    (rcode, rx_len)
                                };

                                app.read_callback
                                    .schedule(From::from(ret), received_length, 0);
                            }
                            _ => {
                                // Some UART error occurred
                                app.read_callback
                                    .schedule(From::from(ReturnCode::FAIL), 0, 0);
                            }
                        }
                    })
                    .unwrap_or_default();
            })
            .unwrap_or_default();

        // Whatever happens, we want to make sure to replace the rx_buffer for future transactions
        self.rx_buffer.replace(buffer);
    }
}

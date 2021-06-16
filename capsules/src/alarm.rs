//! Tock syscall driver capsule for Alarms, which issue callbacks when
//! a point in time has been reached.

use core::cell::Cell;
use core::mem;
use kernel::hil::time::{self, Alarm, Frequency, Ticks, Ticks32};
use kernel::ros::ROSDriver;
use kernel::{
    CommandReturn, Driver, ErrorCode, Grant, ProcessId, ReadWrite, ReadWriteAppSlice, Upcall,
};

/// Syscall driver number.
use crate::driver;
pub const DRIVER_NUM: usize = driver::NUM::Alarm as usize;

#[derive(Copy, Clone, Debug)]
enum Expiration {
    Disabled,
    Enabled { reference: u32, dt: u32 },
}

pub struct AlarmData {
    expiration: Expiration,
    callback: Upcall,

    // This capsule provides a ROS-compatible mechanism to communicate
    // the current clock ticks proactively to userspace. If this
    // mechanism is enabled, this contains the corresponding userspace
    // memory region to write the ticks to.
    ros_region: ReadWriteAppSlice,
    ros_count: Cell<u32>,
}

impl Default for AlarmData {
    fn default() -> AlarmData {
        AlarmData {
            expiration: Expiration::Disabled,
            callback: Upcall::default(),
            ros_region: ReadWriteAppSlice::default(),
            ros_count: Cell::new(0),
        }
    }
}

pub struct AlarmDriver<'a, A: Alarm<'a>> {
    alarm: &'a A,
    num_armed: Cell<usize>,
    app_alarms: Grant<AlarmData>,
    next_alarm: Cell<Expiration>,

    // If the ROS mechanism is enabled, this variable is set to true
    // as soon as the first process is scheduled for the first
    // time. This variable determines whether a read-write allow of
    // the `ros_region` is supported and thus whether the AlarmDriver
    // can supply the current ticks to userspace through the ROS
    // mechanism
    ros_enabled: Cell<bool>,
}

impl<'a, A: Alarm<'a>> AlarmDriver<'a, A> {
    pub const fn new(alarm: &'a A, grant: Grant<AlarmData>) -> AlarmDriver<'a, A> {
        AlarmDriver {
            alarm: alarm,
            num_armed: Cell::new(0),
            app_alarms: grant,
            next_alarm: Cell::new(Expiration::Disabled),
            ros_enabled: Cell::new(false),
        }
    }

    // This logic is tricky because it needs to handle the case when the
    // underlying alarm is wider than 32 bits.
    fn reset_active_alarm(&self) {
        let mut earliest_alarm = Expiration::Disabled;
        let mut earliest_end: A::Ticks = A::Ticks::from(0);
        // Scale now down to a u32 since that is the width of the alarm;
        // otherwise for larger widths (e.g., u64) now can be outside of
        // the range of what an alarm can be set to.
        let now = self.alarm.now();
        let now_lower_bits = A::Ticks::from(now.into_u32());
        // Find the first alarm to fire and store it in earliest_alarm,
        // its counter value at earliest_end. In the case that there
        // are multiple alarms in the past, just store one of them
        // and resolve ordering later, when we fire.
        for alarm in self.app_alarms.iter() {
            alarm.enter(|alarm| match alarm.expiration {
                Expiration::Enabled { reference, dt } => {
                    // Do this because `reference` shadowed below
                    let current_reference = reference;
                    let current_reference_ticks = A::Ticks::from(current_reference);
                    let current_dt = dt;
                    let current_dt_ticks = A::Ticks::from(current_dt);
                    let current_end_ticks = current_reference_ticks.wrapping_add(current_dt_ticks);

                    earliest_alarm = match earliest_alarm {
                        Expiration::Disabled => {
                            earliest_end = current_end_ticks;
                            alarm.expiration
                        }
                        Expiration::Enabled { reference, dt } => {
                            // There are two cases when current might be
                            // an earlier alarm.  The first is if it
                            // fires inside the interval (reference,
                            // reference+dt) of the existing earliest.
                            // The second is if now is not within the
                            // interval: this means that it has
                            // passed. It could be the earliest has passed
                            // too, but at this point we don't need to track
                            // which is earlier: the key point is that
                            // the alarm must fire immediately, and then when
                            // we handle the alarm callback the userspace
                            // callbacks will all be pushed onto processes.
                            // Because there is at most a single callback per
                            // process and they must go through the scheduler
                            // we don't care about the order in which we push
                            // their callbacks, as their order of execution is
                            // determined by the scheduler not push order. -pal
                            let temp_earliest_reference = A::Ticks::from(reference);
                            let temp_earliest_dt = A::Ticks::from(dt);
                            let temp_earliest_end =
                                temp_earliest_reference.wrapping_add(temp_earliest_dt);

                            if current_end_ticks
                                .within_range(temp_earliest_reference, temp_earliest_end)
                            {
                                earliest_end = current_end_ticks;
                                alarm.expiration
                            } else if !now_lower_bits
                                .within_range(temp_earliest_reference, temp_earliest_end)
                            {
                                earliest_end = temp_earliest_end;
                                alarm.expiration
                            } else {
                                earliest_alarm
                            }
                        }
                    }
                }
                Expiration::Disabled => {}
            });
        }
        self.next_alarm.set(earliest_alarm);
        match earliest_alarm {
            Expiration::Disabled => {
                let _ = self.alarm.disarm();
            }
            Expiration::Enabled { reference, dt } => {
                // This logic handles when the underlying Alarm is wider than
                // 32 bits; it sets the reference to include the high bits of now
                let mut high_bits = now.wrapping_sub(now_lower_bits);
                // If lower bits have wrapped around from reference, this means the
                // reference's high bits are actually one less; if we don't subtract
                // one then the alarm will incorrectly be set 1<<32 higher than it should.
                // This uses the invariant that reference <= now.
                if now_lower_bits.into_u32() < reference {
                    // Build 1<<32 in a way that just overflows to 0 if we are 32 bits
                    let bit33 = A::Ticks::from(0xffffffff).wrapping_add(A::Ticks::from(0x1));
                    high_bits = high_bits.wrapping_sub(bit33);
                }
                let real_reference = high_bits.wrapping_add(A::Ticks::from(reference));
                self.alarm.set_alarm(real_reference, A::Ticks::from(dt));
            }
        }
    }
}

impl<'a, A: Alarm<'a>> Driver for AlarmDriver<'a, A> {
    /// Subscribe to alarm expiration
    ///
    /// ### `_subscribe_num`
    ///
    /// - `0`: Subscribe to alarm expiration
    fn subscribe(
        &self,
        subscribe_num: usize,
        mut callback: Upcall,
        app_id: ProcessId,
    ) -> Result<Upcall, (Upcall, ErrorCode)> {
        let res: Result<(), ErrorCode> = match subscribe_num {
            0 => self
                .app_alarms
                .enter(app_id, |td| {
                    mem::swap(&mut callback, &mut td.callback);
                })
                .map_err(ErrorCode::from),
            _ => Err(ErrorCode::NOSUPPORT),
        };

        if let Err(e) = res {
            Err((callback, e))
        } else {
            Ok(callback)
        }
    }

    /// Allow an ROS memory region, if supported
    ///
    /// ### `allow_num`
    ///
    /// - `0`: allow the ROS mechanism memory region
    fn allow_readwrite(
        &self,
        process_id: ProcessId,
        allow_num: usize,
        mut slice: ReadWriteAppSlice,
    ) -> Result<ReadWriteAppSlice, (ReadWriteAppSlice, ErrorCode)> {
        match allow_num {
            0 => {
                if self.ros_enabled.get() {
                    // ROS is enabled
                    let res = self
                        .app_alarms
                        .enter(process_id, |grant| {
                            mem::swap(&mut grant.ros_region, &mut slice);
                        })
                        .map_err(|e| e.into());

                    match res {
                        Ok(()) => Ok(slice),
                        Err(e) => Err((slice, e)),
                    }
                } else {
                    // ROS is not enabled, as otherwise
                    // ROSDriver::update_values would've been called
                    // at least once when this process was initially
                    // scheduled
                    Err((slice, ErrorCode::NODEVICE))
                }
            }
            _ => Err((slice, ErrorCode::NOSUPPORT)),
        }
    }

    /// Setup and read the alarm.
    ///
    /// ### `command_num`
    ///
    /// - `0`: Driver check.
    /// - `1`: Return the clock frequency in Hz.
    /// - `2`: Read the the current clock value
    /// - `3`: Stop the alarm if it is outstanding
    /// - `4`: Set an alarm to fire at a given clock value `time`.
    /// - `5`: Set an alarm to fire at a given clock value `time` relative to `now` (EXPERIMENTAL).
    fn command(
        &self,
        cmd_type: usize,
        data: usize,
        data2: usize,
        caller_id: ProcessId,
    ) -> CommandReturn {
        // Returns the error code to return to the user and whether we need to
        // reset which is the next active alarm. We _don't_ reset if
        //   - we're disabling the underlying alarm anyway,
        //   - the underlying alarm is currently disabled and we're enabling the first alarm, or
        //   - on an error (i.e. no change to the alarms).
        self.app_alarms
            .enter(caller_id, |td| {
                // helper function to rearm alarm
                let mut rearm = |reference: usize, dt: usize| {
                    if let Expiration::Disabled = td.expiration {
                        self.num_armed.set(self.num_armed.get() + 1);
                    }
                    td.expiration = Expiration::Enabled {
                        reference: reference as u32,
                        dt: dt as u32,
                    };
                    (
                        CommandReturn::success_u32(reference.wrapping_add(dt) as u32),
                        true,
                    )
                };
                let now = self.alarm.now();
                match cmd_type {
                    0 /* check if present */ => (CommandReturn::success(), false),
                    1 /* Get clock frequency */ => {
                        let freq = <A::Frequency>::frequency();
                        (CommandReturn::success_u32(freq), false)
                    },
                    2 /* capture time */ => {
                        (CommandReturn::success_u32(now.into_u32()), false)
                    },
                    3 /* Stop */ => {
                        match td.expiration {
                            Expiration::Disabled => {
                                // Request to stop when already stopped
                                (CommandReturn::failure(ErrorCode::ALREADY), false)
                            },
                            _ => {
                                td.expiration = Expiration::Disabled;
                                let new_num_armed = self.num_armed.get() - 1;
                                self.num_armed.set(new_num_armed);
                                (CommandReturn::success(), true)
                            }
                        }
                    },
                    4 /* Set absolute expiration */ => {
                        let reference = now.into_u32() as usize;
                        let future_time = data;
                        let dt = future_time.wrapping_sub(reference);
                        // if previously unarmed, but now will become armed
                        rearm(reference, dt)
                    },
                    5 /* Set relative expiration */ => {
                        let reference = now.into_u32() as usize;
                        let dt = data;
                        // if previously unarmed, but now will become armed
                        rearm(reference, dt)
                    },
                    6 /* Set absolute expiration with reference point */ => {
                        // Taking a reference timestamp from userspace
                        // prevents wraparound bugs; future versions of
                        // libtock will use only this call and deprecate
                        // command #4; for now it is added as an additional
                        // comamnd for backwards compatibility. -pal
                        let reference = data;
                        let dt = data2;
                        rearm(reference, dt)
                    }
                    _ => (CommandReturn::failure(ErrorCode::NOSUPPORT), false)
                }
            })
            .map_or_else(
                |err| CommandReturn::failure(err.into()),
                |(result, reset)| {
                    if reset {
                        self.reset_active_alarm();
                    }
                    result
                },
            )
    }
}

impl<'a, A: Alarm<'a>> time::AlarmClient for AlarmDriver<'a, A> {
    fn alarm(&self) {
        let now: Ticks32 = Ticks32::from(self.alarm.now().into_u32());
        self.app_alarms.each(|_, alarm| {
            if let Expiration::Enabled { reference, dt } = alarm.expiration {
                // Now is not within reference, reference + ticks; this timer
                // as passed (since reference must be in the past)
                if !now.within_range(
                    Ticks32::from(reference),
                    Ticks32::from(reference.wrapping_add(dt)),
                ) {
                    alarm.expiration = Expiration::Disabled;
                    self.num_armed.set(self.num_armed.get() - 1);
                    alarm.callback.schedule(
                        now.into_u32() as usize,
                        reference.wrapping_add(dt) as usize,
                        0,
                    );
                }
            }
        });

        // If there are no armed alarms left, skip checking and just disable.
        // Otherwise, check all the alarms and find the next one, rescheduling
        // the underlying alarm.
        if self.num_armed.get() == 0 {
            let _ = self.alarm.disarm();
        } else {
            self.reset_active_alarm();
        }
    }
}

impl<'a, A: Alarm<'a>> ROSDriver for AlarmDriver<'a, A> {
    fn update_values(&self, process_id: ProcessId) {
        // Mark that the ROS mechanism is enabled
        self.ros_enabled.set(true);

        // Check if the current process has an ROS region allowed. In
        // that case, update the values accordingly.
        let _ = self.app_alarms.enter(process_id, |grant| {
            let count = grant.ros_count.get();
            grant.ros_region.mut_map_or((), |buf| {
                if buf.len() >= 12 {
                    let now = self.alarm.now().into_usize() as u64;
                    let b = buf.as_mut();
                    b[0..4].copy_from_slice(&count.to_le_bytes());
                    b[4..12].copy_from_slice(&now.to_le_bytes());
                }
            });
            grant.ros_count.set(count.wrapping_add(1));
        });
    }
}

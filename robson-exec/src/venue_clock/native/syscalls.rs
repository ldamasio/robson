use std::{ffi::CStr, io};

use super::{ClockError, NativeIo};

pub(super) struct System;

fn error(operation: &'static str) -> ClockError {
    ClockError::Native {
        operation,
        errno: io::Error::last_os_error().raw_os_error().unwrap_or(0),
    }
}

impl NativeIo for System {
    fn open(&mut self, path: &CStr) -> Result<i32, ClockError> {
        // SAFETY: valid borrowed NUL-terminated path; no O_CREAT, so no mode argument.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
        if fd < 0 {
            Err(error("open"))
        } else {
            Ok(fd)
        }
    }
    fn read(&mut self, fd: i32, buffer: &mut [u8]) -> Result<usize, ClockError> {
        // SAFETY: owned descriptor; writable slice lives throughout the call.
        let result = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if result < 0 {
            Err(error("read"))
        } else {
            Ok(result as usize)
        }
    }
    fn close(&mut self, fd: i32) -> Result<(), ClockError> {
        // SAFETY: this owner closes the descriptor exactly once, never retries.
        let result = unsafe { libc::close(fd) };
        if result == 0 {
            Ok(())
        } else {
            Err(error("close"))
        }
    }
    fn readlink(&mut self, path: &CStr, buffer: &mut [u8]) -> Result<usize, ClockError> {
        // SAFETY: static C path and bounded writable storage; no NUL is assumed.
        let result =
            unsafe { libc::readlink(path.as_ptr(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if result < 0 {
            Err(error("readlink"))
        } else {
            Ok(result as usize)
        }
    }
    fn clock(&mut self) -> Result<(i64, i64), ClockError> {
        let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: cfg restricts this module to GNU Linux LP64 x86_64/aarch64.
        // SYS_clock_gettime expects this native two-long timespec, whose aligned
        // initialized storage remains valid for the synchronous kernel write.
        let result = unsafe {
            libc::syscall(
                libc::SYS_clock_gettime,
                libc::CLOCK_BOOTTIME as libc::c_long,
                &mut time as *mut libc::timespec,
            )
        };
        match result {
            0 => Ok((time.tv_sec, time.tv_nsec)),
            -1 => Err(error("clock_gettime")),
            _ => Err(ClockError::InvalidTick),
        }
    }
}

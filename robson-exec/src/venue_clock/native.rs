use std::ffi::CStr;

use super::{ClockDomain, ClockError, TickCapture};

mod syscalls;
use syscalls::System;

const CURRENT: &CStr = c"/proc/thread-self/ns/time";
const CHILDREN: &CStr = c"/proc/thread-self/ns/time_for_children";
const BOOT: &CStr = c"/proc/sys/kernel/random/boot_id";
const THREAD: &CStr = c"/proc/thread-self";

trait NativeIo {
    fn open(&mut self, path: &CStr) -> Result<i32, ClockError>;
    fn read(&mut self, fd: i32, buffer: &mut [u8]) -> Result<usize, ClockError>;
    fn close(&mut self, fd: i32) -> Result<(), ClockError>;
    fn readlink(&mut self, path: &CStr, buffer: &mut [u8]) -> Result<usize, ClockError>;
    fn clock(&mut self) -> Result<(i64, i64), ClockError>;
}

struct File<'a, I: NativeIo> {
    io: &'a mut I,
    fd: Option<i32>,
}
impl<I: NativeIo> File<'_, I> {
    fn finish(mut self) -> Result<(), ClockError> {
        let fd = self.fd.take().expect("owned file descriptor");
        self.io.close(fd)
    }
}
impl<I: NativeIo> Drop for File<'_, I> {
    fn drop(&mut self) {
        // A failing path is already unable to publish. Never retry close: even
        // an interrupted Linux close may have released this descriptor.
        if let Some(fd) = self.fd.take() {
            let _ = self.io.close(fd);
        }
    }
}

fn read_file(io: &mut impl NativeIo, path: &CStr, buffer: &mut [u8]) -> Result<usize, ClockError> {
    let fd = io.open(path)?;
    let file = File { io, fd: Some(fd) };
    let mut used = 0;
    for _ in 0..4 {
        match file.io.read(fd, &mut buffer[used..]) {
            Ok(0) => {
                file.finish()?;
                return Ok(used);
            },
            Ok(n) if n < buffer.len() - used => used += n,
            Ok(_) => return Err(ClockError::Oversized),
            Err(ClockError::Native { errno: libc::EINTR, .. }) => (),
            Err(error) => return Err(error),
        }
    }
    Err(ClockError::ReadLimit)
}

fn text(bytes: &[u8]) -> Result<&str, ClockError> {
    if !bytes.is_ascii() {
        return Err(ClockError::Metadata("non-ascii"));
    }
    std::str::from_utf8(bytes).map_err(|_| ClockError::Metadata("encoding"))
}

fn offsets_path<'a>(
    io: &mut impl NativeIo,
    path: &'a mut [u8; 64],
) -> Result<&'a CStr, ClockError> {
    let mut link = [0; 64];
    let length = io.readlink(THREAD, &mut link)?;
    if length >= link.len() {
        return Err(ClockError::Oversized);
    }
    let (tgid, tid) = text(&link[..length])?
        .split_once("/task/")
        .ok_or(ClockError::Metadata("thread identity"))?;
    for id in [tgid, tid] {
        if id.is_empty()
            || id.len() > 10
            || !(b'1'..=b'9').contains(&id.as_bytes()[0])
            || !id.bytes().all(|b| b.is_ascii_digit())
            || id.parse::<i32>().is_err()
        {
            return Err(ClockError::Metadata("thread identity"));
        }
    }
    // Both IDs belong to this mounted procfs; a local gettid may not.
    let end = 6 + tid.len();
    path[..6].copy_from_slice(b"/proc/");
    path[6..end].copy_from_slice(tid.as_bytes());
    path[end..end + 16].copy_from_slice(b"/timens_offsets\0");
    CStr::from_bytes_with_nul(&path[..end + 16]).map_err(|_| ClockError::Metadata("thread path"))
}

fn namespace(io: &mut impl NativeIo, path: &CStr) -> Result<u64, ClockError> {
    let mut buffer = [0; 128];
    let length = io.readlink(path, &mut buffer)?;
    if length >= buffer.len() {
        return Err(ClockError::Oversized);
    }
    let digits = text(&buffer[..length])?
        .strip_prefix("time:[")
        .and_then(|s| s.strip_suffix(']'))
        .ok_or(ClockError::Metadata("namespace"))?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ClockError::Metadata("namespace"));
    }
    let id = digits.parse().map_err(|_| ClockError::Metadata("namespace"))?;
    if id == 0 {
        return Err(ClockError::Metadata("namespace"));
    }
    Ok(id)
}

fn namespaces(io: &mut impl NativeIo) -> Result<u64, ClockError> {
    let current = namespace(io, CURRENT)?;
    if current != namespace(io, CHILDREN)? {
        return Err(ClockError::NamespaceMismatch);
    }
    Ok(current)
}

fn domain(io: &mut impl NativeIo, path: &CStr) -> Result<ClockDomain, ClockError> {
    let mut boot = [0; 129];
    let count = read_file(io, BOOT, &mut boot)?;
    let boot = text(&boot[..count])?
        .trim()
        .parse()
        .map_err(|_| ClockError::Metadata("boot uuid"))?;
    let mut offsets = [0; 1025];
    let count = read_file(io, path, &mut offsets)?;
    let mut monotonic = None;
    let mut boottime = None;
    for line in text(&offsets[..count])?.lines() {
        let mut fields = line.split_ascii_whitespace();
        let target = match fields.next() {
            Some("monotonic") => &mut monotonic,
            Some("boottime") => &mut boottime,
            _ => return Err(ClockError::Metadata("offset clock")),
        };
        let seconds = fields.next().and_then(|s| s.parse::<i64>().ok());
        let nanos = fields.next().and_then(|s| s.parse::<u32>().ok());
        let (Some(seconds), Some(nanos)) = (seconds, nanos) else {
            return Err(ClockError::Metadata("offset value"));
        };
        if nanos >= 1_000_000_000 || fields.next().is_some() || target.is_some() {
            return Err(ClockError::Metadata("offset shape"));
        }
        *target = Some((seconds, nanos));
    }
    if monotonic.is_none() {
        return Err(ClockError::Metadata("missing monotonic"));
    }
    let (seconds, nanos) = boottime.ok_or(ClockError::Metadata("missing boottime"))?;
    ClockDomain::new(boot, seconds, nanos)
}

fn initialize_with(io: &mut impl NativeIo) -> Result<TickCapture, ClockError> {
    let mut storage = [0; 64];
    let path = offsets_path(io, &mut storage)?;
    let before_ns = namespaces(io)?;
    let before = domain(io, path)?;
    let (seconds, nanos) = io.clock()?;
    let capture = TickCapture::new(before, seconds, nanos)?;
    let after = domain(io, path)?;
    let after_ns = namespaces(io)?;
    if before != after || before_ns != after_ns {
        return Err(ClockError::DomainChanged);
    }
    Ok(capture)
}

pub(super) fn initialize() -> Result<TickCapture, ClockError> {
    initialize_with(&mut System)
}
pub(super) fn tick() -> Result<(i64, i64), ClockError> {
    System.clock()
}

#[cfg(test)]
mod tests;

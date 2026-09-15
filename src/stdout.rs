use crate::output::{SLOTS_PER_WORKER, SLOT_CAP};

use std::fs::File;
use std::mem::ManuallyDrop;
use std::io::{self, Write, IoSlice};

#[cfg(unix)]
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};

#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle};

#[derive(Clone, Copy, PartialEq)]
pub enum OutputKind { #[cfg(unix)] Tty, #[cfg(unix)] Pipe, #[cfg(unix)] File, Other }

#[cfg(unix)]
fn detect_output_kind(fd: std::os::unix::io::RawFd) -> OutputKind {
    unsafe {
        if libc::isatty(fd) == 1 {
            return OutputKind::Tty;
        }

        let mut stat: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &mut stat) == 0 {
            if (stat.st_mode & libc::S_IFMT) == libc::S_IFIFO {
                return OutputKind::Pipe;
            }
            if (stat.st_mode & libc::S_IFMT) == libc::S_IFREG {
                return OutputKind::File;
            }
        }
    }

    OutputKind::Other
}

/// Owns the process's stdout descriptor/handle directly, bypassing
/// std::io::Stdout's internal LineWriter, which flushes on every '\n'
/// no matter what buffering is wrapped around it.
///
/// ManuallyDrop so dropping this value never closes the descriptor or
/// handle out from under the rest of the process, the OS owns it, not us.
pub struct RawStdout(ManuallyDrop<File>);

impl RawStdout {
    #[allow(unused_assignments)]
    pub fn new() -> (Option<(Self, RawFd)>, OutputKind) {
        #[cfg(unix)]
        let file = unsafe { File::from_raw_fd(io::stdout().as_raw_fd()) };

        #[cfg(unix)]
        let fd = file.as_raw_fd();

        let mut output_kind = OutputKind::Other;
        #[cfg(unix)] { // Grow the pipe buffer so the reader doesn't force to block as often
            output_kind = detect_output_kind(fd);
            if output_kind == OutputKind::Pipe {
                tune_pipe_capacity(fd);
            }
        }

        #[cfg(unix)]
        if is_stdout_being_redirected_to_dev_null(fd) {
            return (None, output_kind);
        }

        #[cfg(windows)]
        let file = unsafe { File::from_raw_handle(io::stdout().as_raw_handle()) };

        (Some((RawStdout(ManuallyDrop::new(file)), fd)), output_kind)
    }
}

impl Write for RawStdout {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.0.write_all(buf)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.0.write_vectored(bufs)
    }
}

unsafe impl Send for RawStdout {}

pub const IOV_MAX: usize = 1024; // Linux UIO_MAXIOV

#[cfg(unix)]
fn is_stdout_being_redirected_to_dev_null(fd: i32) -> bool {
    use std::os::unix::fs::MetadataExt;

    let fd_path = format!("/proc/self/fd/{}", fd);
    let Ok(fd_meta) = std::fs::metadata(&fd_path) else { return false }; // follows the symlink, gives target's stat
    let Ok(null_meta) = std::fs::metadata("/dev/null") else { return false };

    fd_meta.dev() == null_meta.dev() && fd_meta.ino() == null_meta.ino()
}

#[cfg(target_os = "linux")]
fn tune_pipe_capacity(raw_fd: std::os::unix::io::RawFd) {
    let desired = (SLOTS_PER_WORKER * SLOT_CAP) as libc::c_int; // 1.25MB
    // SAFETY: raw_fd is our own pipe fd; F_SETPIPE_SZ only resizes the
    // kernel-side ring buffer, no memory aliasing involved. Ignoring the
    // result is fine -- on failure (e.g. /proc/sys/fs/pipe-max-size caps
    // it, or CAP_SYS_RESOURCE is required past that) the pipe just keeps
    // its previous capacity and the vmsplice fallback path still handles it.
    unsafe { libc::fcntl(raw_fd, libc::F_SETPIPE_SZ, desired) };
}

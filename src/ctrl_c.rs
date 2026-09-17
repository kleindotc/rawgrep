use std::io;
use std::sync::{Arc, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::CURSOR_UNHIDE;

static RUNNING: OnceLock<Arc<AtomicBool>> = OnceLock::new();

#[cfg(unix)]
mod imp {
    use super::*;

    extern "C" fn handle_sigint(_sig: libc::c_int) {
        let running = RUNNING.get().expect("signal handler fired before init");

        running.store(false, Ordering::Relaxed);
        unsafe {
            libc::write(
                libc::STDOUT_FILENO,
                CURSOR_UNHIDE.as_ptr() as *const libc::c_void,
                CURSOR_UNHIDE.len(),
            );
            libc::_exit(0);
        }
    }

    #[inline(always)]
    pub fn install() {
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = handle_sigint as *const () as usize;

            libc::sigemptyset(&mut sa.sa_mask);
            sa.sa_flags = libc::SA_RESTART;

            if libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut()) != 0 {
                panic!("Error setting Ctrl-C handler: {}", io::Error::last_os_error());
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::{BOOL, TRUE, FALSE};
    use windows_sys::Win32::System::Console::{
        SetConsoleCtrlHandler, CTRL_C_EVENT, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT,
    };

    unsafe extern "system" fn handle_ctrl(ctrl_type: u32) -> BOOL {
        match ctrl_type {
            CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT => {
                let running = RUNNING.get().expect("signal handler fired before init");
                running.store(false, Ordering::Relaxed);
                {
                    let mut handle = io::stdout().lock();
                    _ = handle.write_all(CURSOR_UNHIDE.as_bytes());
                    _ = handle.flush();
                }
                std::process::exit(0);
            }

            _ => FALSE,
        }
    }

    #[inline(always)]
    pub fn install() {
        unsafe {
            if SetConsoleCtrlHandler(Some(handle_ctrl), TRUE) == 0 {
                panic!("Error setting Ctrl-C handler: {}", io::Error::last_os_error());
            }
        }
    }
}

/// Installs a SIGINT (Unix) / Ctrl-C (Windows) handler.
/// Returns a clone of the shared running flag.
///
/// # Panics
/// If called more than once (the OnceLock is already set).
#[inline]
pub fn setup_signal_handler() -> Arc<AtomicBool> {
    let running = Arc::new(AtomicBool::new(true));

    RUNNING.set(running.clone())
        .unwrap_or_else(|_| panic!("setup_signal_handler called more than once"));

    imp::install();
    running
}

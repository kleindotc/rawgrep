use std::sync::OnceLock;

#[cfg(not(feature = "no-logs"))]
pub fn log_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();

    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("RAWGREP_LOG").as_deref(),
            Ok("1") | Ok("true") | Ok("yes") | Ok("on")
        )
    })
}

#[cfg(feature = "no-logs")]
pub fn log_enabled() -> bool { false }

#[macro_export]
#[cfg(not(feature = "no-logs"))]
macro_rules! debug {
    ($($arg:tt)*) => {
        if $crate::logger::log_enabled() {
            eprintln!($($arg)*);
        }
    };
}

#[macro_export]
#[cfg(feature = "no-logs")]
macro_rules! debug {
    ($($arg:tt)*) => {};
}

#![allow(unused, unused_imports, dead_code, clippy::inline_always)]

#[cfg(feature = "tracy")]
pub use tracy_client::{span, plot, Client, PlotName};

#[cfg(not(feature = "tracy"))]
pub use noop::*;

#[cfg(not(feature = "tracy"))]
mod noop {
    /// No-op stand-in for `tracy_client::PlotName`.
    #[derive(Clone, Copy)]
    pub struct PlotName;

    /// No-op stand-in for `tracy_client::Client`.
    pub struct Client;

    impl Client {
        pub const fn running() -> Option<Self> { None }
        pub const fn start() -> Self { Client }
        pub const fn set_thread_name(&self, _s: &str) {}
        pub const fn message(&self, _s: &str, _depth: u16) {}
        pub const fn color_message(&self, _s: &str, _rgba: u32, _depth: u16) {}
        pub const fn plot(&self, _name: PlotName, _value: f64) {}
    }

    /// No-op stand-in for `tracy_client::span!`.
    #[macro_export]
    macro_rules! span { ($($tt:tt)*) => { () } }
    pub use crate::span;

    /// No-op stand-in for `tracy_client::plot!`.
    #[macro_export]
    macro_rules! plot { ($($tt:tt)*) => {} }
    pub use crate::plot;
}

/// # Safety
/// The string must be static or otherwise live for the entire program,
/// as Tracy stores the pointer internally.
///
/// # Panics
/// If no Tracy client is currently running (tracy feature only).
#[inline(always)]
pub unsafe fn set_thread_name(s: &str) {
    #[cfg(feature = "tracy")] {
        let c = Client::running().expect("set_thread_name without a running Client");
        c.set_thread_name(s);
    }
}

/// # Panics
/// If no Tracy client is currently running (tracy feature only).
#[inline(always)]
pub fn message(s: &str) {
    #[cfg(feature = "tracy")] {
        let c = Client::running().expect("message without a running Client");
        c.message(s, 0);
    }
}

/// # Panics
/// If no Tracy client is currently running (tracy feature only).
#[inline(always)]
pub fn message_color(s: &str, rgba: u32) {
    #[cfg(feature = "tracy")] {
        let c = Client::running().expect("message_color without a running Client");
        c.color_message(s, rgba, 0);
    }
}

/// NOTE: This function leaks memory when tracy is enabled,
/// @Incomplete:
///   Fork `tracy_client` and expose internal module so we can stop leaking memory
///   each `create_plot` call.
///
/// # Safety
/// The string must be static or otherwise live for the entire program,
/// as Tracy stores the pointer internally.
#[inline(always)]
pub fn create_plot(name: &str) -> PlotName {
    #[cfg(feature = "tracy")]
    {
        tracy_client::PlotName::new_leak(name.into())
    }
    #[cfg(not(feature = "tracy"))]
    {
        PlotName
    }
}

/// Record a plot sample for the given plot name.
///
/// # Panics
/// If no Tracy client is currently running (tracy feature only).
#[inline(always)]
pub fn plot_value(plot: PlotName, value: f64) {
    #[cfg(feature = "tracy")] {
        Client::running()
            .expect("plot_value called without a running Client")
            .plot(plot, value);
    }
}

/// Record a one-off plot value for a string name (creates plot if needed).
///
/// # Panics
/// If no Tracy client is currently running (tracy feature only).
#[inline(always)]
pub fn plot_named(name: &str, value: f64) {
    #[cfg(feature = "tracy")] {
        let client = Client::running().expect("plot_named called without a running Client");
        let plot_name = tracy_client::PlotName::new_leak(name.into());
        client.plot(plot_name, value);
    }
}

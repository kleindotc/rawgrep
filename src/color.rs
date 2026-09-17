use std::sync::OnceLock;

/// Detected color capability of the current output stream/terminal.
/// Computed exactly once per process via `color_support()`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorSupport {
    None,
    Basic,     // 16-color ANSI
    Ansi256,   // 256-color palette
    TrueColor, // 24-bit RGB
}

static COLOR_SUPPORT: OnceLock<ColorSupport> = OnceLock::new();

#[inline]
pub fn color_support() -> ColorSupport {
    *COLOR_SUPPORT.get_or_init(detect_color_support)
}

fn detect_color_support() -> ColorSupport {
    if !crate::cli::should_enable_ansi_coloring() {
        return ColorSupport::None;
    }

    // COLORTERM is the de facto signal for 24-bit support
    // (set by most modern terminals: iTerm2, kitty, alacritty, wezterm,
    // gnome-terminal, vscode's integrated terminal, etc.)
    if let Ok(colorterm) = std::env::var("COLORTERM") {
        let colorterm = colorterm.to_ascii_lowercase();
        if colorterm == "truecolor" || colorterm == "24bit" {
            return ColorSupport::TrueColor;
        }
    }

    if let Ok(term) = std::env::var("TERM") {
        if term.contains("256color") {
            return ColorSupport::Ansi256;
        }
        if term == "dumb" {
            return ColorSupport::None;
        }
    }

    ColorSupport::Basic
}

macro_rules! define_color {
    ($name:ident, $true:literal, $ansi256:literal, $basic:literal) => {
        pub mod $name {
            use super::{color_support, ColorSupport};

            const TRUE:    &str = concat!("\x1b[38;2;", $true, "m");
            const ANSI256: &str = concat!("\x1b[38;5;", $ansi256, "m");
            const BASIC:   &str = $basic;

            #[inline]
            pub fn code() -> &'static str {
                match color_support() {
                    ColorSupport::None => "",
                    ColorSupport::TrueColor => TRUE,
                    ColorSupport::Ansi256 => ANSI256,
                    ColorSupport::Basic => BASIC,
                }
            }
        }
    };
}

//                    r;g;b        256-idx  basic ANSI
define_color!(red,   "255;0;0",     "196", "\x1b[1;31m");   // BOLD RED match
define_color!(green, "100;180;140", "108", "\x1b[1;32m");   // sage file paths
define_color!(cyan,  "122;168;184", "109", "\x1b[1;36m");   // slate teal line numbers
define_color!(blue,  "110;135;185", "67",  "\x1b[1;34m");   // slate blue device/fs line

pub const COLOR_RESET: &str = "\x1b[0m";
pub const BOLD:        &str = "\x1b[1m";

#[macro_export]
macro_rules! eprintln_red {
    ($($arg:tt)*) => {{
        let code = $crate::color::red::code();
        if code.is_empty() {
            eprintln!($($arg)*);
        } else {
            eprintln!("{}{}{}", code, format_args!($($arg)*), $crate::color::COLOR_RESET);
        }
    }};
}

#[macro_export]
macro_rules! eprintln_green {
    ($($arg:tt)*) => {{
        let code = $crate::color::green::code();
        if code.is_empty() {
            eprintln!($($arg)*);
        } else {
            eprintln!("{}{}{}", code, format_args!($($arg)*), $crate::color::COLOR_RESET);
        }
    }};
}

#[macro_export]
macro_rules! eprintln_blue {
    ($($arg:tt)*) => {{
        let code = $crate::color::blue::code();
        if code.is_empty() {
            eprintln!($($arg)*);
        } else {
            eprintln!("{}{}{}", code, format_args!($($arg)*), $crate::color::COLOR_RESET);
        }
    }};
}

#[macro_export]
macro_rules! eprintln_cyan {
    ($($arg:tt)*) => {{
        let code = $crate::color::cyan::code();
        if code.is_empty() {
            eprintln!($($arg)*);
        } else {
            eprintln!("{}{}{}", code, format_args!($($arg)*), $crate::color::COLOR_RESET);
        }
    }};
}

#[macro_export]
macro_rules! eprint_red {
    ($($arg:tt)*) => {{
        let code = $crate::color::red::code();
        if code.is_empty() {
            eprint!($($arg)*);
        } else {
            eprint!("{}{}{}", code, format_args!($($arg)*), $crate::color::COLOR_RESET);
        }
    }};
}

#[macro_export]
macro_rules! eprint_green {
    ($($arg:tt)*) => {{
        let code = $crate::color::green::code();
        if code.is_empty() {
            eprint!($($arg)*);
        } else {
            eprint!("{}{}{}", code, format_args!($($arg)*), $crate::color::COLOR_RESET);
        }
    }};
}

#[macro_export]
macro_rules! eprint_blue {
    ($($arg:tt)*) => {{
        let code = $crate::color::blue::code();
        if code.is_empty() {
            eprint!($($arg)*);
        } else {
            eprint!("{}{}{}", code, format_args!($($arg)*), $crate::color::COLOR_RESET);
        }
    }};
}

#[macro_export]
macro_rules! eprint_cyan {
    ($($arg:tt)*) => {{
        let code = $crate::color::cyan::code();
        if code.is_empty() {
            eprint!($($arg)*);
        } else {
            eprint!("{}{}{}", code, format_args!($($arg)*), $crate::color::COLOR_RESET);
        }
    }};
}

#[macro_export]
macro_rules! eprint_rgb {
    ($r:expr, $g:expr, $b:expr, $($arg:tt)*) => {{
        if $crate::color::color_support() == $crate::color::ColorSupport::TrueColor {
            eprint!(
                "\x1b[38;2;{};{};{}m{}{}",
                $r, $g, $b,
                format_args!($($arg)*),
                $crate::color::COLOR_RESET
            );
        } else {
            eprint!($($arg)*);
        }
    }};
}

#[macro_export]
macro_rules! writeln_red {
    ($writer:expr, $($arg:tt)*) => {{
        let code = $crate::color::red::code();
        if code.is_empty() {
            writeln!($writer, $($arg)*)
        } else {
            writeln!($writer, "{}{}{}", code, format_args!($($arg)*), $crate::color::COLOR_RESET)
        }
    }};
}

#[macro_export]
macro_rules! writeln_green {
    ($writer:expr, $($arg:tt)*) => {{
        let code = $crate::color::green::code();
        if code.is_empty() {
            writeln!($writer, $($arg)*)
        } else {
            writeln!($writer, "{}{}{}", code, format_args!($($arg)*), $crate::color::COLOR_RESET)
        }
    }};
}

#[macro_export]
macro_rules! writeln_blue {
    ($writer:expr, $($arg:tt)*) => {{
        let code = $crate::color::blue::code();
        if code.is_empty() {
            writeln!($writer, $($arg)*)
        } else {
            writeln!($writer, "{}{}{}", code, format_args!($($arg)*), $crate::color::COLOR_RESET)
        }
    }};
}

#[macro_export]
macro_rules! writeln_cyan {
    ($writer:expr, $($arg:tt)*) => {{
        let code = $crate::color::cyan::code();
        if code.is_empty() {
            writeln!($writer, $($arg)*)
        } else {
            writeln!($writer, "{}{}{}", code, format_args!($($arg)*), $crate::color::COLOR_RESET)
        }
    }};
}

#[macro_export]
macro_rules! write_red {
    ($writer:expr, $($arg:tt)*) => {{
        let code = $crate::color::red::code();
        if code.is_empty() {
            write!($writer, $($arg)*)
        } else {
            write!($writer, "{}{}{}", code, format_args!($($arg)*), $crate::color::COLOR_RESET)
        }
    }};
}

#[macro_export]
macro_rules! write_green {
    ($writer:expr, $($arg:tt)*) => {{
        let code = $crate::color::green::code();
        if code.is_empty() {
            write!($writer, $($arg)*)
        } else {
            write!($writer, "{}{}{}", code, format_args!($($arg)*), $crate::color::COLOR_RESET)
        }
    }};
}

#[macro_export]
macro_rules! write_blue {
    ($writer:expr, $($arg:tt)*) => {{
        let code = $crate::color::blue::code();
        if code.is_empty() {
            write!($writer, $($arg)*)
        } else {
            write!($writer, "{}{}{}", code, format_args!($($arg)*), $crate::color::COLOR_RESET)
        }
    }};
}

#[macro_export]
macro_rules! write_cyan {
    ($writer:expr, $($arg:tt)*) => {{
        let code = $crate::color::cyan::code();
        if code.is_empty() {
            write!($writer, $($arg)*)
        } else {
            write!($writer, "{}{}{}", code, format_args!($($arg)*), $crate::color::COLOR_RESET)
        }
    }};
}

#[macro_export]
macro_rules! writeln_rgb {
    ($writer:expr, $r:expr, $g:expr, $b:expr, $($arg:tt)*) => {{
        if $crate::color::color_support() == $crate::color::ColorSupport::TrueColor {
            writeln!(
                $writer,
                "\x1b[38;2;{};{};{}m{}{}",
                $r, $g, $b,
                format_args!($($arg)*),
                $crate::color::COLOR_RESET
            )
        } else {
            writeln!($writer, $($arg)*)
        }
    }};
}

#[macro_export]
macro_rules! write_rgb {
    ($writer:expr, $r:expr, $g:expr, $b:expr, $($arg:tt)*) => {{
        if $crate::color::color_support() == $crate::color::ColorSupport::TrueColor {
            write!(
                $writer,
                "\x1b[38;2;{};{};{}m{}{}",
                $r, $g, $b,
                format_args!($($arg)*),
                $crate::color::COLOR_RESET
            )
        } else {
            write!($writer, $($arg)*)
        }
    }};
}

#[macro_export]
macro_rules! eprintln_rgb {
    ($r:expr, $g:expr, $b:expr, $($arg:tt)*) => {{
        if $crate::color::color_support() == $crate::color::ColorSupport::TrueColor {
            eprintln!(
                "\x1b[38;2;{};{};{}m{}{}",
                $r, $g, $b,
                format_args!($($arg)*),
                $crate::color::COLOR_RESET
            );
        } else {
            eprintln!($($arg)*);
        }
    }};
}

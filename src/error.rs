use std::fmt;
use std::io;

/// All recoverable failure modes rawgrep can encounter.
#[derive(Debug)]
pub enum Error {
    /// The regex / literal pattern supplied by the caller is not valid.
    InvalidPattern(Box<str>),

    /// The filesystem was identified, but rawgrep doesn't support searching it yet.
    UnsupportedFilesystem { device: Box<str>, fs: Box<str> },

    /// The requested path could not be canonicalized (doesn't exist, bad
    /// symlink, etc.).
    PathNotFound { path: Box<str>, source: io::Error },

    /// Auto-detection of the block device for a path failed.
    DeviceDetectionFailed(io::Error),

    /// The explicitly supplied (or auto-detected) device path does not exist.
    DeviceNotFound(Box<str>),

    /// The process lacks the privileges needed to open the raw device.
    /// Suggest `sudo` or `CAP_DAC_READ_SEARCH`.
    PermissionDenied(Box<str>),

    /// The device was opened but the on-disk magic doesn't match any known
    /// filesystem (ext4 / APFS / NTFS).
    UnknownFilesystem(Box<str>),

    /// The superblock / boot-sector data is present but structurally invalid.
    /// Includes a hint for ext4 (partition vs whole-disk confusion).
    InvalidFilesystem { fs: Box<str>, source: io::Error, hint: Option<Box<str>> },

    /// The search root path could not be located inside the filesystem image.
    RootNotFound { path: Box<str>, device: Box<str>, source: io::Error },

    /// The `Matcher` (regex engine) failed to initialise.
    MatcherInit(io::Error),

    /// Any other I/O error that escaped the above categories.
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidPattern(msg) => {
                write!(f, "{msg}")
            }
            Error::PathNotFound { path, source } => {
                write!(f, "Couldn't canonicalize '{path}': {source}")
            }
            Error::DeviceDetectionFailed(e) => {
                write!(f, "Couldn't auto-detect partition: {e}")
            }
            Error::DeviceNotFound(dev) => {
                write!(f, "Device or partition not found: '{dev}'")
            }
            Error::PermissionDenied(dev) => {
                write!(
                    f,
                    "Permission denied opening '{dev}'\n\
                     help: try running with sudo/root, or grant the binary \
                     CAP_DAC_READ_SEARCH:\n  \
                     sudo setcap cap_dac_read_search=eip <path-to-binary>"
                )
            }
            Error::UnsupportedFilesystem { device, fs } => {
                write!(f, "'{device}' has a {fs} filesystem, which rawgrep doesn't support yet")?;
                write!(f, "\nsupported filesystems: ext4, NTFS, APFS")
            }
            Error::UnknownFilesystem(dev) => {
                write!(f, "Couldn't identify a filesystem on '{dev}'")?;
                write!(
                    f,
                    "\nhint: Double-check this is the right partition (try `lsblk` or `sudo fdisk -l`) -- \
                     an unpartitioned disk, the wrong partition, or a filesystem rawgrep has no signature \
                     for will all land here"
                )
            }
            Error::InvalidFilesystem { fs, source, hint } => {
                write!(f, "Invalid {fs} filesystem: {source}")?;
                if let Some(h) = hint {
                    write!(f, "\n{h}")?;
                }
                Ok(())
            }
            Error::RootNotFound { path, device, source } => {
                write!(f, "Couldn't find '{path}' in '{device}': {source}")
            }
            Error::MatcherInit(e) => {
                write!(f, "Failed to build matcher: {e}")
            }
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {
    #[inline]
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::PathNotFound    { source, .. } => Some(source),
            Error::DeviceDetectionFailed(e)       => Some(e),
            Error::InvalidFilesystem { source, .. } => Some(source),
            Error::RootNotFound    { source, .. } => Some(source),
            Error::MatcherInit(e)                 => Some(e),
            Error::Io(e)                          => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    #[inline]
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

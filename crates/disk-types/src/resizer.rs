use crate::{FileSystem, Fscker};
use std::io;
use std::path::Path;
use std::process::ExitStatus;

pub trait PartitionResizer {
    fn flush(&self) -> Result<(), ResizeError>;

    fn grow(path: &Path, fs: FileSystem, current: u64, new: u64) -> Result<(), ResizeError>;

    fn shrink(path: &Path, fs: FileSystem, current: u64, new: u64) -> Result<(), ResizeError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ResizeError {
    #[error("shrinking not supported for {fs}")]
    ShrinkNotSupported { fs: FileSystem },
    #[error("growing not supported for {fs}")]
    GrowNotSupported { fs: FileSystem },
    #[error("I/O error occurred while shrinking: {0}")]
    Io(io::Error),
    #[error("command failed with exit status: {0}")]
    BadStatus(ExitStatus)
}

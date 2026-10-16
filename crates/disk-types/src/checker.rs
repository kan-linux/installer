use crate::fs::FileSystem;
use std::io;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

pub trait Fscker {
    fn fsck(path: &Path, fs: FileSystem) -> Result<(), FsckError>;
}

#[derive(Debug, thiserror::Error)]
pub enum FsckError {
    #[error("fsck I/O error: {0:?}")]
    Io(io::Error),
    #[error("command failed with exit status: {0}")]
    BadStatus(ExitStatus),
}

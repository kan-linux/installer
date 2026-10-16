use crate::{FileSystem, PartitionType};
use libparted::PartitionFlag;
use std::io;
use std::path::{Path, PathBuf};

pub struct NewPartition {
    pub start: u64,
    pub end: u64,
    pub fs: Option<FileSystem>,
    pub label: Option<String>,
    pub flags: Vec<PartitionFlag>,
    pub kind: PartitionType,
}

#[derive(Debug, thiserror::Error)]
pub enum PartitionError {
    #[error("failed to open disk: {0}")]
    OpenDisk(io::Error),
    #[error("failed to remove partition: {0}")]
    RemovePartition(io::Error),
    #[error("failed to commit to disk: {0}")]
    CommitToDisk(io::Error),
    #[error("failed to create partition: {0}")]
    CreatePartition(io::Error),
    #[error("failed to retrieve new partition info: {0}")]
    GetNewData(io::Error)

}

pub trait Partitioner {
    fn create(&mut self, device: &Path, data: NewPartition) -> Result<(), PartitionError>;

    fn delete(&mut self, device: &Path, number: u32) -> Result<Result<(Option<u32>, PathBuf), PartitionError>, PartitionError>;
}

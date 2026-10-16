use crate::auto::{InstallOptionError, RecoveryOption};
use crate::disks::Disks;
use crate::installer::RecoveryEnv;
use std::io;

#[derive(Debug, thiserror::Error)]
pub enum UpgradeError {
    #[error("attempted to recover from errors, but failed: {0}")]
    AttemptFailed(io::Error),
    #[error("failed to mount file systems to chroot: {0}")]
    ChrootMount(io::Error),
    #[error("failed to create temporary chroot mount directory: {0}")]
    ChrootTempCreate(io::Error),
    #[error("failed to configure disk(s): {0}")]
    Configure(InstallOptionError),
    #[error("failed to mount efivars directory: {0}")]
    EfiVars(io::Error),
    #[error("failed to mount $CHROOT/etc to /etc: {0}")]
    EtcMount(io::Error),
    #[error("failed to find the current loader entry in systemd-boot")]
    MissingCurrentEntry,
    #[error("attempted an upgrade, but the upgrade mode was not set")]
    ModeNotSet,
    #[error("systemd-boot loader conf error: {0}")]
    SystemdBootConf(systemd_boot_conf::Error),
    #[error("failed to remove upgrade flag from recovery.conf: {0}")]
    UpgradeFlag(io::Error),
    #[error("release upgrade is not supported on kanlinux")]
    Unsupported,
}

#[derive(Debug)]
pub enum UpgradeEvent<'a> {
    AttemptingRepair,
    AttemptingUpgrade,
    DpkgInfo(&'a str),
    DpkgErr(&'a str),
    UpgradeInfo(&'a str),
    UpgradeErr(&'a str),
    PackageProcessing(&'a str),
    PackageProgress(u8),
    PackageSettingUp(&'a str),
    PackageUnpacking { package: &'a str, version: &'a str, over: &'a str },
    ResumingUpgrade,
}

/// Chroot into an existing install, and upgrade it to the next release.
///
/// kanlinux has no apt/dpkg release-upgrade path; this entry point is kept for
/// API compatibility and always fails with `UpgradeError::Unsupported`.
pub fn upgrade<F: Fn(UpgradeEvent), R: Fn() -> bool>(
    _recovery_conf: &mut RecoveryEnv,
    _disks: &mut Disks,
    option: &RecoveryOption,
    _callback: F,
    _attempt_repair: R,
) -> Result<(), UpgradeError> {
    if option.mode.as_deref() != Some("upgrade") {
        return Err(UpgradeError::ModeNotSet);
    }

    Err(UpgradeError::Unsupported)
}

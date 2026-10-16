//! Stubs for the former Debian/Ubuntu package hooks.
//!
//! kanlinux has no package manager; these entry points remain only so the
//! configure pipeline keeps a single call site if packaging is ever reintroduced.
#![allow(dead_code)]

use crate::installer::traits::InstallerDiskOps;
use os_release::OsRelease;
use std::io;

/// kanlinux has no `check-language-support`; language packs are configured
/// statically in the rootfs.
pub fn check_language_support(_lang: &str, _chroot: &crate::chroot::Chroot<'_>) -> io::Result<Option<String>> {
    Ok(None)
}

/// kanlinux has no apt dependency resolver.
pub fn get_dependencies_from_list<P: AsRef<str>>(_deps: &[P]) -> Option<Vec<String>> {
    None
}

/// Bootloader packages are already present in the kanlinux rootfs.
pub fn get_bootloader_packages(_os_release: &OsRelease) -> io::Result<&'static [&'static str]> {
    Ok(&[])
}

/// Filesystem tools for the target layout are already present in the kanlinux rootfs.
pub fn get_required_packages<D: InstallerDiskOps>(
    _disks: &D,
    _release: &OsRelease,
) -> Vec<&'static str> {
    Vec::new()
}

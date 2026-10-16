use crate::chroot::Chroot;
use crate::disks::{Bootloader, Disks};
use crate::errors::IoContext;
use libc;
use os_release::OsRelease;
use std::{
    env,
    ffi::{OsStr, OsString},
    fs, io,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};
use crate::Config;
use crate::MODIFY_BOOT_ORDER;

use super::mount_efivars;

/// kanlinux builds its initramfs with its own tooling; `update-initramfs` is a
/// Debian tool and may not exist on the target. Skip it when absent.
fn update_initramfs_if_present(chroot: &mut Chroot) -> io::Result<()> {
    match chroot.command("update-initramfs", &["-c", "-k", "all"]).run() {
        Ok(()) => Ok(()),
        Err(ref e) if e.kind() == io::ErrorKind::NotFound => {
            info!("update-initramfs not present; skipping");
            Ok(())
        }
        Err(why) => Err(why),
    }
}

pub fn bootloader<F: FnMut(i32)>(
    disks: &Disks,
    mount_dir: &Path,
    bootloader: Bootloader,
    config: &Config,
    iso_os_release: &OsRelease,
    mut callback: F,
) -> io::Result<()> {
    // Obtain the root device & partition, with an optional EFI device & partition.
    let ((root_dev, _root_part), boot_opt) = disks.get_base_partitions(bootloader);

    let mut efi_part_num = 0;

    let bootloader_dev = boot_opt.map_or(root_dev, |(dev, dev_part)| {
        efi_part_num = dev_part.number;
        dev
    });

    info!("{}: installing bootloader for {:?}", bootloader_dev.display(), bootloader);

    {
        let efi_path = {
            let chroot = mount_dir.as_os_str().as_bytes();
            let mut target_mount: Vec<u8> = if chroot[chroot.len() - 1] == b'/' {
                chroot.to_owned()
            } else {
                let mut temp = chroot.to_owned();
                temp.push(b'/');
                temp
            };

            target_mount.extend_from_slice(b"boot/efi/");
            PathBuf::from(OsString::from_vec(target_mount))
        };

        // Also ensure that the /boot/efi directory is created.
        if bootloader == Bootloader::Efi && boot_opt.is_some() {
            fs::create_dir_all(&efi_path)
                .with_context(|err| format!("failed to create efi directory: {}", err))?;
        }

        {
            let mut chroot = Chroot::new(mount_dir)?;
            let efivars_mount = mount_efivars(&mount_dir)?;

            // kanlinux: 同时安装 BIOS(GRUB i386-pc) 与 EFI(GRUB x86_64-efi)。
            // 这样虚拟盘用 qemu 自带 SeaBIOS 即可启动，不必找 OVMF；
            // 真机 UEFI 仍可引导。
            let name = super::normalize_os_release_name(&iso_os_release.name);

            // 1) BIOS GRUB → 写入磁盘（需 BIOS boot 分区）
            info!("installing BIOS GRUB to {}", bootloader_dev.display());
            chroot
                .command(
                    "grub-install",
                    &[
                        "--target=i386-pc",
                        "--recheck",
                        bootloader_dev.to_str().unwrap(),
                    ],
                )
                .run()
                .with_context(|why| {
                    format!(
                        "BIOS grub-install to {} failed: {}. \
                         (Need BIOS boot partition + /usr/lib/grub/i386-pc)",
                        bootloader_dev.display(),
                        why
                    )
                })?;

            // 2) EFI GRUB → ESP
            let grub_efi_target = match env::consts::ARCH {
                "aarch64" => "arm64-efi",
                "x86_64" => "x86_64-efi",
                unknown => {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        format!("unsupported architecture {} for EFI", unknown),
                    ))
                }
            };

            if boot_opt.is_some() {
                info!("installing EFI GRUB to /boot/efi");
                chroot
                    .command(
                        "grub-install",
                        &[
                            &format!("--target={}", grub_efi_target),
                            "--efi-directory=/boot/efi",
                            &format!("--bootloader-id={}", name),
                            "--no-nvram",
                            "--recheck",
                        ],
                    )
                    .run()?;

                // UEFI fallback: 固件无 NVRAM 条目时只认 \EFI\BOOT\BOOTX64.EFI
                let fallback = match env::consts::ARCH {
                    "aarch64" => "BOOTAA64.EFI",
                    _ => "BOOTX64.EFI",
                };
                chroot
                    .command(
                        "sh",
                        &[
                            "-c",
                            &format!(
                                r#"set -e
espsrc="/boot/efi/EFI"
d="$espsrc/BOOT"
mkdir -p "$d"
src="$(find "$espsrc" -name 'grub*.efi' -type f | head -n1)"
if [ -z "$src" ]; then
    src="$(find "$espsrc" -name '*.efi' -type f | head -n1)"
fi
[ -n "$src" ] || {{ echo "no EFI binary found under $espsrc" >&2; exit 1; }}
cp -f "$src" "$d/{fallback}"
echo "fallback bootloader: $d/{fallback} <- $src"
"#,
                                fallback = fallback
                            ),
                        ],
                    )
                    .run()?;
            }

            // 先生成 initrd，再 grub-mkconfig，否则 grub.cfg 引用不到 initrd
            update_initramfs_if_present(&mut chroot)?;
            chroot.command("grub-mkconfig", &["-o", "/boot/grub/grub.cfg"]).run()?;

            if config.flags & MODIFY_BOOT_ORDER != 0 && boot_opt.is_some() {
                let efi_part_num = efi_part_num.to_string();
                let efi_arch = match env::consts::ARCH {
                    "aarch64" => "aa64",
                    "x86_64" => "x64",
                    unknown => {
                        return Err(io::Error::new(
                            io::ErrorKind::Other,
                            format!("unsupported architecture {} for EFI", unknown),
                        ))
                    }
                };
                let loader = format!("\\EFI\\{}\\grub{}.efi", name, efi_arch);

                let args: &[&OsStr] = &[
                    "--create".as_ref(),
                    "--disk".as_ref(),
                    bootloader_dev.as_ref(),
                    "--part".as_ref(),
                    efi_part_num.as_ref(),
                    "--write-signature".as_ref(),
                    "--label".as_ref(),
                    iso_os_release.pretty_name.as_ref(),
                    "--loader".as_ref(),
                    loader.as_ref(),
                ][..];

                chroot.command("efibootmgr", args).run()?;
            }

            // Sync to the disk before unmounting
            unsafe {
                libc::sync();
            }

            drop(efivars_mount);
            chroot.unmount(true)?;
        }
    }

    callback(99);

    Ok(())
}

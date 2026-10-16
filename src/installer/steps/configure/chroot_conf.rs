use crate::chroot::{Chroot, Command};
use crate::errors::{IoContext, IntoIoResult};
use crate::misc;
use partition_identity::PartitionID;
use proc_mounts::MountList;
use std::path::PathBuf;
use std::{
    fs,
    io::{self, Write},
    path::Path,
};
use crate::timezones::Region;
use crate::Config;

// For a reliable boot when using recovery, we show all output and do not use plymouth
const RECOVERY_BOOT_OPTIONS: &str = "";

pub struct ChrootConfigurator<'a> {
    chroot: Chroot<'a>,
}

impl<'a> ChrootConfigurator<'a> {
    pub fn new(chroot: Chroot<'a>) -> Self {
        Self { chroot }
    }

    /// Configure the bootloader on the system.
    ///
    /// kanlinux installs the bootloader in `steps::bootloader` (grub-install /
    /// grub-mkconfig). This hook is kept for API compatibility and does nothing.
    pub fn bootloader(&self) -> io::Result<()> {
        info!("bootloader will be installed by the bootloader step");
        Ok(())
    }

    /// Run the kanlinux post-install script inside the target.
    ///
    /// The script is embedded in the installer and written to `/tmp` on the
    /// target for the duration of the call.
    pub fn run_post_install(
        &self,
        config: &Config,
        user: Option<&crate::UserAccountCreate>,
        timezone: Option<&Region>,
    ) -> io::Result<()> {
        info!("running kanlinux post-install configuration");

        let script_path = self.chroot.path.join("tmp/kanlinux-configure.sh");
        fs::write(&script_path, include_str!("../../../../kanlinux-configure.sh"))
            .with_context(|err| format!("writing kanlinux-configure.sh: {}", err))?;

        // Timezone as "Area/City" from the region table, if provided.
        let tz = timezone.map(|r| {
            // Region's path is like /usr/share/zoneinfo/Asia/Shanghai
            r.path()
                .to_str()
                .and_then(|p| p.strip_prefix("/usr/share/zoneinfo/"))
                .unwrap_or("Asia/Shanghai")
                .to_owned()
        });

        let (username, password, fullname) = match user {
            Some(u) => (
                u.username.clone(),
                u.password.clone().unwrap_or_default(),
                u.realname.clone().unwrap_or_default(),
            ),
            None => (String::new(), String::new(), String::new()),
        };

        // The script requires a username; skip user setup when none was given.
        let mut command = self.chroot.command("sh", &["/tmp/kanlinux-configure.sh"]);
        command.env("KAN_HOSTNAME", &config.hostname);
        command.env("KAN_LOCALE", &config.lang);
        if let Some(tz) = tz {
            command.env("KAN_TIMEZONE", &tz);
        }
        if !username.is_empty() {
            command.env("KAN_USERNAME", &username);
            if !password.is_empty() {
                command.env("KAN_PASSWORD", &password);
            }
            if !fullname.is_empty() {
                command.env("KAN_FULLNAME", &fullname);
            }
        } else {
            // No user requested: use a placeholder so the script can still
            // apply hostname/locale/identity changes.
            command.env("KAN_USERNAME", "kan");
        }

        let result = command.run();
        let _ = fs::remove_file(&script_path);
        result
    }

    /// Create a new user account.
    pub fn create_user(
        &self,
        config: &Config,
        user: &str,
        pass: Option<&str>,
        fullname: Option<&str>,
        profile_icon: Option<&str>,
    ) -> io::Result<()> {
        let _ = config;
        // kanlinux: wheel grants sudo; audio/video/input/kvm/render for device access.
        // squashfs 里可能已有同名用户（如 kan），存在则只更新组，不再 useradd。
        {
            let mut cmd = self.chroot.command(
                "sh",
                &[
                    "-c",
                    // $1 = user, $2 = fullname (optional)
                    r#"if id "$1" >/dev/null 2>&1; then \
                           usermod -a -G wheel,audio,video,input,kvm,render "$1"; \
                       else \
                           if [ -n "$2" ]; then \
                               useradd -m -G wheel,audio,video,input,kvm,render -s /bin/bash -c "$2" "$1"; \
                           else \
                               useradd -m -G wheel,audio,video,input,kvm,render -s /bin/bash "$1"; \
                           fi; \
                       fi"#,
                    "sh",
                    user,
                    fullname.unwrap_or(""),
                ],
            );
            cmd.run()?;
        }

        // Set the password for the user.
        if let Some(pass) = pass {
            let pass = &[pass, "\n", pass, "\n"].concat();
            self.chroot.command("passwd", &[user]).stdin_input(pass).run()?;
        }

        // Copy the profile icon to `/var/lib/AccountsService/icons/{user}` and assign that in
        // the config file at `/var/lib/AccountsService/users/{user}`.
        if let Some(path) = profile_icon {
            let mut dest = self.chroot.path.join(&["var/lib/AccountsService/icons/", user].concat());

            if fs::copy(&path, &dest).is_err() {
                let _ = fs::remove_file(&dest);
                return Ok(());
            }

            dest = self.chroot.path.join(&["var/lib/AccountsService/users/", user].concat());

            if fs::write(&dest, fomat!(
                "[User]\n"
                "Icon=/var/lib/AccountsService/icons/" (user) "\n"
                "SystemAccount=false\n"
            )).is_err() {
                let _ = fs::remove_file(&dest);
            }
        }
        
        // Reset the shell in case a non-standard shell was used.
        _ = self.chroot.command("usermod", [
            "--shell",
            "/bin/bash",
            user
        ]).run();
        
        // Create a config for COSMIC if cosmic-comp is installed.
        if Path::new("/bin/cosmic-comp").exists() {
            let config_path = ["home/", user, "/.config/cosmic/com.system76.CosmicComp/v1/xkb_config"].concat();
            set_cosmic_xkb_config(config, self.chroot.path.join(config_path))?;
        }

        // Ensure the user's folder and cosmic xkb config file has the right permissions.
        self.chroot.command("chown", [
            "-R",
            &[user, ":", user].concat(),
            &["/home/", user, "/.config/"].concat(),
        ]).run()?;

        Ok(())
    }

    /// Disable the nvidia fallback service.
    pub fn disable_nvidia_fallback(&self) {
        info!("attempting to disable nvidia-fallback.service");
        let args = &["disable", "nvidia-fallback.service"];
        if let Err(why) = self.chroot.command("systemctl", args).run() {
            warn!("disabling nvidia-fallback.service failed: {}", why);
        }
    }

    /// Use locale-gen and update-locale to set the locale of the machine.
    ///
    /// kanlinux 没有 Debian 的 locale-gen/update-locale；locale 由
    /// `kanlinux-configure.sh` 写入 `/etc/locale.conf`。这里仅在工具存在时执行。
    pub fn generate_locale(&self, locale: &str) -> io::Result<()> {
        info!("generating locales via `locale-gen` and `update-locale`");
        let _ = locale; // chroot 环境已设 LC_ALL
        self.chroot
            .command("sh", &["-c",
                "command -v locale-gen >/dev/null 2>&1 || exit 0; \
                 locale-gen --purge \"$LC_ALL\"; \
                 if command -v update-locale >/dev/null 2>&1; then \
                     update-locale --reset \"LANG=$LC_ALL\"; \
                 fi"])
            .run()
            .with_context(|err| format!("failed to generate locales: {}", err))
    }

    /// Generate a new machine ID for /var/lib/dbus/machine-id
    pub fn generate_machine_id(&self) -> io::Result<()> {
        info!("generating machine id via `dbus-uuidgen`");
        self.chroot.command("sh", &["-c", "dbus-uuidgen > /etc/machine-id"]).run()?;
        self.chroot.command("ln", &["-sf", "/etc/machine-id", "/var/lib/dbus/machine-id"]).run()
    }

    /// Set the hostname of the new install.
    pub fn hostname(&self, hostname: &str) -> io::Result<()> {
        info!("setting hostname to {}", hostname);
        let hostfile = self.chroot.path.join("etc/hostname");
        let mut file = misc::create(&hostfile)?;
        writeln!(&mut file, "{}", hostname)
            .with_context(|err| format!("failed to write hostname to {:?}: {}", hostfile, err))
    }

    /// Create a default hosts file for the new install.
    pub fn hosts(&self, hostname: &str) -> io::Result<()> {
        info!("setting hosts file");
        let hosts = self.chroot.path.join("etc/hosts");
        let mut file = misc::create(&hosts)?;
        writeln!(
            &mut file,
            "# See `man hosts` for details.\n\
             127.0.0.1\tlocalhost\n\
             127.0.1.1\t{}\n\
             ::1\t\tlocalhost ip6-localhost ip6-loopback\n\
             ff02::1\t\tip6-allnodes\n\
             ff02::2\t\tip6-allrouters",
            hostname
        )
        .with_context(|err| format!("failed to write hosts to {:?}: {}", hosts, err))
    }

    /// Temporarily disable `update-initramfs` so package/script hooks don't
    /// regenerate initramfs during setup. kanlinux has no such tool — skip.
    pub fn initramfs_disable(&self) -> io::Result<()> {
        info!("symlinking update-initramfs to true for duration of initial setup");

        // 不存在则跳过（kanlinux 无 Debian 的 update-initramfs）
        self.chroot
            .command("sh", &["-c", "test -e /usr/sbin/update-initramfs || exit 0; \
                mv /usr/sbin/update-initramfs /usr/sbin/update-initramfs.bak && \
                ln -s /usr/bin/true /usr/sbin/update-initramfs"])
            .run()
            .with_context(|err| format!("failed to migrate `update-initramfs`: {}", err))
    }

    pub fn initramfs_reenable(&self) -> io::Result<()> {
        info!("re-enabling update-initramfs");
        self.chroot
            .command("sh", &["-c", "test -e /usr/sbin/update-initramfs.bak || exit 0; \
                rm -f /usr/sbin/update-initramfs; \
                mv /usr/sbin/update-initramfs.bak /usr/sbin/update-initramfs"])
            .run()
            .with_context(|err| format!("failed to restore backup of update-initramfs: {}", err))
    }

    /// Set the keyboard layout so that the layout will function, even within the decryption screen.
    ///
    /// kanlinux 不使用 Debian 的 console-setup；写入 `/etc/default/keyboard` 后，
    /// 仅在工具存在时刷新 console-setup。
    pub fn keyboard_layout(&self, config: &Config) -> io::Result<()> {
        info!("configuring keyboard layout");

        // Create a config for COSMIC if cosmic-comp is installed.
        if Path::new("/bin/cosmic-comp").exists() {
            set_cosmic_xkb_config(config, self.chroot.path.join("etc/cosmic/com.system76.CosmicComp/v1/xkb_config"))?;
        }

        // Write the keyboard layout to /etc/default/keyboard in the chroot.
        // kanlinux rootfs 可能没有 etc/default 目录。
        let keyboard_dir = self.chroot.path.join("etc/default");
        fs::create_dir_all(&keyboard_dir)
            .with_context(|err| format!("failed to create {:?}: {}", keyboard_dir, err))?;

        let keyboard_file = self.chroot.path.join("etc/default/keyboard");
        let mut file = misc::create(&keyboard_file)?;
        writeln!(&mut file, "XKBLAYOUT={}\nBACKSPACE=guess", config.keyboard_layout)
            .with_context(|err| {
                format!("failed to write keyboard layout to /etc/default/keyboard: {}", err)
            })?;

        if let Some(model) = config.keyboard_model.as_deref() {
            writeln!(&mut file, "XKBMODEL={}", model).with_context(|err| {
                format!("failed to write keyboard layout to /etc/default/keyboard: {}", err)
            })?;
        }

        if let Some(variant) = config.keyboard_variant.as_deref() {
            writeln!(&mut file, "XKBVARIANT={}", variant).with_context(|err| {
                format!("failed to write keyboard layout to /etc/default/keyboard: {}", err)
            })?;
        }

        // Debian console-setup 可选：kanlinux 无此工具则跳过
        self.chroot
            .command(
                "sh",
                &[
                    "-c",
                    "test -x /etc/init.d/console-setup.sh || exit 0; \
                     openvt -- /etc/init.d/console-setup.sh reload || true",
                ],
            )
            .run()
            .with_context(|err| format!("failed to reload console-setup: {}", err))?;

        let cached_file = self.chroot.path.join("etc/console-setup/cached.kmap.gz");
        if cached_file.exists() {
            fs::remove_file(cached_file)
                .with_context(|err| format!("failed to remove console-setup cache: {}", err))?;
        }

        // 同样仅在目标已有 console-setup 时做 symlink
        self.chroot
            .command(
                "sh",
                &[
                    "-c",
                    "test -d /etc/console-setup || exit 0; \
                     ln -sf /etc/console-setup/cached_UTF-8_del.kmap.gz /etc/console-setup/cached.kmap.gz || true",
                ],
            )
            .run()
    }

    /// In case the kernel is located outside of the squashfs image, find it.
    pub fn kernel_copy(&self) -> io::Result<()> {
        let cdrom_kernel = Path::new("/cdrom/casper/vmlinuz");
        let chroot_kernel = self.chroot.path.join("vmlinuz");

        if cdrom_kernel.exists() && !chroot_kernel.exists() {
            info!("copying kernel from /cdrom");
            self.chroot
                .command("sh", &["-c", "cp /cdrom/casper/vmlinuz \"$(realpath /vmlinuz)\""])
                .run()
        } else {
            Ok(())
        }
    }

    pub fn netresolve(&self) -> io::Result<()> {
        info!("creating /etc/resolv.conf");

        let resolvconf = "../run/systemd/resolve/stub-resolv.conf";
        self.chroot.command("ln", &["-sf", resolvconf, "/etc/resolv.conf"]).run()
    }

    pub fn recovery(
        &self,
        config: &Config,
        name: &str,
        root_uuid: &str,
        luks_uuid: &str,
    ) -> io::Result<()> {
        info!("creating recovery partition");
        let recovery_path = self.chroot.path.join("recovery");
        let efi_path = self.chroot.path.join("boot/efi");
        let live_mount_path = crate::live_mount_path();

        let result = if recovery_path.exists() { 0 } else { 1 }
            | if efi_path.is_dir() { 0 } else { 2 }
            | if Path::new(live_mount_path).is_dir() { 0 } else { 4 };

        if result != 0 {
            warn!(
                "{}, therefore no recovery partition will be created",
                if result & 1 != 0 {
                    format!("recovery at {} was not found", recovery_path.display())
                } else if result & 2 != 0 {
                    format!("no EFI partition found at {}", efi_path.display())
                } else {
                    "/cdrom was not found".into()
                }
            );
            return Ok(());
        }

        let mounts = MountList::new()?;
        let recovery_mount = mounts
            .get_mount_by_dest(&recovery_path)
            .into_io_result(|| "/recovery is mount not associated with block device")?;

        let efi_mount = mounts
            .get_mount_by_dest(&efi_path)
            .into_io_result(|| "efi is mount not associated with block device")?;

        let efi_partuuid = PartitionID::get_partuuid(&efi_mount.source)
            .into_io_result(|| "efi partiton does not have a PartUUID")?;

        let recovery_partuuid = PartitionID::get_partuuid(&recovery_mount.source)
            .into_io_result(|| "/recovery does not have a PartUUID")?;

        let recovery_uuid = PartitionID::get_uuid(&recovery_mount.source)
            .or_else(|| PartitionID::get_uuid(&efi_mount.source))
            .into_io_result(|| "/recovery does not have a UUID")?;

        let cdrom_uuid =
            Command::new("findmnt").args(&["-n", "-o", "UUID", live_mount_path]).run_with_stdout()?;
        let cdrom_uuid = cdrom_uuid.trim();

        // If we are installing from the recovery partition, then we can skip this step.
        if recovery_uuid.id == cdrom_uuid {
            return Ok(());
        }

        // Erase whatever is on the recovery partition currently
        if let Ok(dir) = recovery_path.read_dir() {
            for entry in dir.filter_map(Result::ok) {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_file() {
                        let _ = fs::remove_file(&entry.path());
                    } else if metadata.is_dir() {
                        let _ = fs::remove_dir_all(&entry.path());
                    }
                }
            }
        }

        let casper_data: String = if Path::new("/cdrom/recovery.conf").exists() {
            ["/cdrom/casper-", cdrom_uuid, "/"].concat()
        } else {
            String::from("/cdrom/casper/")
        };

        let casper = ["casper-", &recovery_uuid.id].concat();
        let recovery = ["Recovery-", &recovery_uuid.id].concat();
        if recovery_uuid.id != cdrom_uuid {
            self.chroot
                .command(
                    "rsync",
                    &[
                        "-KLavc",
                        "--delete-before",
                        "/cdrom/.disk",
                        "/cdrom/dists",
                        "/cdrom/pool",
                        "/recovery",
                    ],
                )
                .run()?;

            self.chroot
                .command("rsync", &["-KLavc", &casper_data, &["/recovery/", &casper].concat()])
                .run()?;
        }

        // Create recovery file.
        let recovery_data = format!(
            r#"HOSTNAME={}
LANG={}
KBD_LAYOUT={}
KBD_MODEL={}
KBD_VARIANT={}
EFI_UUID=PARTUUID={}
RECOVERY_UUID=PARTUUID={}
ROOT_UUID={}
LUKS_UUID={}
OEM_MODE=0
"#,
            config.hostname,
            config.lang,
            config.keyboard_layout,
            config.keyboard_model.as_deref().unwrap_or(""),
            config.keyboard_variant.as_deref().unwrap_or(""),
            efi_partuuid.id,
            recovery_partuuid.id,
            root_uuid,
            luks_uuid,
        );

        // Copy initrd and vmlinuz to EFI partition
        let recovery_path = self.chroot.path.join("recovery/recovery.conf");
        let mut recovery_file = misc::create(&recovery_path)?;
        recovery_file
            .write_all(recovery_data.as_bytes())
            .with_context(|err| format!("failed to write recovery file: {}", err))?;

        let efi_recovery = ["boot/efi/EFI/", recovery.as_str()].concat();
        let efi_initrd = self.chroot.path.join([&efi_recovery, "/initrd.gz"].concat());
        let efi_vmlinuz = self.chroot.path.join([&efi_recovery, "/vmlinuz.efi"].concat());

        fs::create_dir_all(self.chroot.path.join(efi_recovery))
            .with_context(|err| format!("failed to create EFI recovery directories: {}", err))?;

        misc::cp(&[&casper_data, "initrd.gz"].concat(), &efi_initrd)?;
        misc::cp(&[&casper_data, "vmlinuz.efi"].concat(), &efi_vmlinuz)?;

        // If the NVIDIA DKMS driver is installed, force it to load in the recovery partition
        // This test must not use /proc or /sys for detection since the installer can run inside a
        // chroot where those come from the host environment.
        let has_nvidia = Path::new("/var/lib/dkms/nvidia").exists();

        let rec_entry_data = format!(
            r#"title {0} recovery
linux /EFI/{1}/vmlinuz.efi
initrd /EFI/{1}/initrd.gz
options {2} boot=casper hostname=recovery userfullname=Recovery username=recovery live-media-path=/{3} live-media=/dev/disk/by-partuuid/{4} noprompt {5}
"#,
            name,
            recovery,
            RECOVERY_BOOT_OPTIONS,
            casper,
            recovery_partuuid.id,
            if has_nvidia { "modules_load=nvidia nvidia-drm.modeset=1" } else { "" }
        );
        let loader_entries = self.chroot.path.join("boot/efi/loader/entries/");
        if !loader_entries.exists() {
            fs::create_dir_all(&loader_entries)
                .with_context(|err| format!("failed to create EFI loader directories: {}", err))?;
        }

        let rec_entry_path = loader_entries.join([recovery.as_str(), ".conf"].concat());
        let mut rec_entry_file = misc::create(&rec_entry_path)?;
        rec_entry_file
            .write_all(rec_entry_data.as_bytes())
            .with_context(|err| format!("failed to write recovery EFI entry: {}", err))?;
        Ok(())
    }

    pub fn timezone(&self, region: &Region) -> io::Result<()> {
        self.chroot.command("rm", &["-f", "/etc/timezone"]).run()?;

        // ln -s 而不是 ln：硬链接会让 /etc/timezone 与 zoneinfo 共用 inode，
        // 之后 echo > /etc/timezone 会写穿并毁掉 TZif（时区回落 UTC）。
        let args: &[&str] = &["-s"];
        self.chroot.command("ln", args).arg(region.path()).arg("/etc/timezone").run()
    }

    /// Refresh the target's initramfs after configuration (bootloader drivers, etc.).
    #[allow(dead_code)]
    pub fn update_initramfs(&self) -> io::Result<()> {
        self.chroot
            .command("update-initramfs", &["-u"])
            .run()
            .with_context(|why| format!("failed to update initramfs: {}", why))
    }
}


fn set_cosmic_xkb_config(config: &Config, path: PathBuf) -> io::Result<()> {
    info!("installing xkb_config for cosmic at `{}`", path.display());

    if path.exists() {
        _ = std::fs::remove_file(&path);
    }
    
    _ = std::fs::create_dir_all(path.parent().unwrap());
    let mut cosmic_xkb_file = misc::create(&path)?;

    writeln!(
        &mut cosmic_xkb_file,
        r#"(
    rules: "",
    model: "{}",
    layout: "{}",
    variant: "{}",
    options: Some("lv3:ralt_switch,compose:rctrl"),
    repeat_delay: 600,
    repeat_rate: 25,
)"#,
        config.keyboard_model.as_ref().map(String::as_str).unwrap_or_default(),
        config.keyboard_layout,
        config.keyboard_variant.as_ref().map(String::as_str).unwrap_or_default()
    )
    .with_context(|err| {
        format!("failed to write keyboard layout to {}: {}", path.display(), err)
    })
}

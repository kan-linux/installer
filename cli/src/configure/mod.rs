mod decrypt;
mod lvm;
mod moved;
mod new;
mod removed;
mod reuse;
mod table;

use self::{decrypt::*, lvm::*, moved::*, new::*, removed::*, reuse::*, table::*};

use super::*;
use errors::DistinstError;

pub(crate) fn configure_disks(matches: &ArgMatches) -> Result<Disks, DistinstError> {
    // Whole-disk erase install: GPT + ESP + root. Skips the fine-grained
    // partition flags used by the original distinst CLI.
    if let Some(device) = matches.value_of("erase") {
        eprintln!("distinst: erase-and-install to {}", device);
        return erase_whole_disk(device);
    }

    let mut disks = Disks::default();

    {
        let disks = &mut disks;

        for block in matches.values_of("disk").unwrap() {
            eprintln!("distinst: adding {} to disks configuration", block);
            disks.add(Disk::from_name(block)?);
        }

        tables(disks, matches.values_of("table"))
            .and_then(|_| removed(disks, matches.values_of("delete")))
            .and_then(|_| moved(disks, matches.values_of("move")))
            .and_then(|_| reused(disks, matches.values_of("use")))
            .and_then(|_| new(disks, matches.values_of("new")))
            .and_then(|_| initialize_logical(disks))
            .and_then(|_| decrypt(disks, matches.values_of("decrypt")))
            .and_then(|_| {
                lvm(
                    disks,
                    matches.values_of("logical"),
                    matches.values_of("logical-modify"),
                    matches.values_of("logical-remove"),
                    matches.is_present("logical-remove-all"),
                )
            })?;

        eprintln!("distinst: disks configured");
    }

    Ok(disks)
}

/// Build a single-disk layout: GPT, BIOS-boot + 512 MiB ESP, rest ext4 root.
/// BIOS-boot 分区让 GRUB i386-pc 可写入，虚拟盘用 SeaBIOS 即可启动（无需 OVMF）。
fn erase_whole_disk(device: &str) -> Result<Disks, DistinstError> {
    use distinst::{
        DiskExt, FileSystem, PartitionBuilder, PartitionFlag, PartitionTable, PartitionType,
        Sector, SectorExt,
    };

    let mut disk = Disk::from_name(device)?;
    eprintln!("distinst: creating GPT on {}", device);
    disk.mklabel(PartitionTable::Gpt)?;

    let start = disk.get_sector(Sector::Start);
    // 1 MiB BIOS boot（无文件系统，供 grub i386-pc 写 core.img）
    let bios_end = start + 2048;
    let esp_end = bios_end + 1_024_000;
    let root_end = disk.get_sector(Sector::End);

    eprintln!("distinst: bios-boot {}..{} (no fs)", start, bios_end);
    disk.add_partition(
        PartitionBuilder::new(start, bios_end, None::<FileSystem>)
            .partition_type(PartitionType::Primary)
            .flag(PartitionFlag::PED_PARTITION_BIOS_GRUB),
    )?;

    eprintln!("distinst: ESP {}..{} (FAT32, /boot/efi)", bios_end, esp_end);
    disk.add_partition(
        PartitionBuilder::new(bios_end, esp_end, FileSystem::Fat32)
            .partition_type(PartitionType::Primary)
            .flag(PartitionFlag::PED_PARTITION_ESP)
            .mount("/boot/efi".into()),
    )?;

    eprintln!("distinst: root {}..{} (ext4, /)", esp_end, root_end);
    disk.add_partition(
        PartitionBuilder::new(esp_end, root_end, FileSystem::Ext4)
            .partition_type(PartitionType::Primary)
            .mount("/".into()),
    )?;

    let mut disks = Disks::default();
    disks.add(disk);
    Ok(disks)
}

fn initialize_logical(disks: &mut Disks) -> Result<(), DistinstError> {
    eprintln!("distinst: initializing LVM groups");
    disks.initialize_volume_groups().map_err(|why| DistinstError::InitializeVolumes { why })
}

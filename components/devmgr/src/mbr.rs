// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! MBR partition table parsing.

/// Logical sector size used by MBR partition entries.
pub const SECTOR_SIZE: u64 = 512;
const TABLE_OFFSET: usize = 446;
const ENTRY_SIZE: usize = 16;
const ENTRIES: usize = 4;
const SIGNATURE_OFFSET: usize = 510;

// REFERENCE: https://www.uefi.org/sites/default/files/resources/UEFI_Spec_2_10_Aug29.pdf
// REFERENCE: https://en.wikipedia.org/wiki/Master_boot_record#Partition_table_entries

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Partition {
    /// Byte offset of the first sector in the containing disk.
    pub start: u64,
    /// Length of the partition in bytes.
    pub length: u64,
}

/// Parses the four primary partition entries of an MBR sector.
///
/// Returns an empty table when the sector does not carry the `0x55AA` signature.
/// Entries with an unknown status, a zero type, or a range outside `capacity` are
/// ignored, so a corrupt table cannot grant access outside the disk.
pub fn parse(sector: &[u8], capacity: u64) -> [Option<Partition>; ENTRIES] {
    let mut partitions = [None; ENTRIES];
    if sector.len() < SIGNATURE_OFFSET + 2
        || sector[SIGNATURE_OFFSET..SIGNATURE_OFFSET + 2] != [0x55, 0xaa]
    {
        return partitions;
    }
    let disk_sectors = capacity / SECTOR_SIZE;
    for (index, entry) in sector[TABLE_OFFSET..TABLE_OFFSET + ENTRIES * ENTRY_SIZE]
        .as_chunks::<ENTRY_SIZE>()
        .0
        .iter()
        .enumerate()
    {
        let status = entry[0];
        let kind = entry[4];
        let start = u64::from(u32::from_le_bytes(entry[8..12].try_into().unwrap()));
        let sectors = u64::from(u32::from_le_bytes(entry[12..16].try_into().unwrap()));
        if !matches!(status, 0x00 | 0x80)
            || kind == 0
            || start == 0
            || sectors == 0
            || start
                .checked_add(sectors)
                .is_none_or(|end| end > disk_sectors)
        {
            continue;
        }
        partitions[index] = Some(Partition {
            start: start * SECTOR_SIZE,
            length: sectors * SECTOR_SIZE,
        });
    }
    partitions
}

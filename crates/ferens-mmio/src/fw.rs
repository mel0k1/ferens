use std::fs;
use std::io;
use std::path::Path;

use ferens_core::Chip;

pub const FIRMWARE_DIR: &str = "/lib/firmware/amdgpu";
pub const FIRMWARE_DIR_ALT: &str = "/usr/lib/firmware/amdgpu";

const GFX_HEADER_VERSION_MAJOR: u16 = 1;
const RLC_HEADER_VERSION_MAJORS: [u16; 2] = [1, 2];
const HEADER_MIN_BYTES: usize = 0x2C;
const HEADER_VERSION_OFFSET: usize = 0x08;
const UCODE_VERSION_OFFSET: usize = 0x10;
const UCODE_SIZE_OFFSET: usize = 0x14;
const UCODE_ARRAY_OFFSET: usize = 0x18;
const FEATURE_VERSION_OFFSET: usize = 0x20;
const REG_RESTORE_LIST_SIZE_OFFSET: usize = 0x38;
const REG_LIST_FORMAT_START_OFFSET: usize = 0x3C;
const STARTING_OFFSETS_START_OFFSET: usize = 0x44;
const REG_LIST_FORMAT_SIZE_OFFSET: usize = 0x48;
const REG_LIST_FORMAT_ARRAY_OFFSET: usize = 0x4C;
const REG_LIST_SIZE_OFFSET: usize = 0x50;
const REG_LIST_ARRAY_OFFSET: usize = 0x54;
const RLC_LIST_HEADER_MIN_BYTES: usize = 0x58;

const CRC32_POLY: u32 = 0xedb8_8320;

const fn crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 {
                (value >> 1) ^ CRC32_POLY
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

static CRC32_TABLE: [u32; 256] = crc32_table();

pub fn crc32(words: &[u32]) -> u32 {
    let mut crc = 0xffff_ffff;
    for word in words {
        for byte in word.to_le_bytes() {
            crc = CRC32_TABLE[((crc ^ u32::from(byte)) & 0xff) as usize] ^ (crc >> 8);
        }
    }
    !crc
}

pub struct UcodeFirmware {
    pub version: u32,
    pub feature_version: u32,
    pub words: Vec<u32>,
}

pub struct RlcRegisterList {
    pub format: Vec<u32>,
    pub restore: Vec<u32>,
    pub format_start: u32,
    pub restore_list_size: u32,
    pub offsets_start: u32,
}

pub struct FirmwareSet {
    pub mec1: UcodeFirmware,
    pub mec2: Option<UcodeFirmware>,
    pub rlc: UcodeFirmware,
    pub rlc_list: Option<RlcRegisterList>,
}

pub fn chip_prefix(chip: Chip) -> io::Result<&'static str> {
    match chip {
        Chip::Polaris10 => Ok("polaris10"),
        Chip::Polaris11 => Ok("polaris11"),
        Chip::Polaris12 => Ok("polaris12"),
        _ => Err(io::Error::other(
            "direct cp queue supports polaris10, polaris11 and polaris12",
        )),
    }
}

fn read_word(data: &[u8], offset: usize) -> io::Result<u32> {
    if offset + 4 > data.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "firmware truncated",
        ));
    }
    Ok(u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]))
}

fn parse_ucode(data: &[u8], header_majors: &[u16]) -> io::Result<UcodeFirmware> {
    let header_size = read_word(data, 0x04)? as usize;
    let version_major = read_word(data, HEADER_VERSION_OFFSET)? as u16;
    let version = read_word(data, UCODE_VERSION_OFFSET)?;
    let ucode_size = read_word(data, UCODE_SIZE_OFFSET)? as usize;
    let array_offset = read_word(data, UCODE_ARRAY_OFFSET)? as usize;
    let feature_version = read_word(data, FEATURE_VERSION_OFFSET)?;
    if !header_majors.contains(&version_major) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported firmware header version",
        ));
    }
    if header_size < HEADER_MIN_BYTES || header_size > data.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad header size",
        ));
    }
    if ucode_size == 0 || !ucode_size.is_multiple_of(4) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad ucode size"));
    }
    if array_offset < header_size || array_offset + ucode_size > data.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ucode array out of file bounds",
        ));
    }
    let mut words = Vec::with_capacity(ucode_size / 4);
    for index in 0..ucode_size / 4 {
        words.push(read_word(data, array_offset + index * 4)?);
    }
    Ok(UcodeFirmware {
        version,
        feature_version,
        words,
    })
}

pub fn parse_gfx_firmware(data: &[u8]) -> io::Result<UcodeFirmware> {
    parse_ucode(data, &[GFX_HEADER_VERSION_MAJOR])
}

pub fn parse_rlc_firmware(data: &[u8]) -> io::Result<UcodeFirmware> {
    parse_ucode(data, &RLC_HEADER_VERSION_MAJORS)
}

pub fn parse_rlc_register_list(data: &[u8]) -> io::Result<Option<RlcRegisterList>> {
    let header_size = read_word(data, 0x04)? as usize;
    if header_size < RLC_LIST_HEADER_MIN_BYTES || header_size > data.len() {
        return Ok(None);
    }
    let format_size = read_word(data, REG_LIST_FORMAT_SIZE_OFFSET)? as usize;
    let format_offset = read_word(data, REG_LIST_FORMAT_ARRAY_OFFSET)? as usize;
    let restore_size = read_word(data, REG_LIST_SIZE_OFFSET)? as usize;
    let restore_offset = read_word(data, REG_LIST_ARRAY_OFFSET)? as usize;
    if format_size == 0 && restore_size == 0 {
        return Ok(None);
    }
    if !format_size.is_multiple_of(4) || !restore_size.is_multiple_of(4) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "rlc register list is not word aligned",
        ));
    }
    if format_offset + format_size > data.len() || restore_offset + restore_size > data.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "rlc register list out of file bounds",
        ));
    }
    let mut format = Vec::with_capacity(format_size / 4);
    for index in 0..format_size / 4 {
        format.push(read_word(data, format_offset + index * 4)?);
    }
    let mut restore = Vec::with_capacity(restore_size / 4);
    for index in 0..restore_size / 4 {
        restore.push(read_word(data, restore_offset + index * 4)?);
    }
    Ok(Some(RlcRegisterList {
        format,
        restore,
        format_start: read_word(data, REG_LIST_FORMAT_START_OFFSET)?,
        restore_list_size: read_word(data, REG_RESTORE_LIST_SIZE_OFFSET)?,
        offsets_start: read_word(data, STARTING_OFFSETS_START_OFFSET)?,
    }))
}

type ParseFn = fn(&[u8]) -> io::Result<UcodeFirmware>;

fn firmware_path(dir: &Path, prefix: &str, suffix: &str) -> io::Result<Vec<u8>> {
    let path = dir.join(format!("{prefix}_{suffix}.bin"));
    fs::read(&path).map_err(|error| io::Error::other(format!("{}: {error}", path.display())))
}

fn load_file(dir: &Path, prefix: &str, suffix: &str, parse: ParseFn) -> io::Result<UcodeFirmware> {
    let data = firmware_path(dir, prefix, suffix)?;
    parse(&data)
}

fn load_required(
    dir: &Path,
    dir_alt: &Path,
    prefix: &str,
    suffix: &str,
    parse: ParseFn,
) -> io::Result<UcodeFirmware> {
    load_file(dir, prefix, suffix, parse).or_else(|error| {
        if dir_alt.join(format!("{prefix}_{suffix}.bin")).exists() {
            load_file(dir_alt, prefix, suffix, parse)
        } else {
            Err(error)
        }
    })
}

fn load_optional(dir: &Path, dir_alt: &Path, prefix: &str, suffix: &str) -> Option<UcodeFirmware> {
    load_file(dir, prefix, suffix, parse_gfx_firmware)
        .ok()
        .or_else(|| load_file(dir_alt, prefix, suffix, parse_gfx_firmware).ok())
}

pub fn load(chip: Chip) -> io::Result<FirmwareSet> {
    let prefix = chip_prefix(chip)?;
    let dir = Path::new(FIRMWARE_DIR);
    let dir_alt = Path::new(FIRMWARE_DIR_ALT);
    let mec1 = load_required(dir, dir_alt, prefix, "mec", parse_gfx_firmware)?;
    let mec2 = load_optional(dir, dir_alt, prefix, "mec2");
    let rlc_bytes = read_firmware(dir, dir_alt, prefix, "rlc")?;
    let rlc = parse_rlc_firmware(&rlc_bytes)?;
    let rlc_list = parse_rlc_register_list(&rlc_bytes)?;
    Ok(FirmwareSet {
        mec1,
        mec2,
        rlc,
        rlc_list,
    })
}

pub fn load_sdma(chip: Chip) -> io::Result<[UcodeFirmware; 2]> {
    let prefix = chip_prefix(chip)?;
    let dir = Path::new(FIRMWARE_DIR);
    let dir_alt = Path::new(FIRMWARE_DIR_ALT);
    let instance0 = load_required(dir, dir_alt, prefix, "sdma", parse_gfx_firmware)?;
    let instance1 = load_required(dir, dir_alt, prefix, "sdma1", parse_gfx_firmware)?;
    Ok([instance0, instance1])
}

fn read_firmware(dir: &Path, dir_alt: &Path, prefix: &str, suffix: &str) -> io::Result<Vec<u8>> {
    let path = dir.join(format!("{prefix}_{suffix}.bin"));
    match fs::read(&path) {
        Ok(data) => Ok(data),
        Err(error) => {
            let alt = dir_alt.join(format!("{prefix}_{suffix}.bin"));
            if alt.exists() {
                fs::read(&alt).map_err(|e| io::Error::other(format!("{}: {e}", alt.display())))
            } else {
                Err(io::Error::other(format!("{}: {error}", path.display())))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_bytes(ucode: &[u32], header_size: usize) -> Vec<u8> {
        let mut data = vec![0u8; header_size];
        data[0x04..0x08].copy_from_slice(&(header_size as u32).to_le_bytes());
        data[0x08..0x0A].copy_from_slice(&GFX_HEADER_VERSION_MAJOR.to_le_bytes());
        data[0x0A..0x0C].copy_from_slice(&0u16.to_le_bytes());
        data[UCODE_VERSION_OFFSET..UCODE_VERSION_OFFSET + 4]
            .copy_from_slice(&0x0001_0203u32.to_le_bytes());
        data[UCODE_SIZE_OFFSET..UCODE_SIZE_OFFSET + 4]
            .copy_from_slice(&((ucode.len() * 4) as u32).to_le_bytes());
        data[UCODE_ARRAY_OFFSET..UCODE_ARRAY_OFFSET + 4]
            .copy_from_slice(&(header_size as u32).to_le_bytes());
        data[FEATURE_VERSION_OFFSET..FEATURE_VERSION_OFFSET + 4]
            .copy_from_slice(&0x0000_0042u32.to_le_bytes());
        for word in ucode {
            data.extend_from_slice(&word.to_le_bytes());
        }
        data
    }

    fn rlc_header_bytes(ucode: &[u32]) -> Vec<u8> {
        let header_size = 0x68usize;
        let array_offset = 0x100usize;
        let mut data = vec![0u8; array_offset];
        data[0x04..0x08].copy_from_slice(&(header_size as u32).to_le_bytes());
        data[0x08..0x0A].copy_from_slice(&2u16.to_le_bytes());
        data[UCODE_VERSION_OFFSET..UCODE_VERSION_OFFSET + 4]
            .copy_from_slice(&0x11Eu32.to_le_bytes());
        data[UCODE_SIZE_OFFSET..UCODE_SIZE_OFFSET + 4]
            .copy_from_slice(&((ucode.len() * 4) as u32).to_le_bytes());
        data[UCODE_ARRAY_OFFSET..UCODE_ARRAY_OFFSET + 4]
            .copy_from_slice(&(array_offset as u32).to_le_bytes());
        data[FEATURE_VERSION_OFFSET..FEATURE_VERSION_OFFSET + 4]
            .copy_from_slice(&1u32.to_le_bytes());
        for word in ucode {
            data.extend_from_slice(&word.to_le_bytes());
        }
        data
    }

    #[test]
    fn parse_extracts_ucode_words() {
        let data = header_bytes(&[0xDEAD_BEEF, 0x1234_5678], 0x2C);
        let firmware = parse_gfx_firmware(&data).unwrap();
        assert_eq!(firmware.version, 0x0001_0203);
        assert_eq!(firmware.feature_version, 0x0000_0042);
        assert_eq!(firmware.words, vec![0xDEAD_BEEF, 0x1234_5678]);
    }

    #[test]
    fn parse_rejects_wrong_header_version() {
        let mut data = header_bytes(&[1], 0x2C);
        data[8] = 9;
        assert!(parse_gfx_firmware(&data).is_err());
    }

    #[test]
    fn parse_rejects_truncated_payload() {
        let data = header_bytes(&[1, 2, 3], 0x2C);
        assert!(parse_gfx_firmware(&data[..data.len() - 4]).is_err());
    }

    #[test]
    fn parse_rejects_unaligned_ucode_size() {
        let mut data = header_bytes(&[1], 0x2C);
        let size = (4 + 2) as u32;
        data[UCODE_SIZE_OFFSET..UCODE_SIZE_OFFSET + 4].copy_from_slice(&size.to_le_bytes());
        assert!(parse_gfx_firmware(&data).is_err());
    }

    #[test]
    fn parse_rejects_array_outside_file() {
        let mut data = header_bytes(&[1], 0x2C);
        data[UCODE_ARRAY_OFFSET..UCODE_ARRAY_OFFSET + 4].copy_from_slice(&0x1000u32.to_le_bytes());
        assert!(parse_gfx_firmware(&data).is_err());
    }

    #[test]
    fn parse_rejects_oversized_header() {
        let data = header_bytes(&[1], 0x2C);
        let mut truncated = data[..0x20].to_vec();
        truncated.extend_from_slice(&[1, 2, 3, 4]);
        assert!(parse_gfx_firmware(&truncated).is_err());
    }

    #[test]
    fn parse_rlc_accepts_v2_header_like_polaris_bins() {
        let data = rlc_header_bytes(&[0xBEEF_0001, 0xBEEF_0002]);
        let firmware = parse_rlc_firmware(&data).unwrap();
        assert_eq!(firmware.version, 0x11E);
        assert_eq!(firmware.feature_version, 1);
        assert_eq!(firmware.words, vec![0xBEEF_0001, 0xBEEF_0002]);
    }

    #[test]
    fn parse_rlc_accepts_v1_header() {
        let data = header_bytes(&[7], 0x34);
        assert!(parse_rlc_firmware(&data).is_ok());
    }

    #[test]
    fn parse_rlc_rejects_unknown_header_version() {
        let mut data = rlc_header_bytes(&[1]);
        data[8] = 3;
        assert!(parse_rlc_firmware(&data).is_err());
    }

    #[test]
    fn parse_gfx_rejects_rlc_v2_header() {
        let data = rlc_header_bytes(&[1]);
        assert!(parse_gfx_firmware(&data).is_err());
    }

    #[test]
    fn parse_rlc_rejects_truncated_payload() {
        let data = rlc_header_bytes(&[1, 2, 3]);
        assert!(parse_rlc_firmware(&data[..data.len() - 4]).is_err());
    }

    #[test]
    fn chip_prefix_covers_polaris_only() {
        assert_eq!(chip_prefix(Chip::Polaris10).unwrap(), "polaris10");
        assert_eq!(chip_prefix(Chip::Polaris12).unwrap(), "polaris12");
        assert!(chip_prefix(Chip::VegaM).is_err());
        assert!(chip_prefix(Chip::Unknown(0x1234)).is_err());
    }

    fn rlc_v2_list_bytes(format: &[u32], restore: &[u32]) -> Vec<u8> {
        let header_size = 0x68usize;
        let mut data = vec![0u8; header_size];
        data[0x04..0x08].copy_from_slice(&(header_size as u32).to_le_bytes());
        data[0x08..0x0A].copy_from_slice(&2u16.to_le_bytes());
        data[UCODE_VERSION_OFFSET..UCODE_VERSION_OFFSET + 4]
            .copy_from_slice(&0x11Eu32.to_le_bytes());
        data[UCODE_SIZE_OFFSET..UCODE_SIZE_OFFSET + 4].copy_from_slice(&4u32.to_le_bytes());
        data[UCODE_ARRAY_OFFSET..UCODE_ARRAY_OFFSET + 4]
            .copy_from_slice(&(header_size as u32).to_le_bytes());
        data[FEATURE_VERSION_OFFSET..FEATURE_VERSION_OFFSET + 4]
            .copy_from_slice(&1u32.to_le_bytes());
        data[REG_RESTORE_LIST_SIZE_OFFSET..REG_RESTORE_LIST_SIZE_OFFSET + 4]
            .copy_from_slice(&3u32.to_le_bytes());
        data[REG_LIST_FORMAT_START_OFFSET..REG_LIST_FORMAT_START_OFFSET + 4]
            .copy_from_slice(&0x40u32.to_le_bytes());
        data[STARTING_OFFSETS_START_OFFSET..STARTING_OFFSETS_START_OFFSET + 4]
            .copy_from_slice(&0x50u32.to_le_bytes());
        data[REG_LIST_FORMAT_SIZE_OFFSET..REG_LIST_FORMAT_SIZE_OFFSET + 4]
            .copy_from_slice(&((format.len() * 4) as u32).to_le_bytes());
        data[REG_LIST_FORMAT_ARRAY_OFFSET..REG_LIST_FORMAT_ARRAY_OFFSET + 4]
            .copy_from_slice(&(header_size as u32).to_le_bytes());
        data[REG_LIST_SIZE_OFFSET..REG_LIST_SIZE_OFFSET + 4]
            .copy_from_slice(&((restore.len() * 4) as u32).to_le_bytes());
        data[REG_LIST_ARRAY_OFFSET..REG_LIST_ARRAY_OFFSET + 4]
            .copy_from_slice(&((header_size + format.len() * 4) as u32).to_le_bytes());
        for word in format.iter().chain(restore.iter()) {
            data.extend_from_slice(&word.to_le_bytes());
        }
        data
    }

    #[test]
    fn parse_rlc_register_list_extracts_format_and_restore() {
        let data = rlc_v2_list_bytes(
            &[0x1111_1111, 0xFFFF_FFFF, 0x2222_2222],
            &[0xAAAA_0001, 0xAAAA_0002],
        );
        let list = parse_rlc_register_list(&data).unwrap().unwrap();
        assert_eq!(list.format, vec![0x1111_1111, 0xFFFF_FFFF, 0x2222_2222]);
        assert_eq!(list.restore, vec![0xAAAA_0001, 0xAAAA_0002]);
        assert_eq!(list.format_start, 0x40);
        assert_eq!(list.restore_list_size, 3);
        assert_eq!(list.offsets_start, 0x50);
    }

    #[test]
    fn parse_rlc_register_list_absent_on_v1_header() {
        let data = header_bytes(&[7], 0x34);
        assert!(parse_rlc_register_list(&data).unwrap().is_none());
    }

    #[test]
    fn parse_rlc_register_list_rejects_out_of_bounds() {
        let data = rlc_v2_list_bytes(&[1, 2, 3], &[4]);
        assert!(parse_rlc_register_list(&data[..data.len() - 8]).is_err());
    }

    #[test]
    fn parse_rlc_register_list_rejects_unaligned_sizes() {
        let mut data = rlc_v2_list_bytes(&[1, 2, 3], &[4]);
        let offset = REG_LIST_FORMAT_SIZE_OFFSET;
        data[offset..offset + 4].copy_from_slice(&6u32.to_le_bytes());
        assert!(parse_rlc_register_list(&data).is_err());
    }
}

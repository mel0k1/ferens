use std::fs;
use std::io;
use std::path::Path;

use ferens_core::Chip;

pub const FIRMWARE_DIR: &str = "/lib/firmware/amdgpu";
pub const FIRMWARE_DIR_ALT: &str = "/usr/lib/firmware/amdgpu";

const HEADER_VERSION_MAJOR: u16 = 1;
const HEADER_MIN_BYTES: usize = 0x2C;
const UCODE_VERSION_OFFSET: usize = 0x10;
const UCODE_SIZE_OFFSET: usize = 0x14;
const UCODE_ARRAY_OFFSET: usize = 0x18;
const FEATURE_VERSION_OFFSET: usize = 0x20;

pub struct MecFirmware {
    pub version: u32,
    pub feature_version: u32,
    pub words: Vec<u32>,
}

pub struct MecFirmwareSet {
    pub mec1: MecFirmware,
    pub mec2: Option<MecFirmware>,
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

pub fn parse_gfx_firmware(data: &[u8]) -> io::Result<MecFirmware> {
    let header_size = read_word(data, 0x04)? as usize;
    let version_major = u16::from_le_bytes([data[8], data[9]]);
    let version = read_word(data, UCODE_VERSION_OFFSET)?;
    let ucode_size = read_word(data, UCODE_SIZE_OFFSET)? as usize;
    let array_offset = read_word(data, UCODE_ARRAY_OFFSET)? as usize;
    let feature_version = read_word(data, FEATURE_VERSION_OFFSET)?;
    if version_major != HEADER_VERSION_MAJOR {
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
    Ok(MecFirmware {
        version,
        feature_version,
        words,
    })
}

fn firmware_path(dir: &Path, prefix: &str, suffix: &str) -> io::Result<Vec<u8>> {
    let path = dir.join(format!("{prefix}_{suffix}.bin"));
    fs::read(&path).map_err(|error| io::Error::other(format!("{}: {error}", path.display())))
}

fn load_one(dir: &Path, prefix: &str, suffix: &str) -> io::Result<MecFirmware> {
    let data = firmware_path(dir, prefix, suffix)?;
    parse_gfx_firmware(&data)
}

pub fn load(chip: Chip) -> io::Result<MecFirmwareSet> {
    let prefix = chip_prefix(chip)?;
    let dir = Path::new(FIRMWARE_DIR);
    let dir_alt = Path::new(FIRMWARE_DIR_ALT);
    let mec1 = load_one(dir, prefix, "mec").or_else(|error| {
        if dir_alt.join(format!("{prefix}_mec.bin")).exists() {
            load_one(dir_alt, prefix, "mec")
        } else {
            Err(error)
        }
    })?;
    let mec2 = load_one(dir, prefix, "mec2")
        .ok()
        .or_else(|| load_one(dir_alt, prefix, "mec2").ok());
    Ok(MecFirmwareSet { mec1, mec2 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_bytes(ucode: &[u32], header_size: usize) -> Vec<u8> {
        let mut data = vec![0u8; header_size];
        data[0x04..0x08].copy_from_slice(&(header_size as u32).to_le_bytes());
        data[0x08..0x0A].copy_from_slice(&HEADER_VERSION_MAJOR.to_le_bytes());
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
    fn chip_prefix_covers_polaris_only() {
        assert_eq!(chip_prefix(Chip::Polaris10).unwrap(), "polaris10");
        assert_eq!(chip_prefix(Chip::Polaris12).unwrap(), "polaris12");
        assert!(chip_prefix(Chip::VegaM).is_err());
        assert!(chip_prefix(Chip::Unknown(0x1234)).is_err());
    }
}

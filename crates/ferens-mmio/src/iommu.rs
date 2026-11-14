use std::io;
use std::path::{Path, PathBuf};

pub const IOMMU_GROUPS: &str = "/sys/kernel/iommu_groups";
pub const ALLOW_ENV: &str = "FERENS_IOMMU_ALLOW";

fn group_device(root: &Path, group: &Path, slot: &str) -> Option<PathBuf> {
    let devices = group.join("devices");
    let candidate = devices.join(slot);
    if candidate.exists() {
        return Some(candidate);
    }
    let _ = root;
    None
}

pub fn behind_iommu(root: &Path, slot: &str) -> io::Result<bool> {
    let groups = root.join(IOMMU_GROUPS.trim_start_matches('/'));
    let entries = match std::fs::read_dir(&groups) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if group_device(root, &entry.path(), slot).is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn guard(root: &Path, slot: &str) -> io::Result<()> {
    if std::env::var_os(ALLOW_ENV).is_some_and(|value| value == "1") {
        return Ok(());
    }
    if behind_iommu(root, slot)? {
        return Err(io::Error::other(format!(
            "{slot} is behind an iommu group: gpu dma to system memory will fault, \
             add the device to the passthrough or set {ALLOW_ENV}=1"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_groups(root: &Path, slot: Option<&str>) {
        let group = root.join("sys/kernel/iommu_groups/3");
        std::fs::create_dir_all(group.join("devices")).unwrap();
        if let Some(slot) = slot {
            std::fs::write(group.join("devices").join(slot), "").unwrap();
        }
    }

    #[test]
    fn no_groups_means_no_guard() {
        let root = std::env::temp_dir().join(format!("ferens-iommu-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sys/kernel/iommu_groups")).unwrap();
        assert!(!behind_iommu(&root, "0000:01:00.0").unwrap());
        guard(&root, "0000:01:00.0").unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_sysfs_tree_is_not_guarded() {
        let root =
            std::env::temp_dir().join(format!("ferens-iommu-missing-{}", std::process::id()));
        assert!(!behind_iommu(&root, "0000:01:00.0").unwrap());
    }

    #[test]
    fn device_in_group_is_guarded() {
        let root = std::env::temp_dir().join(format!("ferens-iommu-hit-{}", std::process::id()));
        make_groups(&root, Some("0000:01:00.0"));
        assert!(behind_iommu(&root, "0000:01:00.0").unwrap());
        let error = guard(&root, "0000:01:00.0").unwrap_err();
        assert!(error.to_string().contains("iommu group"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn other_device_in_group_is_not_guarded() {
        let root = std::env::temp_dir().join(format!("ferens-iommu-miss-{}", std::process::id()));
        make_groups(&root, Some("0000:02:00.0"));
        assert!(!behind_iommu(&root, "0000:01:00.0").unwrap());
        std::fs::remove_dir_all(&root).unwrap();
    }
}

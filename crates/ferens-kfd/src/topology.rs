use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

pub const TOPOLOGY_ROOT: &str = "/sys/class/kfd/kfd/topology/nodes";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub node_id: u32,
    pub gpu_id: u32,
    pub domain: u32,
    pub location_id: u32,
    pub device_id: u32,
    pub render_minor: u32,
    pub gfx_target_version: u32,
}

impl Node {
    pub fn slot(&self) -> String {
        format!(
            "{:04x}:{:02x}:{:02x}.{}",
            self.domain,
            (self.location_id >> 8) & 0xff,
            (self.location_id >> 3) & 0x1f,
            self.location_id & 0x7
        )
    }

    pub fn render_node(&self) -> String {
        format!("/dev/dri/renderD{}", self.render_minor)
    }
}

pub fn parse_properties(text: &str) -> BTreeMap<String, u32> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let key = parts.next()?.to_string();
            let value = parts.next()?.parse::<u32>().ok()?;
            Some((key, value))
        })
        .collect()
}

fn read_gpu_id(dir: &Path) -> io::Result<Option<u32>> {
    match fs::read_to_string(dir.join("gpu_id")) {
        Ok(text) => Ok(text.trim().parse().ok()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn scan_dir(root: &Path) -> io::Result<Vec<Node>> {
    let mut nodes = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().into_string().unwrap_or_default();
        let Ok(node_id) = name.parse::<u32>() else {
            continue;
        };
        let Some(gpu_id) = read_gpu_id(&entry.path())? else {
            continue;
        };
        if gpu_id == 0 {
            continue;
        }
        let text = fs::read_to_string(entry.path().join("properties")).unwrap_or_default();
        let props = parse_properties(&text);
        nodes.push(Node {
            node_id,
            gpu_id,
            domain: *props.get("domain").unwrap_or(&0),
            location_id: *props.get("location_id").unwrap_or(&0),
            device_id: *props.get("device_id").unwrap_or(&0),
            render_minor: *props.get("drm_render_minor").unwrap_or(&0),
            gfx_target_version: *props.get("gfx_target_version").unwrap_or(&0),
        });
    }
    nodes.sort_by_key(|node| node.node_id);
    Ok(nodes)
}

pub fn scan() -> io::Result<Vec<Node>> {
    scan_dir(Path::new(TOPOLOGY_ROOT))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_PROPERTIES: &str = "cpu_cores_count 0\nsimd_count 128\ngfx_target_version 80003\nvendor_id 4098\ndevice_id 26591\nlocation_id 256\ndomain 0\ndrm_render_minor 128\nmax_engine_clk_fcompute 1206\nnum_cp_queues 24\n";

    fn make_fixture(root: &Path) {
        let gpu = root.join("0");
        let cpu = root.join("1");
        fs::create_dir_all(&gpu).unwrap();
        fs::create_dir_all(&cpu).unwrap();
        fs::write(gpu.join("gpu_id"), "7\n").unwrap();
        fs::write(gpu.join("properties"), SAMPLE_PROPERTIES).unwrap();
        fs::write(cpu.join("properties"), "cpu_cores_count 8\n").unwrap();
    }

    #[test]
    fn properties_parse_numeric_pairs() {
        let props = parse_properties(SAMPLE_PROPERTIES);
        assert_eq!(props.get("device_id"), Some(&26591));
        assert_eq!(props.get("drm_render_minor"), Some(&128));
        assert_eq!(props.get("location_id"), Some(&256));
        assert_eq!(props.get("cpu_cores_count"), Some(&0));
        assert_eq!(props.len(), 10);
    }

    #[test]
    fn scan_keeps_gpu_nodes_with_gpu_id() {
        let root = std::env::temp_dir().join(format!("ferens-topo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        make_fixture(&root);
        let nodes = scan_dir(&root).unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_id, 0);
        assert_eq!(nodes[0].gpu_id, 7);
        assert_eq!(nodes[0].device_id, 26591);
        assert_eq!(nodes[0].slot(), "0000:01:00.0");
        assert_eq!(nodes[0].render_node(), "/dev/dri/renderD128");
    }

    #[test]
    fn slot_decodes_location_id() {
        let node = Node {
            node_id: 0,
            gpu_id: 7,
            domain: 0x1000,
            location_id: (0x40 << 8) | (0x08 << 3) | 0x1,
            device_id: 0x67df,
            render_minor: 128,
            gfx_target_version: 80003,
        };
        assert_eq!(node.slot(), "1000:40:08.1");
    }
}

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;

use acorn_dfs::new_map::FaultValue;
use acorn_dfs::new_map::sys_structures::FormatE;
use insta::{assert_debug_snapshot, with_settings};

#[test]
fn format_e_images() {
    const FILES: &[&str] = &["test_images/adfs800E.adf"];
    for f in FILES {
        let p = Path::new(*f);
        assert_format_e(p);
    }
}

fn load_format_e(path: &Path) -> FormatE {
    let contents = std::fs::read(path).unwrap();
    FormatE::parse(&contents).unwrap()
}

fn calculate_checksum(contents: &[u8]) -> u32 {
    // Arbitrary choice of polynomial. The important part is that its consistent.
    let crc = crc::Crc::<u32>::new(&crc::CRC_32_CKSUM);
    crc.checksum(contents)
}

fn assert_format_e(path: &Path) {
    let disk = load_format_e(path);

    let mut entries = BTreeMap::new();
    let mut checksums = BTreeMap::new();

    for f in disk.entries(None) {
        let Ok(FaultValue(Ok((entry, contents)), _)) = disk.get_file(&f) else {
            continue;
        };
        let crc = calculate_checksum(&contents);
        entries.insert(f.clone(), entry);
        checksums.insert(f, crc);
    }

    let snapshot_path = format!(
        "image_snapshots/{}/",
        path.file_name().and_then(OsStr::to_str).unwrap()
    );

    with_settings!({
        snapshot_path => snapshot_path,
        prepend_module_to_snapshot => false,
        input_file => path,
    }, {
        assert_debug_snapshot!(format!("faults"), &disk.faults);
        assert_debug_snapshot!(format!("map"), &disk.map);
        assert_debug_snapshot!(format!("entries"), entries);
        assert_debug_snapshot!(format!("checksums"), checksums);
    });
}

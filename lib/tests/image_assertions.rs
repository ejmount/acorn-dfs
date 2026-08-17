use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;

use acorn_dfs::error::FaultValue;
use acorn_dfs::new_map::new_map_disk::{Format, NewMapDisk};
use insta::{assert_debug_snapshot, with_settings};

#[test]
fn assert_image_snapshots() {
    const FILES: &[(&str, Format)] = &[
        ("test_images/adfs800E.adf", Format::E),
        ("test_images/adfs1600F.adf", Format::F),
    ];
    for (file, format) in FILES {
        let p = Path::new(*file);
        assert_file(p, *format);
    }
}

fn load_file(path: &Path, format: Format) -> NewMapDisk {
    let contents = std::fs::read(path).unwrap();
    NewMapDisk::parse(&contents, format).unwrap()
}

fn calculate_checksum(contents: &[u8]) -> u32 {
    // Arbitrary choice of polynomial. The important part is that its consistent.
    let crc = crc::Crc::<u32>::new(&crc::CRC_32_CKSUM);
    crc.checksum(contents)
}

fn assert_file(path: &Path, format: Format) {
    let disk = load_file(path, format);

    let mut entries = BTreeMap::new();
    let mut checksums = BTreeMap::new();

    for f in disk.entries(None).unwrap() {
        let mut contents = vec![];
        let Ok(FaultValue(entry, _)) = disk.get_file(&f, &mut contents) else {
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

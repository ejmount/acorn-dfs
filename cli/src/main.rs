use acorn_dfs::new_map::disc_structures::FormatE;
use acorn_dfs::old_map::FreeSpaceMap;

static DATA: &[u8] =
    include_bytes!("../../lib/test_images/ro_archive_test_tar_zip_sparkfile_sparkdir_arc.adf");

fn main() {
    let fsm_data = DATA;
    let map = FormatE::parse(fsm_data);
    println!("{map:#?}");
}

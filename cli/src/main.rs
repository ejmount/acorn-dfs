use acorn_dfs::new_map::FormatE;
use acorn_dfs::old_map::FreeSpaceMap;

static DATA: &[u8] =
    include_bytes!("../../lib/test_images/0344_ComputerConcepts_TurboDrivers_HP1_fluxengine.img");

fn main() {
    let fsm_data = DATA;
    let map = FormatE::parse(fsm_data);
    println!("{map:#?}");
}

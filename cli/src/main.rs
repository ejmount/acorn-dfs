use acorn_dfs::structures::FreeSpaceMap;

static DATA: &[u8] =
    include_bytes!("../../lib/test_images/0344_ComputerConcepts_TurboDrivers_HP1_fluxengine.img");

fn main() {
    let fsm_data = DATA.first_chunk().unwrap();
    let fsm = FreeSpaceMap::from_bytes(fsm_data).unwrap();
    println!("{fsm:#?}");
}

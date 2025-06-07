use fuser::MountOption;
use fuselog_core::FuseLogFS; 

fn main() {
    env_logger::init();

    let mountpoint = std::env::args_os().nth(1).expect("Expected mount point as argument");

    log::info!("Mounting filesystem at {:?}", mountpoint);

    let options = vec![
        MountOption::FSName("fuselog".to_string()),
        MountOption::AutoUnmount,
    ];

    fuser::mount2(FuseLogFS, mountpoint, &options).unwrap();
}
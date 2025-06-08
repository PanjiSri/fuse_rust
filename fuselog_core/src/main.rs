use fuser::MountOption;
use fuselog_core::socket::start_listener;
use fuselog_core::FuseLogFS;

const SOCKET_PATH: &str = "/tmp/fuselog.sock";

fn main() {
    env_logger::init();

    let mountpoint = std::env::args_os().nth(1).expect("Expected mount point as argument");
    log::info!("Mounting filesystem at {:?}", mountpoint);

    if let Err(e) = start_listener(SOCKET_PATH) {
        log::error!("Failed to start socket listener: {}", e);
        std::process::exit(1);
    }

    let options = vec![
        MountOption::FSName("fuselog".to_string()),
        MountOption::AutoUnmount,
    ];

    fuser::mount2(FuseLogFS, mountpoint, &options).unwrap();
}
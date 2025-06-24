use fuser::MountOption;
use fuselog_core::socket::start_listener;
use fuselog_core::FuseLogFS;
use std::path::PathBuf;
use std::env;

const SOCKET_PATH: &str = "/tmp/fuselog.sock";

fn main() {
    env_logger::init();

    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <directory>", args[0]);
        std::process::exit(1);
    }

    let root_dir = PathBuf::from(&args[1]);

    if !root_dir.exists() {
        if let Err(e) = std::fs::create_dir_all(&root_dir) {
            log::error!("Failed to create directory '{}': {}", root_dir.display(), e);
            std::process::exit(1);
        }
        log::info!("Created directory: {}", root_dir.display());
    } else if !root_dir.is_dir() {
        log::error!("Path '{}' exists but is not a directory", root_dir.display());
        std::process::exit(1);
    }

    log::info!("Starting Fuselog on directory: '{}'", root_dir.display());

    let socket_file = env::var("FUSELOG_SOCKET_FILE").unwrap_or_else(|_| SOCKET_PATH.to_string());
    if let Err(e) = start_listener(&socket_file[..]) {
        log::error!("Failed to start socket listener: {}", e);
        std::process::exit(1);
    }

    if let Err(e) = std::env::set_current_dir(&root_dir) {
        log::error!("Failed to change directory to '{}': {}", root_dir.display(), e);
        std::process::exit(1);
    }

    let options = vec![
        MountOption::FSName("fuselog".to_string()),
        MountOption::AutoUnmount,
        MountOption::AllowOther,
        MountOption::DefaultPermissions,
    ];

    let fs = FuseLogFS::new(root_dir.clone());

    fuser::mount2(fs, &root_dir, &options).unwrap();
}

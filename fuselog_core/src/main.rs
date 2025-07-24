use fuser::MountOption;
use fuselog_core::socket::start_listener;
use fuselog_core::FuseLogFS;
use std::path::PathBuf;
use std::env;
use std::sync::mpsc;
use std::thread;
use std::fs::File;
use daemonize::Daemonize;

const SOCKET_PATH: &str = "/tmp/fuselog.sock";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    
    let foreground = args.iter().any(|arg| arg == "-f" || arg == "--foreground");
    
    let filtered_args: Vec<String> = args.into_iter()
        .filter(|arg| arg != "-f" && arg != "--foreground")
        .collect();
    
    if filtered_args.len() != 2 {
        eprintln!("Usage: {} [-f|--foreground] <directory>", filtered_args[0]);
        std::process::exit(1);
    }

    let root_dir = PathBuf::from(&filtered_args[1]);

    if !root_dir.exists() {
        if let Err(e) = std::fs::create_dir_all(&root_dir) {
            eprintln!("Failed to create directory '{}': {}", root_dir.display(), e);
            std::process::exit(1);
        }
        println!("Created directory: {}", root_dir.display());
    } else if !root_dir.is_dir() {
        eprintln!("Path '{}' exists but is not a directory", root_dir.display());
        std::process::exit(1);
    }

    if foreground {
        env_logger::init();
        log::info!("Starting Fuselog in foreground mode on directory: '{}'", root_dir.display());
        let exit_code = run_fuse_logic(root_dir);
        std::process::exit(exit_code);
    } else {
        let stdout = match File::create("/tmp/fuselog.out") {
            Ok(file) => file,
            Err(e) => {
                eprintln!("Failed to create stdout log file: {}", e);
                std::process::exit(1);
            }
        };
        
        let stderr = match File::create("/tmp/fuselog.err") {
            Ok(file) => file,
            Err(e) => {
                eprintln!("Failed to create stderr log file: {}", e);
                std::process::exit(1);
            }
        };

        // Create PID file name based on mount point
        let pid_file = format!("/tmp/fuselog_{}.pid", 
            root_dir.to_string_lossy().replace("/", "_").replace(" ", "_"));

        let daemonize = Daemonize::new()
            .pid_file(pid_file)
            .chown_pid_file(true)
            .working_directory(&root_dir)
            .stdout(stdout)
            .stderr(stderr);

        match daemonize.start() {
            Ok(_) => {
                env_logger::init();
                log::info!("Successfully daemonized fuselog for directory: '{}'", root_dir.display());
                let exit_code = run_fuse_logic(root_dir);
                std::process::exit(exit_code);
            }
            Err(e) => {
                eprintln!("Error daemonizing: {}", e);
                std::process::exit(1);
            }
        }
    }
}

fn run_fuse_logic(root_dir: PathBuf) -> i32 {
    log::info!("Starting Fuselog on directory: '{}'", root_dir.display());
    let (shutdown_tx, shutdown_rx) = mpsc::channel();

    let socket_file = env::var("FUSELOG_SOCKET_FILE").unwrap_or_else(|_| SOCKET_PATH.to_string());

    let listener_handle = thread::spawn({
        let socket_file = socket_file.clone();
        move || {
            if let Err(e) = start_listener(&socket_file[..], shutdown_rx) {
                log::error!("Failed to start socket listener: {}", e);
                std::process::exit(1);
            }
        }
    });

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

    let exit_code = match fuser::mount2(fs, &root_dir, &options) {
        Ok(_) => {
            log::info!("FUSE filesystem has been unmounted.");
            0
        }
        Err(e) => {
            log::error!("Failed to mount FUSE filesystem: {}", e);
            1
        }
    };

    let _ = shutdown_tx.send(());

    if let Err(e) = listener_handle.join() {
        log::error!("Listener thread panicked: {:?}", e);
    }

    exit_code
}

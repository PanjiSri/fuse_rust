use crate::STATEDIFF_LOG;
use bincode::config;
use log::{error, info, warn};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};

fn handle_client(mut stream: UnixStream) -> Result<(), Box<dyn std::error::Error>> {
    info!("Socket: Client connected");

    let mut buffer = [0; 1];

    loop {
        match stream.read_exact(&mut buffer) {
            Ok(_) => {
                let result = match buffer[0] {
                    b'g' => send_statediff(stream.try_clone()?),
                    b'c' => clear_statediff(),
                    b'm' => {
                        println!("[]==========[] CHECKPOINT []==========[] ");
                        Ok(())
                    },
                    _ => {
                        warn!("Socket: Received unknown command: {}", buffer[0] as char);
                        Ok(())
                    }
                };

                if let Err(e) = result {
                    error!("Socket: Command error: {}", e);
                }
            }
            Err(e) => {
                eprintln!("Read error or client disconnected: {}", e);
                break;
            }
        }
    }
    Ok(())
}

pub fn start_listener(socket_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let _ = std::fs::remove_file(socket_path);
    
    let listener = UnixListener::bind(socket_path).map_err(|e| {
        error!("Failed to bind to socket at {}: {}", socket_path, e);
        e
    })?;
    
    info!("Socket listener started at {}", socket_path);
    
    for stream in listener.incoming() {
        println!("Client connected");

        let res: Result<(), Box<dyn std::error::Error>> = match stream {
            Ok(stream) => handle_client(stream),
            Err(e) => {
                error!("Socket: Error handling client: {}", e);
                Ok(())
            },
        };

        if let Err(e) = res {
            error!("Socket: Error handling client: {}", e);
        }

        info!("Socket: Client disconnected");
    }

    Ok(())
}

// Helper Functions
fn send_statediff(mut stream: UnixStream) -> Result<(), Box<dyn std::error::Error>> {
    info!("Socket: Received 'get' command");

    let serialized_data = {
        let mut log = STATEDIFF_LOG.lock().map_err(|e| {
            error!("Socket: Failed to lock statediff log: {}", e);
            std::io::Error::new(std::io::ErrorKind::Other, "Lock poisoned")
        })?;

        let data = bincode::encode_to_vec(&*log, config::standard()).map_err(|e| {
            error!("Socket: Failed to serialize statediff log: {}", e);
            std::io::Error::new(std::io::ErrorKind::Other, format!("Serialization failed: {}", e))
        })?;

        let action_count = log.actions.len();
        let fid_count = log.fid_map.len();
        log.actions.clear();
        log.fid_map.clear();

        info!("Socket: Cleared statediff log (had {} actions, {} fids). Sending {} bytes", 
            action_count, fid_count, data.len());

        data
    };

    // Send stateDiff size first
    stream.write_all(&serialized_data.len().to_le_bytes())?;

    // Send the actual stateDiff
    stream.write_all(&serialized_data)?;
    info!("Socket: Successfully sent data to client");
    Ok(())
}

fn clear_statediff() -> Result<(), Box<dyn std::error::Error>> {
    info!("Socket: Received 'clear' command");

    let mut log = STATEDIFF_LOG.lock().map_err(|e| {
        error!("Socket: Failed to lock statediff log: {}", e);
        std::io::Error::new(std::io::ErrorKind::Other, "Lock poisoned")
    })?;

    let action_count = log.actions.len();
    let fid_count = log.fid_map.len();
    log.actions.clear();
    log.fid_map.clear();

    info!("Socket: Cleared statediff log (had {} actions, {} fids).", 
        action_count, fid_count);

    Ok(())
}

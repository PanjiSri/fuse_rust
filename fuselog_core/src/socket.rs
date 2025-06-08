use crate::STATEDIFF_LOG;
use bincode::config;
use log::{error, info, warn};
use std::io::{Read, Write};
use std::thread;
use std::os::unix::net::{UnixListener, UnixStream};

fn handle_client(mut stream: UnixStream) -> Result<(), Box<dyn std::error::Error>> {
    info!("Socket: Client connected");
    
    let mut buffer = [0; 1];
    
    stream.read_exact(&mut buffer)?;
    
    if buffer[0] == b'g' {
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
        
        stream.write_all(&serialized_data)?;
        info!("Socket: Successfully sent data to client");
        
    } else {
        warn!("Socket: Received unknown command: 0x{:02x}", buffer[0]);
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
    
    let socket_path_owned = socket_path.to_string();
    
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    thread::spawn(move || {
                        if let Err(e) = handle_client(stream) {
                            error!("Socket: Error handling client: {}", e);
                        }
                        info!("Socket: Client disconnected");
                    });
                }
                Err(e) => {
                    error!("Socket: Failed to accept connection: {}", e);
                }
            }
        }
        
        let _ = std::fs::remove_file(&socket_path_owned);
        info!("Socket: Listener thread exiting, cleaned up socket file");
    });
    
    Ok(())
}
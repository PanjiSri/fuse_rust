use crate::statediff::{StateDiffAction, StateDiffLog};
use crate::STATEDIFF_LOG;
use bincode::config;
use log::{error, info, warn};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::env;

#[derive(Default)]
struct PruneState {
    creation_idx: Option<usize>,
    last_chmod_idx: Option<usize>,
    last_chown_idx: Option<usize>,
}

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

fn prune_log(log: &mut StateDiffLog) {
    if log.actions.is_empty() {
        return;
    }

    let original_action_count = log.actions.len();
    let original_fid_count = log.fid_map.len();

    let mut actions: Vec<Option<StateDiffAction>> = log.actions.drain(..).map(Some).collect();
    let mut file_states: HashMap<u64, PruneState> = HashMap::new();
    let mut fids_to_purge: HashSet<u64> = HashSet::new();

    for i in 0..actions.len() {
        let action = match &actions[i] {
            Some(a) => a,
            None => continue,
        };

        match action {
            StateDiffAction::Create { fid, .. }
            | StateDiffAction::Mkdir { fid }
            | StateDiffAction::Symlink { link_fid: fid, .. } => {
                file_states.entry(*fid).or_default().creation_idx = Some(i);
            }
            StateDiffAction::Chmod { fid, .. } => {
                let state = file_states.entry(*fid).or_default();
                if let Some(prev_idx) = state.last_chmod_idx.replace(i) {
                    actions[prev_idx] = None;
                }
            }
            StateDiffAction::Chown { fid, .. } => {
                let state = file_states.entry(*fid).or_default();
                if let Some(prev_idx) = state.last_chown_idx.replace(i) {
                    actions[prev_idx] = None; 
                }
            }
            StateDiffAction::Unlink { fid } | StateDiffAction::Rmdir { fid } => {
                if let Some(state) = file_states.get(fid) {
                    if state.creation_idx.is_some() {
                        fids_to_purge.insert(*fid);
                    }
                }
            }
            _ => {}
        }
    }

    let mut final_actions = Vec::new();
    let mut used_fids = HashSet::new();

    for action_opt in actions.into_iter() {
        if let Some(action) = action_opt {
            let mut action_fids = Vec::new();
            match &action {
                StateDiffAction::Create { fid, .. }
                | StateDiffAction::Write { fid, .. }
                | StateDiffAction::Unlink { fid }
                | StateDiffAction::Truncate { fid, .. }
                | StateDiffAction::Chown { fid, .. }
                | StateDiffAction::Chmod { fid, .. }
                | StateDiffAction::Mkdir { fid }
                | StateDiffAction::Rmdir { fid } => action_fids.push(*fid),
                StateDiffAction::Symlink { link_fid, .. } => action_fids.push(*link_fid),
                StateDiffAction::Rename { from_fid, to_fid } => {
                    action_fids.push(*from_fid);
                    action_fids.push(*to_fid);
                }
                StateDiffAction::Link {
                    source_fid,
                    new_link_fid,
                } => {
                    action_fids.push(*source_fid);
                    action_fids.push(*new_link_fid);
                }
            };

            if action_fids.iter().any(|fid| fids_to_purge.contains(fid)) {
                continue;
            }

            for fid in action_fids {
                used_fids.insert(fid);
            }
            final_actions.push(action);
        }
    }

    log.actions = final_actions;
    log.fid_map.retain(|fid, _| used_fids.contains(fid));

    if log.actions.len() < original_action_count {
        info!(
            "Log pruned: {} actions -> {} actions, {} fids -> {} fids",
            original_action_count,
            log.actions.len(),
            original_fid_count,
            log.fid_map.len()
        );
    }
}

fn send_statediff(mut stream: UnixStream) -> Result<(), Box<dyn std::error::Error>> {
    info!("Socket: Received 'get' command");

    let serialized_data = {
        let mut log = STATEDIFF_LOG.lock().map_err(|e| {
            error!("Socket: Failed to lock statediff log: {}", e);
            std::io::Error::new(std::io::ErrorKind::Other, "Lock poisoned")
        })?;

        let original_action_count = log.actions.len();
        let original_fid_count = log.fid_map.len();

        // Pruning is disabled by default
        let is_prune_enabled = env::var("FUSELOG_PRUNE")
            .map_or(false, |val| val.to_lowercase() == "true" || val == "1");

        if is_prune_enabled {
            info!("=========================================");
            info!("Pruning enabled. Pruning statediff log...");
            info!("==========================================");
            prune_log(&mut log);
        } else {
            info!("Pruning is disabled. Skipping pruning of statediff log.");
        }

        let bincode_data = bincode::encode_to_vec(&*log, config::standard()).map_err(|e| {
            error!("Socket: Failed to serialize statediff log: {}", e);
            std::io::Error::new(std::io::ErrorKind::Other, format!("Serialization failed: {}", e))
        })?;

        // Compression is disabled by default
        let compression_enabled = env::var("FUSELOG_COMPRESSION")
            .map_or(false, |val| val.to_lowercase() == "true" || val == "1");

        let final_payload = if compression_enabled && !bincode_data.is_empty() {
            let adaptive_enabled = env::var("ADAPTIVE_COMPRESSION")
                .map_or(false, |val| val.to_lowercase() == "true" || val == "1");

            if adaptive_enabled {
                info!("Adaptive compression enabled (placeholder). Using standard compression for now.");
                // This is the placeholder for future adaptive logic.
                // For now, it behaves identically to the 'else' block.
            } else {
                info!("Standard zstd compression enabled.");
            }

            let compressed_data = zstd::encode_all(bincode_data.as_slice(), 0)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Zstd compression failed: {}", e)))?;

            info!("Data compressed from {} to {} bytes.", bincode_data.len(), compressed_data.len());

            let mut payload = Vec::with_capacity(1 + compressed_data.len());
            payload.push(b'z');
            payload.extend(compressed_data);
            payload
        } else {
            info!("Compression is disabled or data is empty. Sending raw data.");
            let mut payload = Vec::with_capacity(1 + bincode_data.len());
            payload.push(b'n');
            payload.extend(bincode_data);
                payload
        };        

        let action_count = log.actions.len();
        let fid_count = log.fid_map.len();
        log.actions.clear();
        log.fid_map.clear();

        info!("Socket: Original statediff log had {} actions, {} fids. Pruned to {} actions, {} fids.",
            original_action_count, original_fid_count, action_count, fid_count);

        final_payload
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
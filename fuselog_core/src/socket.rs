use crate::statediff::{StateDiffAction, StateDiffLog};
use crate::STATEDIFF_LOG;
use bincode::config;
use log::{error, info, warn};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::env;

#[derive(Default)]
struct PruneState {
    creation_idx: Option<usize>,
    last_chmod_idx: Option<usize>,
    last_chown_idx: Option<usize>,
}

struct AdaptiveState {
    // Buffer to hold raw, uncompressed bincode data for training.
    training_buffer: Vec<Vec<u8>>,
    // The current dictionary bytes, if one has been trained.
    encoder_dict: Option<Arc<Vec<u8>>>,
}

impl Default for AdaptiveState {
    fn default() -> Self {
        Self {
            training_buffer: Vec::new(),
            encoder_dict: None,
        }
    }
}

static ADAPTIVE_STATE: once_cell::sync::Lazy<Mutex<AdaptiveState>> = once_cell::sync::Lazy::new(|| Mutex::new(AdaptiveState::default()));

fn handle_client(mut stream: UnixStream) -> Result<(), Box<dyn std::error::Error>> {
    info!("Socket: Client connected");

    let mut buffer = [0; 1];

    loop {
        match stream.read_exact(&mut buffer) {
            Ok(_) => {
                let result = match buffer[0] {
                    b'g' => send_statediff(stream.try_clone()?),
                    b't' => trigger_retraining(),
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

fn trigger_retraining() -> Result<(), Box<dyn std::error::Error>> {
    info!("Socket: Received 'train' command.");

    let mut state = ADAPTIVE_STATE.lock().unwrap();

    if state.training_buffer.is_empty() {
        warn!("Training buffer is empty, cannot train new dictionary.");
        return Ok(());
    }

    let total_bytes: usize = state.training_buffer.iter().map(|v| v.len()).sum();
    let sample_count = state.training_buffer.len();
    
    // Best Practice: > 100 samples, 100x training data to dict size ratio
    // https://manpages.debian.org/testing/zstd/zstd.1.en.html

    // DEVELOPMENT MODE: Relaxed thresholds for testing
    const DEV_MIN_SAMPLES: usize = 5;
    const DEV_MIN_TOTAL_BYTES: usize = 2 * 1024; 
    const DEV_MIN_BYTES_PER_SAMPLE: usize = 50;
    
    // PRODUCTION MODE: Following zstd best practices  
    // 100KB (for 1KB dict)
    const PROD_MIN_SAMPLES: usize = 50;        
    const PROD_MIN_TOTAL_BYTES: usize = 100 * 1024; 
    const PROD_MIN_BYTES_PER_SAMPLE: usize = 500;
    
    let dev_mode = std::env::var("ADAPTIVE_DEV_MODE")
        .map_or(false, |val| val.to_lowercase() == "true" || val == "1");
    
    let (min_samples, min_total_bytes, min_bytes_per_sample) = if dev_mode {
        info!("Using development mode thresholds (relaxed for testing)");
        (DEV_MIN_SAMPLES, DEV_MIN_TOTAL_BYTES, DEV_MIN_BYTES_PER_SAMPLE)
    } else {
        info!("Using production mode thresholds (zstd best practices)");
        (PROD_MIN_SAMPLES, PROD_MIN_TOTAL_BYTES, PROD_MIN_BYTES_PER_SAMPLE)
    };
    
    if sample_count < min_samples {
        warn!("Insufficient samples for training: {} (need at least {}). Collect more data first.", 
              sample_count, min_samples);
        return Ok(());
    }
    
    if total_bytes < min_total_bytes {
        warn!("Insufficient training data: {} bytes (need at least {} bytes). Collect more data first.", 
              total_bytes, min_total_bytes);
        return Ok(());
    }
    
    let avg_sample_size = total_bytes / sample_count;
    if avg_sample_size < min_bytes_per_sample {
        warn!("Average sample size too small: {} bytes (need at least {} bytes per sample).", 
              avg_sample_size, min_bytes_per_sample);
        return Ok(());
    }

    info!(
        "Starting dictionary training with {} samples ({} bytes total, avg {} bytes per sample).",
        sample_count, total_bytes, avg_sample_size
    );

    // Official recommendation: training data should be 100x dictionary size
    // So dictionary should be ~1% of training data, capped at reasonable limits
    let dict_size = if dev_mode {
        // Development: smaller dictionaries, less strict ratios
        std::cmp::min(8 * 1024, total_bytes / 2).max(512) // 512B to 8KB
    } else {
        // Production: follow zstd guidelines more closely
        let optimal_size = total_bytes / 100;
        std::cmp::min(
            std::cmp::min(64 * 1024, optimal_size),
            total_bytes / 10 
        ).max(1024) 
    };

    match zstd::dict::from_samples(&state.training_buffer, dict_size) {
        Ok(dict_content) => {
            info!(
                "Successfully trained new dictionary of size {} bytes (target was {} bytes).",
                dict_content.len(), dict_size
            );
            
            let compression_ratio = (dict_size as f32) / (total_bytes as f32) * 100.0;
            info!("Dictionary represents {:.2}% of training data size", compression_ratio);
            
            state.encoder_dict = Some(Arc::new(dict_content));
            
            let max_buffer_samples = if dev_mode { 20 } else { 200 };
            if state.training_buffer.len() > max_buffer_samples {
                let keep_samples = max_buffer_samples / 2;
                let buffer_len = state.training_buffer.len();
                state.training_buffer.drain(0..buffer_len - keep_samples);
            }
        }
        Err(e) => {
            error!("Failed to train dictionary: {}", e);
            return Err(Box::new(e));
        }
    }

    Ok(())
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
                let dict_arc_option;
                {
                    let mut state = ADAPTIVE_STATE.lock().unwrap();
                    state.training_buffer.push(bincode_data.clone());
                    dict_arc_option = state.encoder_dict.clone();
                }

                if let Some(dict) = dict_arc_option {
                    info!("Compressing with adaptive dictionary.");
                    let mut compressor = zstd::bulk::Compressor::with_dictionary(1, &dict)?;
                    let compressed_log = compressor.compress(&bincode_data)?;
                    info!("Data compressed from {} to {} bytes using dictionary.", bincode_data.len(), compressed_log.len());

                    if Arc::strong_count(&dict) == 2 {
                        info!("Attaching new dictionary to payload.");
                        let mut payload = Vec::new();
                        payload.push(b'd');
                        payload.extend_from_slice(&(dict.len() as u32).to_le_bytes());
                        payload.extend_from_slice(&dict);
                        payload.push(b'z');
                        payload.extend(compressed_log);
                        payload
                    } else {
                        // Dictionary is old, just send the data.
                        let mut payload = Vec::with_capacity(1 + compressed_log.len());
                        payload.push(b'z');
                        payload.extend(compressed_log);
                        payload
                    }
                } else {
                    info!("Adaptive mode on, but no dictionary trained yet. Using standard compression.");
                    let compressed_data = zstd::encode_all(&bincode_data[..], 0)?;
                    let mut payload = Vec::with_capacity(1 + compressed_data.len());
                    payload.push(b'z');
                    payload.extend(compressed_data);
                    payload
                }
            } else {
                info!("Standard zstd compression enabled.");
                let compressed_data = zstd::encode_all(&bincode_data[..], 0)?;
                info!("Data compressed from {} to {} bytes.", bincode_data.len(), compressed_data.len());
                let mut payload = Vec::with_capacity(1 + compressed_data.len());
                payload.push(b'z');
                payload.extend(compressed_data);
                payload
            }
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
    stream.write_all(&(serialized_data.len() as u64).to_le_bytes())?;

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
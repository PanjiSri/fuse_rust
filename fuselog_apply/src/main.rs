use fuselog_core::statediff::{StateDiffAction, StateDiffLog};
use log::{error, info, warn};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

const SOCKET_PATH: &str = "/tmp/fuselog.sock";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let target_dir = std::env::args()
        .nth(1)
        .expect("Usage: fuselog-apply <target_directory>");
    
    let target_path = Path::new(&target_dir);
    
    if !target_path.exists() {
        info!("Creating target directory: {}", target_dir);
        std::fs::create_dir_all(target_path)?;
    } else if !target_path.is_dir() {
        error!("Target path '{}' exists but is not a directory.", target_dir);
        std::process::exit(1);
    }
    
    info!("Applying changes to target directory: {}", target_dir);

    info!("Connecting to fuselog socket at {}", SOCKET_PATH);
    let mut stream = UnixStream::connect(SOCKET_PATH)
        .map_err(|e| format!("Failed to connect to socket: {}. Is fuselog_core running?", e))?;

    info!("Requesting state diff log...");
    stream.write_all(b"g")?;

    let mut buffer = Vec::new();
    stream.read_to_end(&mut buffer)?;
    info!("Received {} bytes of data", buffer.len());

    if buffer.is_empty() {
        info!("No changes to apply - log is empty");
        return Ok(());
    }

    let (log, _): (StateDiffLog, usize) = bincode::decode_from_slice(
        &buffer, 
        bincode::config::standard()
    )?;
    
    info!("Deserialized log with {} actions and {} file mappings", 
          log.actions.len(), log.fid_map.len());

    for (i, action) in log.actions.iter().enumerate() {
        info!("Applying action {}/{}: {:?}", i + 1, log.actions.len(), action);
        
        match action {
            StateDiffAction::Write { fid, offset, data } => {
                apply_write(&log, *fid, *offset, data, target_path)?;
            }
            StateDiffAction::Unlink { fid } => {
                apply_unlink(&log, *fid, target_path)?;
            }
            StateDiffAction::Truncate { fid, size } => {
                apply_truncate(&log, *fid, *size, target_path)?;
            }
            StateDiffAction::Rename { from_fid, to_fid } => {
                apply_rename(&log, *from_fid, *to_fid, target_path)?;
            }
            StateDiffAction::Link { source_fid, new_link_fid } => {
                apply_link(&log, *source_fid, *new_link_fid, target_path)?;
            }
            StateDiffAction::Chown { fid, uid, gid } => {
                apply_chown(&log, *fid, *uid, *gid, target_path)?;
            }
        }
    }

    info!("Successfully applied all {} actions", log.actions.len());
    Ok(())
}

fn apply_write(
    log: &StateDiffLog, 
    fid: u64, 
    offset: u64, 
    data: &[u8], 
    target_path: &Path
) -> Result<(), Box<dyn std::error::Error>> {
    let file_path = log.fid_map.get(&fid)
        .ok_or_else(|| format!("Unknown file ID: {}", fid))?;
    
    let full_path = target_path.join(file_path);
    
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    
    info!("  Writing {} bytes to {:?} at offset {}", data.len(), full_path, offset);
    
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .open(&full_path)?;
    
    use std::io::Seek;
    file.seek(std::io::SeekFrom::Start(offset))?;
    file.write_all(data)?;
    
    Ok(())
}

fn apply_unlink(
    log: &StateDiffLog, 
    fid: u64, 
    target_path: &Path
) -> Result<(), Box<dyn std::error::Error>> {
    let file_path = log.fid_map.get(&fid)
        .ok_or_else(|| format!("Unknown file ID: {}", fid))?;
    
    let full_path = target_path.join(file_path);
    info!("  Removing file: {:?}", full_path);
    
    match std::fs::remove_file(&full_path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warn!("  File already doesn't exist: {:?}", full_path);
            Ok(())
        }
        Err(e) => Err(Box::new(e))
    }
}

fn apply_truncate(
    log: &StateDiffLog, 
    fid: u64, 
    size: u64, 
    target_path: &Path
) -> Result<(), Box<dyn std::error::Error>> {
    let file_path = log.fid_map.get(&fid)
        .ok_or_else(|| format!("Unknown file ID: {}", fid))?;
    
    let full_path = target_path.join(file_path);
    
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    
    info!("  Truncating {:?} to {} bytes", full_path, size);
    
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)  
        .open(&full_path)?;
    
    file.set_len(size)?;
    Ok(())
}


fn apply_rename(
    log: &StateDiffLog, 
    from_fid: u64, 
    to_fid: u64, 
    target_path: &Path
) -> Result<(), Box<dyn std::error::Error>> {
    let from_path = log.fid_map.get(&from_fid)
        .ok_or_else(|| format!("Unknown from file ID: {}", from_fid))?;
    let to_path = log.fid_map.get(&to_fid)
        .ok_or_else(|| format!("Unknown to file ID: {}", to_fid))?;
    
    let full_from_path = target_path.join(from_path);
    let full_to_path = target_path.join(to_path);
    
    info!("  Renaming {:?} to {:?}", full_from_path, full_to_path);
    
    if let Some(parent) = full_to_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    
    std::fs::rename(full_from_path, full_to_path)?;
    Ok(())
}

fn apply_link(
    log: &StateDiffLog,
    source_fid: u64,
    new_link_fid: u64,
    target_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let source_file_path = log.fid_map.get(&source_fid)
        .ok_or_else(|| format!("Unknown source file ID for link: {}", source_fid))?;
    let new_link_path = log.fid_map.get(&new_link_fid)
        .ok_or_else(|| format!("Unknown new link file ID for link: {}", new_link_fid))?;

    let full_source_path = target_path.join(source_file_path);
    let full_new_link_path = target_path.join(new_link_path);

    info!("  Creating hard link from {:?} to {:?}", full_source_path, full_new_link_path);

    if let Some(parent) = full_new_link_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    
    std::fs::hard_link(full_source_path, full_new_link_path)?;
    Ok(())
}

fn apply_chown(
    log: &StateDiffLog,
    fid: u64,
    uid: u32,
    gid: u32,
    target_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let file_path = log.fid_map.get(&fid)
        .ok_or_else(|| format!("Unknown file ID for chown: {}", fid))?;

    let full_path = target_path.join(file_path);

    info!("  Changing ownership of {:?} to {}:{}", full_path, uid, gid);

    match std::os::unix::fs::chown(&full_path, Some(uid), Some(gid)) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warn!("  Cannot chown, file does not exist: {:?}", full_path);
            // Well, I think it is not fatal error if the file was pruned
            // But I will revisit this later
            Ok(())
        }
        Err(e) => Err(Box::new(e)),
    }
}
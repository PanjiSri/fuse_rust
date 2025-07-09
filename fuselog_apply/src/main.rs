use fuselog_core::statediff::{StateDiffAction, StateDiffLog};
use log::{error, info, warn};
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::os::unix::fs::PermissionsExt;
use std::env;
use std::sync::Arc;
use zstd::stream::read::Decoder;

const CACHE_DICT_PATH: &str = "/tmp/statediff.dict";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let target_dir = env::args()
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

    let Some(diff_file) = env::args().skip(2).next() else {
        error!("Not enough arguments.");
        std::process::exit(1);
    };

    let Some(diff_path) = diff_file.strip_prefix("--statediff=") else {
        error!("statediff file is not specified.");
        std::process::exit(1);
    };

    info!("Applying changes to target directory: {}", target_dir);
    info!("Reading state diff from file: {}", diff_path);

    let mut file = File::open(diff_path)
        .map_err(|e| format!("Failed to open diff file '{}': {}", diff_path, e))?;

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    info!("Received {} bytes of data", buffer.len());

    if buffer.is_empty() {
        info!("No changes to apply - log is empty");
        return Ok(());
    }

    let mut cursor = Cursor::new(&buffer);
    let mut header_buf = [0; 1];
    cursor.read_exact(&mut header_buf)?;
    let compression_header = header_buf[0];

    let bincode_slice = match compression_header {
        b'd' => {
            info!("Received payload with new dictionary.");
            let mut len_buf = [0; 4];
            cursor.read_exact(&mut len_buf)?;
            let dict_len = u32::from_le_bytes(len_buf) as usize;

            let dict_start = cursor.position() as usize;
            let dict_end = dict_start + dict_len;
            let dict_data = &cursor.get_ref()[dict_start..dict_end];

            info!("Saving new {} byte dictionary to '{}'", dict_len, CACHE_DICT_PATH);
            std::fs::write(CACHE_DICT_PATH, dict_data)?;
            let new_dict = Arc::new(dict_data.to_vec());

            cursor.set_position(dict_end as u64);
            
            cursor.read_exact(&mut header_buf)?;
            let data_start = cursor.position() as usize;
            let compressed_log_data = &cursor.get_ref()[data_start..];

            if header_buf[0] != b'z' {
                return Err("Expected zstd compressed data after dictionary payload".into());
            }

            info!("Decompressing log data using the new dictionary.");
            decompress_with_dictionary(compressed_log_data, &new_dict)?
        }
        b'z' => {
            let data_start = cursor.position() as usize;
            let compressed_data = &cursor.get_ref()[data_start..];

            info!("Received zstd compressed data. Attempting to decompress with cached dictionary.");
            let dict = std::fs::read(CACHE_DICT_PATH).ok();
            if let Some(d) = &dict {
                decompress_with_dictionary(compressed_data, d)?
            } else {
                warn!("No dictionary found at '{}'. Using standard decompression.", CACHE_DICT_PATH);
                zstd::decode_all(compressed_data)?
            }
        }
        b'n' => {
            info!("Received uncompressed data.");
            let data_start = cursor.position() as usize;
            cursor.get_ref()[data_start..].to_vec()
        }
        _ => return Err(format!("Unknown protocol header: '{}'", compression_header as char).into()),
    };

    let (log, _): (StateDiffLog, usize) = bincode::decode_from_slice(
        &bincode_slice, 
        bincode::config::standard()
    ).map_err(|e| format!("Failed to deserialize bincode data: {}", e))?;
    
    info!("Deserialized log with {} actions and {} file mappings", 
          log.actions.len(), log.fid_map.len());

    for (i, action) in log.actions.iter().enumerate() {
        info!("Applying action {}/{}: {:?}", i + 1, log.actions.len(), action);
        
        match action {
            StateDiffAction::Create { fid, uid, gid, mode } => {
                apply_create(&log, *fid, *uid, *gid, *mode, target_path)?;
            }
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
            StateDiffAction::Chmod { fid, mode } => {
                apply_chmod(&log, *fid, *mode, target_path)?;
            }
            StateDiffAction::Mkdir { fid } => {
                apply_mkdir(&log, *fid, target_path)?;
            }
            StateDiffAction::Rmdir { fid } => {
                apply_rmdir(&log, *fid, target_path)?;
            }
            StateDiffAction::Symlink { link_fid, target_path: symlink_target_str, uid, gid } => {
                apply_symlink(&log, *link_fid, symlink_target_str, *uid, *gid, target_path)?;
            }
        }
    }

    info!("Successfully applied all {} actions", log.actions.len());
    Ok(())
}

fn decompress_with_dictionary(compressed_data: &[u8], dict: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let cursor = Cursor::new(compressed_data);
    let mut decoder = Decoder::with_dictionary(cursor, dict)?;
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    Ok(decompressed)
}

fn get_full_path(log: &StateDiffLog, fid: u64, target_path: &Path) -> Result<PathBuf, String> {
    let file_path = log.fid_map.get(&fid)
        .ok_or_else(|| format!("Unknown file ID: {}", fid))?;
    Ok(target_path.join(file_path))
}

fn apply_create(
    log: &StateDiffLog,
    fid: u64,
    uid: u32,
    gid: u32,
    mode: u32,
    target_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let full_path = get_full_path(log, fid, target_path)?;

    info!("Creating file {:?} with mode {:o} and owner {}:{}", full_path, mode, uid, gid);

    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::File::create(&full_path)?;

    let perms = std::fs::Permissions::from_mode(mode);
    std::fs::set_permissions(&full_path, perms)?;

    std::os::unix::fs::chown(&full_path, Some(uid), Some(gid))?;
    
    Ok(())
}

fn apply_write(
    log: &StateDiffLog, 
    fid: u64, 
    offset: u64, 
    data: &[u8], 
    target_path: &Path
) -> Result<(), Box<dyn std::error::Error>> {
    let full_path = get_full_path(log, fid, target_path)?;
    
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    
    info!("Writing {} bytes to {:?} at offset {}", data.len(), full_path, offset);
    
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
    let full_path = get_full_path(log, fid, target_path)?;
    info!("Removing file: {:?}", full_path);
    
    match std::fs::remove_file(&full_path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warn!("File to unlink already doesn't exist: {:?}", full_path);
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
    let full_path = get_full_path(log, fid, target_path)?;
    
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    
    info!("Truncating {:?} to {} bytes", full_path, size);
    
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
    let full_from_path = get_full_path(log, from_fid, target_path)?;
    let full_to_path = get_full_path(log, to_fid, target_path)?;
    
    info!("Renaming {:?} to {:?}", full_from_path, full_to_path);
    
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
    let full_source_path = get_full_path(log, source_fid, target_path)?;
    let full_new_link_path = get_full_path(log, new_link_fid, target_path)?;

    info!("Creating hard link from {:?} to {:?}", full_source_path, full_new_link_path);

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
    let full_path = get_full_path(log, fid, target_path)?;

    info!("Changing ownership of {:?} to {}:{}", full_path, uid, gid);

    match std::os::unix::fs::lchown(&full_path, Some(uid), Some(gid)) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warn!("Cannot chown, file/dir does not exist: {:?}. This can be normal if it was deleted.", full_path);
            Ok(())
        }
        Err(e) => Err(Box::new(e)),
    }
}

fn apply_chmod(
    log: &StateDiffLog,
    fid: u64,
    mode: u32,
    target_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let full_path = get_full_path(log, fid, target_path)?;

    info!("Changing mode of {:?} to {:o}", full_path, mode);

    let perms = std::fs::Permissions::from_mode(mode);
    match std::fs::set_permissions(&full_path, perms) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warn!("Cannot chmod, file/dir does not exist: {:?}. This can be normal if it was deleted.", full_path);
            Ok(())
        }
        Err(e) => Err(Box::new(e)),
    }
}

fn apply_mkdir(
    log: &StateDiffLog,
    fid: u64,
    target_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let full_path = get_full_path(log, fid, target_path)?;
    info!("Creating directory: {:?}", full_path);
    std::fs::create_dir_all(&full_path)?;
    Ok(())
}

fn apply_rmdir(
    log: &StateDiffLog,
    fid: u64,
    target_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let full_path = get_full_path(log, fid, target_path)?;
    info!("Removing directory: {:?}", full_path);
    match std::fs::remove_dir(&full_path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warn!("Directory to remove already doesn't exist: {:?}", full_path);
            Ok(())
        }
        Err(e) => Err(Box::new(e)),
    }
}

fn apply_symlink(
    log: &StateDiffLog,
    link_fid: u64,
    target_path_str: &str,
    uid: u32,
    gid: u32,
    base_target_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let full_link_path = get_full_path(log, link_fid, base_target_path)?;

    info!("Creating symlink {:?} -> {} with owner {}:{}", full_link_path, target_path_str, uid, gid);

    if let Some(parent) = full_link_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::os::unix::fs::symlink(target_path_str, &full_link_path)?;
    std::os::unix::fs::lchown(&full_link_path, Some(uid), Some(gid))?;
    
    Ok(())
}
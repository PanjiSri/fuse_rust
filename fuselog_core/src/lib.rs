pub mod socket;
pub mod statediff;

use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyWrite, ReplyCreate, ReplyData, Request,
};
use libc::{ENOENT, EIO, EEXIST};
use log::{debug, info};
use statediff::{StateDiffAction, StateDiffLog};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH, SystemTime};

const TTL: Duration = Duration::from_secs(1);

static STATEDIFF_LOG: once_cell::sync::Lazy<Arc<Mutex<StateDiffLog>>> =
    once_cell::sync::Lazy::new(|| Arc::new(Mutex::new(StateDiffLog::default())));

fn get_fid(path: &str) -> u64 {
    let mut log = STATEDIFF_LOG.lock().unwrap();
    
    if let Some((fid, _)) = log.fid_map.iter().find(|(_, p)| p == &path) {
        return *fid;
    }
    
    let new_fid = log.fid_map.len() as u64 + 1;
    log.fid_map.insert(new_fid, path.to_string());
    new_fid
}

fn metadata_to_file_attr(ino: u64, metadata: &std::fs::Metadata) -> FileAttr {
    let file_type = if metadata.is_dir() {
        FileType::Directory
    } else if metadata.is_file() {
        FileType::RegularFile
    } else if metadata.file_type().is_symlink() {
        FileType::Symlink
    } else {
        FileType::RegularFile 
    };

    FileAttr {
        ino,
        size: metadata.len(),
        blocks: metadata.blocks(),
        atime: metadata.accessed().unwrap_or(UNIX_EPOCH),
        mtime: metadata.modified().unwrap_or(UNIX_EPOCH),
        ctime: SystemTime::UNIX_EPOCH + Duration::from_secs(metadata.ctime() as u64),
        crtime: metadata.created().unwrap_or(UNIX_EPOCH),
        kind: file_type,
        perm: (metadata.mode() & 0o7777) as u16,
        nlink: metadata.nlink() as u32,
        uid: metadata.uid(),
        gid: metadata.gid(),
        rdev: metadata.rdev() as u32,
        flags: 0,
        blksize: metadata.blksize() as u32,
    }
}

struct InodeManager {
    ino_to_path: HashMap<u64, PathBuf>,
    path_to_ino: HashMap<PathBuf, u64>,
    next_ino: u64,
}

impl InodeManager {
    fn new() -> Self {
        let mut manager = Self {
            ino_to_path: HashMap::new(),
            path_to_ino: HashMap::new(),
            next_ino: 2, 
        };
        
        let root_path = PathBuf::from(".");
        manager.ino_to_path.insert(1, root_path.clone());
        manager.path_to_ino.insert(root_path, 1);
        
        manager
    }
    
    fn get_path(&self, ino: u64) -> Option<&PathBuf> {
        self.ino_to_path.get(&ino)
    }
    
    fn get_or_create_ino(&mut self, path: &Path) -> u64 {
        if let Some(&ino) = self.path_to_ino.get(path) {
            return ino;
        }
        
        let ino = self.next_ino;
        self.next_ino += 1;
        self.ino_to_path.insert(ino, path.to_path_buf());
        self.path_to_ino.insert(path.to_path_buf(), ino);
        ino
    }
}

pub struct FuseLogFS {
    inodes: Mutex<InodeManager>,
}

impl FuseLogFS {
    pub fn new(_root: PathBuf) -> Self {
        Self {
            inodes: Mutex::new(InodeManager::new()),
        }
    }
    
    fn get_relative_path(&self, full_path: &Path) -> String {
        if full_path == Path::new(".") {
            ".".to_string()
        } else {
            full_path.to_string_lossy().to_string()
        }
    }
}

impl Filesystem for FuseLogFS {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        debug!("lookup(parent={}, name={:?})", parent, name);
        
        let mut inodes = self.inodes.lock().unwrap();
        
        let parent_path = match inodes.get_path(parent) {
            Some(p) => p.clone(),
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        
        let child_path = parent_path.join(name);
        
        match std::fs::metadata(&child_path) {
            Ok(metadata) => {
                let ino = inodes.get_or_create_ino(&child_path);
                let attrs = metadata_to_file_attr(ino, &metadata);
                reply.entry(&TTL, &attrs, 0);
            }
            Err(_) => reply.error(ENOENT),
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        debug!("getattr(ino={})", ino);
        
        let inodes = self.inodes.lock().unwrap();
        
        let path = match inodes.get_path(ino) {
            Some(p) => p.clone(),
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        
        match std::fs::metadata(&path) {
            Ok(metadata) => {
                let attrs = metadata_to_file_attr(ino, &metadata);
                reply.attr(&TTL, &attrs);
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(EIO)),
        }
    }

    fn readdir(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
        debug!("readdir(ino={}, offset={})", ino, offset);
        
        let mut inodes = self.inodes.lock().unwrap();
        
        let path = match inodes.get_path(ino) {
            Some(p) => p.clone(),
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        
        let mut entries = vec![];
        
        entries.push((ino, FileType::Directory, ".".to_string()));
        
        let parent_ino = if path == Path::new(".") {
            1
        } else {
            path.parent()
                .and_then(|p| inodes.path_to_ino.get(p))
                .copied()
                .unwrap_or(1)
        };
        entries.push((parent_ino, FileType::Directory, "..".to_string()));
        
        if let Ok(dir_iter) = std::fs::read_dir(&path) {
            for entry in dir_iter.filter_map(Result::ok) {
                let entry_path = if path == Path::new(".") {
                    PathBuf::from(entry.file_name())
                } else {
                    path.join(entry.file_name())
                };
                
                let entry_ino = inodes.get_or_create_ino(&entry_path);
                
                let file_type = if entry.file_type().map_or(false, |ft| ft.is_dir()) {
                    FileType::Directory
                } else {
                    FileType::RegularFile
                };
                
                if let Some(name) = entry.file_name().to_str() {
                    entries.push((entry_ino, file_type, name.to_string()));
                }
            }
        }
        
        for (i, (ino, kind, name)) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(ino, (i + 1) as i64, kind, &name) {
                break; 
            }
        }
        reply.ok();
    }

    fn mkdir(&mut self, _req: &Request, parent: u64, name: &OsStr, mode: u32, _umask: u32, reply: ReplyEntry) {
        debug!("mkdir(parent={}, name={:?}, mode={:o})", parent, name, mode);
        
        let mut inodes = self.inodes.lock().unwrap();
        
        let parent_path = match inodes.get_path(parent) {
            Some(p) => p.clone(),
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        
        let dir_path = parent_path.join(name);
        
        if dir_path.exists() {
            reply.error(EEXIST);
            return;
        }
        
        match std::fs::create_dir(&dir_path) {
            Ok(_) => {
                let ino = inodes.get_or_create_ino(&dir_path);
                
                if let Err(e) = std::fs::set_permissions(&dir_path, std::fs::Permissions::from_mode(mode)) {
                    debug!("Warning: failed to set directory permissions: {}", e);
                }
                
                match std::fs::metadata(&dir_path) {
                    Ok(metadata) => {
                        let attrs = metadata_to_file_attr(ino, &metadata);
                        info!("Created directory: {:?}", dir_path);
                        reply.entry(&TTL, &attrs, 0);
                    }
                    Err(e) => reply.error(e.raw_os_error().unwrap_or(EIO)),
                }
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(EIO)),
        }
    }

    fn rmdir(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        debug!("rmdir(parent={}, name={:?})", parent, name);
        
        let inodes = self.inodes.lock().unwrap();
        
        let parent_path = match inodes.get_path(parent) {
            Some(p) => p.clone(),
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        
        let dir_path = parent_path.join(name);
        
        match std::fs::remove_dir(&dir_path) {
            Ok(_) => {
                info!("Removed directory: {:?}", dir_path);
                reply.ok();
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(EIO)),
        }
    }

    fn open(&mut self, _req: &Request, ino: u64, _flags: i32, reply: ReplyOpen) {
        debug!("open(ino={})", ino);
        reply.opened(0, 0);
    }

    fn create(&mut self, _req: &Request, parent: u64, name: &OsStr, mode: u32, _umask: u32, flags: i32, reply: ReplyCreate) {
        debug!("create(parent={}, name={:?}, mode={:o})", parent, name, mode);
        
        let mut inodes = self.inodes.lock().unwrap();
        
        let parent_path = match inodes.get_path(parent) {
            Some(p) => p.clone(),
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        
        let file_path = parent_path.join(name);
        
        match std::fs::File::create(&file_path) {
            Ok(_) => {
                let ino = inodes.get_or_create_ino(&file_path);
                if let Ok(metadata) = std::fs::metadata(&file_path) {
                    let attrs = metadata_to_file_attr(ino, &metadata);
                    reply.created(&TTL, &attrs, 0, ino, flags as u32);
                } else {
                    reply.error(EIO);
                }
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(EIO)),
        }
    }

    fn read(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, size: u32, _flags: i32, _lock_owner: Option<u64>, reply: ReplyData) {
        debug!("read(ino={}, offset={}, size={})", ino, offset, size);
        
        let inodes = self.inodes.lock().unwrap();
        
        let path = match inodes.get_path(ino) {
            Some(p) => p.clone(),
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        
        use std::fs::File;
        use std::io::{Read, Seek, SeekFrom};
        
        match File::open(&path) {
            Ok(mut file) => {
                if let Err(e) = file.seek(SeekFrom::Start(offset as u64)) {
                    reply.error(e.raw_os_error().unwrap_or(EIO));
                    return;
                }
                
                let mut buffer = vec![0u8; size as usize];
                match file.read(&mut buffer) {
                    Ok(bytes_read) => {
                        buffer.truncate(bytes_read);
                        reply.data(&buffer);
                    }
                    Err(e) => reply.error(e.raw_os_error().unwrap_or(EIO)),
                }
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(EIO)),
        }
    }

    fn write(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, data: &[u8], _write_flags: u32, _flags: i32, _lock_owner: Option<u64>, reply: ReplyWrite) {
        debug!("write(ino={}, offset={}, size={})", ino, offset, data.len());
        
        let inodes = self.inodes.lock().unwrap();
        
        let path = match inodes.get_path(ino) {
            Some(p) => p.clone(),
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        
        let relative_path = self.get_relative_path(&path);
        
        use std::fs::OpenOptions;
        use std::io::{Read, Seek, SeekFrom, Write};
        
        let mut file = match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(f) => f,
            Err(e) => {
                reply.error(e.raw_os_error().unwrap_or(EIO));
                return;
            }
        };
        
        let mut old_data = Vec::new();
        if file.seek(SeekFrom::Start(offset as u64)).is_ok() {
            let _ = file.read_to_end(&mut old_data);
        }
        
        let mut diffs = Vec::new();
        let mut current_diff_start: Option<usize> = None;
        
        for i in 0..data.len() {
            let old_byte = old_data.get(i).cloned().unwrap_or(0);
            let new_byte = data[i];
            
            if old_byte != new_byte {
                if current_diff_start.is_none() {
                    current_diff_start = Some(i);
                }
            } else if let Some(start) = current_diff_start.take() {
                diffs.push((start, data[start..i].to_vec()));
            }
        }
        
        if let Some(start) = current_diff_start {
            diffs.push((start, data[start..].to_vec()));
        }
        
        if !diffs.is_empty() {
            let fid = get_fid(&relative_path);
            let mut log = STATEDIFF_LOG.lock().unwrap();
            
            for (diff_start, diff_data) in diffs {
                let diff_offset = offset as u64 + diff_start as u64;
                info!("Coalesced diff: fid={}, offset={}, size={}", fid, diff_offset, diff_data.len());
                log.actions.push(StateDiffAction::Write {
                    fid,
                    offset: diff_offset,
                    data: diff_data,
                });
            }
        }
        
        if file.seek(SeekFrom::Start(offset as u64)).is_ok() {
            match file.write(data) {
                Ok(bytes_written) => reply.written(bytes_written as u32),
                Err(e) => reply.error(e.raw_os_error().unwrap_or(EIO)),
            }
        } else {
            reply.error(EIO);
        }
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        debug!("unlink(parent={}, name={:?})", parent, name);
        
        let inodes = self.inodes.lock().unwrap();
        
        let parent_path = match inodes.get_path(parent) {
            Some(p) => p.clone(),
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        
        let file_path = parent_path.join(name);
        let relative_path = self.get_relative_path(&file_path);
        
        match std::fs::remove_file(&file_path) {
            Ok(_) => {
                let fid = get_fid(&relative_path);
                let mut log = STATEDIFF_LOG.lock().unwrap();
                log.actions.push(StateDiffAction::Unlink { fid });
                reply.ok();
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(EIO)),
        }
    }

    fn setattr(&mut self, _req: &Request, ino: u64, _mode: Option<u32>, _uid: Option<u32>, _gid: Option<u32>, size: Option<u64>, _atime: Option<fuser::TimeOrNow>, _mtime: Option<fuser::TimeOrNow>, _ctime: Option<SystemTime>, _fh: Option<u64>, _crtime: Option<SystemTime>, _chgtime: Option<SystemTime>, _bkuptime: Option<SystemTime>, _flags: Option<u32>, reply: ReplyAttr) {
        debug!("setattr(ino={}, size={:?})", ino, size);
        
        let inodes = self.inodes.lock().unwrap();
        
        let path = match inodes.get_path(ino) {
            Some(p) => p.clone(),
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        
        if let Some(new_size) = size {
            let relative_path = self.get_relative_path(&path);
            
            match std::fs::OpenOptions::new().write(true).open(&path) {
                Ok(file) => {
                    if let Err(e) = file.set_len(new_size) {
                        reply.error(e.raw_os_error().unwrap_or(EIO));
                        return;
                    }
                    
                    let fid = get_fid(&relative_path);
                    let mut log = STATEDIFF_LOG.lock().unwrap();
                    log.actions.push(StateDiffAction::Truncate { fid, size: new_size });
                }
                Err(e) => {
                    reply.error(e.raw_os_error().unwrap_or(EIO));
                    return;
                }
            }
        }
        
        match std::fs::metadata(&path) {
            Ok(metadata) => {
                let attrs = metadata_to_file_attr(ino, &metadata);
                reply.attr(&TTL, &attrs);
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(EIO)),
        }
    }

    fn release(&mut self, _req: &Request, _ino: u64, _fh: u64, _flags: i32, _lock_owner: Option<u64>, _flush: bool, reply: ReplyEmpty) {
        debug!("release called");
        reply.ok();
    }

    fn flush(&mut self, _req: &Request, _ino: u64, _fh: u64, _lock_owner: u64, reply: ReplyEmpty) {
        debug!("flush called");
        reply.ok();
    }
}
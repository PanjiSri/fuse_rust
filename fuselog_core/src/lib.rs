pub mod statediff;
pub mod socket;

use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyWrite, ReplyEmpty, ReplyCreate, ReplyOpen, ReplyEntry, ReplyDirectory, Request,
};
use std::sync::Mutex;
use libc::ENOENT;
use log::info;
use statediff::{StateDiffAction, StateDiffLog};
use std::ffi::OsStr;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

const TTL: Duration = Duration::from_secs(1);

static STATEDIFF_LOG: once_cell::sync::Lazy<Arc<Mutex<StateDiffLog>>> = once_cell::sync::Lazy::new(|| Arc::new(Mutex::new(StateDiffLog::default())));

fn get_fid(path: &str) -> u64 {
    let mut log = STATEDIFF_LOG.lock().unwrap();

    if let Some((fid, _)) = log.fid_map.iter().find(|(_, p)| p == &path) {
        return *fid;
    }

    let new_fid = log.fid_map.len() as u64 + 1;
    log.fid_map.insert(new_fid, path.to_string());
    new_fid
}

pub struct FuseLogFS;

impl Filesystem for FuseLogFS {
    fn getattr(&mut self, _req: &Request, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        info!("getattr(ino={})", ino);
        
        if ino == 1 {
            let attrs = FileAttr {
                ino: 1,
                size: 0,
                blocks: 0,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: FileType::Directory,
                perm: 0o755,
                nlink: 2,
                uid: 1000,
                gid: 1000,
                rdev: 0,
                flags: 0,
                blksize: 512,
            };
            reply.attr(&TTL, &attrs);
        } else if ino == 2 {
            match std::fs::metadata("test_file.txt") {
                Ok(metadata) => {
                    let attrs = FileAttr {
                        ino: 2,
                        size: metadata.len(),
                        blocks: (metadata.len() + 511) / 512,
                        atime: UNIX_EPOCH,
                        mtime: UNIX_EPOCH,
                        ctime: UNIX_EPOCH,
                        crtime: UNIX_EPOCH,
                        kind: FileType::RegularFile,
                        perm: 0o644,
                        nlink: 1,
                        uid: 1000,
                        gid: 1000,
                        rdev: 0,
                        flags: 0,
                        blksize: 512,
                    };
                    reply.attr(&TTL, &attrs);
                }
                Err(_) => {
                    reply.error(ENOENT);
                }
            }
        } else {
            reply.error(ENOENT);
        }
    }

    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        info!("lookup(parent={}, name={:?})", parent, name);
        
        if parent == 1 && name == "test_file.txt" {
            match std::fs::metadata("test_file.txt") {
                Ok(metadata) => {
                    let attrs = FileAttr {
                        ino: 2,
                        size: metadata.len(),
                        blocks: (metadata.len() + 511) / 512,
                        atime: UNIX_EPOCH,
                        mtime: UNIX_EPOCH,
                        ctime: UNIX_EPOCH,
                        crtime: UNIX_EPOCH,
                        kind: FileType::RegularFile,
                        perm: 0o644,
                        nlink: 1,
                        uid: 1000,
                        gid: 1000,
                        rdev: 0,
                        flags: 0,
                        blksize: 512,
                    };
                    reply.entry(&TTL, &attrs, 0);
                }
                Err(_) => {
                    info!("  -> File doesn't exist, returning ENOENT");
                    reply.error(ENOENT);
                }
            }
        } else {
            reply.error(ENOENT);
        }
    }

    fn create(&mut self, _req: &Request, parent: u64, name: &OsStr, _mode: u32, _umask: u32, _flags: i32, reply: ReplyCreate) {
        info!("create(parent={}, name={:?})", parent, name);
        
        if parent == 1 && name == "test_file.txt" {
            if let Err(e) = std::fs::File::create("test_file.txt") {
                reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                return;
            }
            
            let attrs = FileAttr {
                ino: 2,
                size: 0,
                blocks: 1,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: FileType::RegularFile,
                perm: 0o644,
                nlink: 1,
                uid: 1000,
                gid: 1000,
                rdev: 0,
                flags: 0,
                blksize: 512,
            };
            reply.created(&TTL, &attrs, 0, 2, 0);
        } else {
            reply.error(libc::ENOSYS);
        }
    }

    fn open(&mut self, _req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
        info!("open(ino={}, flags=0x{:x})", ino, flags);
        
        if ino == 2 {
            if !std::path::Path::new("test_file.txt").exists() && (flags & libc::O_CREAT) != 0 {
                info!("  -> Creating file because O_CREAT flag is set");
                match std::fs::File::create("test_file.txt") {
                    Ok(_) => info!("  -> File created successfully"),
                    Err(e) => {
                        info!("  -> Failed to create file: {}", e);
                        reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                        return;
                    }
                }
            }
            
            reply.opened(2, 0);
        } else {
            reply.error(ENOENT);
        }
    }

    fn flush(&mut self, _req: &Request, _ino: u64, _fh: u64, _lock_owner: u64, reply: ReplyEmpty) {
        info!("flush called");
        reply.ok();
    }

    fn release(&mut self, _req: &Request, _ino: u64, _fh: u64, _flags: i32, _lock_owner: Option<u64>, _flush: bool, reply: ReplyEmpty) {
        info!("release called");
        reply.ok();
    }

    fn setattr(&mut self, _req: &Request, ino: u64, mode: Option<u32>, uid: Option<u32>, gid: Option<u32>, 
               size: Option<u64>, _atime: Option<fuser::TimeOrNow>, _mtime: Option<fuser::TimeOrNow>, 
               _ctime: Option<std::time::SystemTime>, _fh: Option<u64>, _crtime: Option<std::time::SystemTime>, 
               _chgtime: Option<std::time::SystemTime>, _bkuptime: Option<std::time::SystemTime>, 
               _flags: Option<u32>, reply: ReplyAttr) {
        info!("setattr(ino={}, size={:?})", ino, size);
        
        if ino == 2 {
            if let Some(new_size) = size {
                info!("  -> Truncating file to size {}", new_size);
                match std::fs::OpenOptions::new().write(true).truncate(true).open("test_file.txt") {
                    Ok(_) => {
                        info!("  -> Truncation successful");
                        let fid = get_fid("test_file.txt");
                        let mut log = STATEDIFF_LOG.lock().unwrap();
                        log.actions.push(StateDiffAction::Truncate { fid, size: new_size });
                    }
                    Err(e) => {
                        info!("  -> Truncation failed: {}", e);
                        reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                        return;
                    }
                }
            }
            
            let attrs = FileAttr {
                ino: 2,
                size: size.unwrap_or(0),
                blocks: 1,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: FileType::RegularFile,
                perm: mode.unwrap_or(0o644) as u16,
                nlink: 1,
                uid: uid.unwrap_or(1000),
                gid: gid.unwrap_or(1000),
                rdev: 0,
                flags: 0,
                blksize: 512,
            };
            reply.attr(&TTL, &attrs);
        } else {
            reply.error(ENOENT);
        }
    }

    fn readdir(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
        info!("readdir(ino={}, offset={})", ino, offset);
        
        if ino == 1 {
            if offset == 0 {
                let _ = reply.add(1, 0, FileType::Directory, ".");
                let _ = reply.add(1, 1, FileType::Directory, "..");
                
                if std::fs::metadata("test_file.txt").is_ok() {
                    let _ = reply.add(2, 2, FileType::RegularFile, "test_file.txt");
                }
            }
            reply.ok();
        } else {
            reply.error(ENOENT);
        }
    }
    
    fn write(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        let path = "test_file.txt";
        info!("write(path={}, offset={}, size={}) - ENTRY", path, offset, data.len());

        use std::fs::OpenOptions;
        use std::io::{Read, Seek, SeekFrom, Write};

        let mut file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path) {
                Ok(f) => f,
                Err(e) => {
                    reply.error(e.raw_os_error().unwrap_or(libc::EIO));
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
            let old_byte = old_data.get(i).cloned().unwrap_or(0); // Default to 0 if reading past the end
            let new_byte = data[i];

            if old_byte != new_byte {
                if current_diff_start.is_none() {
                    current_diff_start = Some(i);
                }
            } else {
                if let Some(start) = current_diff_start.take() {
                    diffs.push((start, data[start..i].to_vec()));
                }
            }
        }
        if let Some(start) = current_diff_start {
            diffs.push((start, data[start..].to_vec()));
        }

        if !diffs.is_empty() {
            let fid = get_fid(path);
            let mut log = STATEDIFF_LOG.lock().unwrap();

            for (diff_start, diff_data) in diffs {
                let diff_offset = offset as u64 + diff_start as u64;
                info!(
                    "  -> Coalesced diff: fid={}, offset={}, size={}",
                    fid,
                    diff_offset,
                    diff_data.len()
                );
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
                Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
            }
        } else {
            reply.error(libc::EIO);
        }
    }
    
    fn unlink(&mut self, _req: &Request, _parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let path = match name.to_str() {
            Some(p) => p,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        info!("unlink(path={})", path);

        match std::fs::remove_file(path) {
            Ok(_) => {
                let fid = get_fid(path);
                let mut log = STATEDIFF_LOG.lock().unwrap();
                log.actions.push(StateDiffAction::Unlink { fid });
                reply.ok();
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
        }
    }
}
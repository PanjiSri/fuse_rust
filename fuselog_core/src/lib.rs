use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry, Request,
};
use libc::ENOENT;
use std::ffi::OsStr;
use std::time::{Duration, UNIX_EPOCH};

const ROOT_DIR_INO: u64 = 1;
const HELLO_TXT_INO: u64 = 2;
const HELLO_TXT_FILENAME: &str = "hello.txt";
const HELLO_TXT_CONTENT: &str = "Hello from your Rust FUSE filesystem!\n";

const TTL: Duration = Duration::from_secs(1);

pub struct FuseLogFS;

impl Filesystem for FuseLogFS {
    /// `getattr` is called by the kernel to get file attributes (like size, permissions).
    fn getattr(&mut self, _req: &Request, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        log::info!("getattr(ino={}, fh={:?})", ino, _fh);

        let attrs = match ino {
            ROOT_DIR_INO => FileAttr {
                ino: ROOT_DIR_INO,
                size: 0, 
                blocks: 0,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: FileType::Directory,
                perm: 0o755, 
                nlink: 2, 
                uid: 501, 
                gid: 20,  
                rdev: 0,
                flags: 0,
                blksize: 512,
            },
            HELLO_TXT_INO => FileAttr {
                ino: HELLO_TXT_INO,
                size: HELLO_TXT_CONTENT.len() as u64,
                blocks: 1,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: FileType::RegularFile,
                perm: 0o644, 
                nlink: 1,
                uid: 501,
                gid: 20,
                rdev: 0,
                flags: 0,
                blksize: 512,
            },
            _ => {
                reply.error(ENOENT);
                return;
            }
        };
        reply.attr(&TTL, &attrs);
    }

    /// `lookup` is called to find a file by name within a directory.
    fn lookup(&mut self, _req: &Request, parent_ino: u64, name: &OsStr, reply: ReplyEntry) {
        log::info!("lookup(parent_ino={}, name={:?})", parent_ino, name);

        if parent_ino == ROOT_DIR_INO && name.to_str() == Some(HELLO_TXT_FILENAME) {
            let attrs = FileAttr {
                ino: HELLO_TXT_INO,
                size: HELLO_TXT_CONTENT.len() as u64,
                blocks: 1,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: FileType::RegularFile,
                perm: 0o644,
                nlink: 1,
                uid: 501,
                gid: 20,
                rdev: 0,
                flags: 0,
                blksize: 512,
            };
            reply.entry(&TTL, &attrs, 0);
        } else {
            reply.error(ENOENT);
        }
    }

    /// `readdir` is called to list the contents of a directory.
    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        log::info!("readdir(ino={}, offset={})", ino, offset);

        if ino != ROOT_DIR_INO {
            reply.error(ENOENT);
            return;
        }

        let entries = vec![
            (ROOT_DIR_INO, FileType::Directory, "."),
            (ROOT_DIR_INO, FileType::Directory, ".."),
            (HELLO_TXT_INO, FileType::RegularFile, HELLO_TXT_FILENAME),
        ];

        for (i, entry) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(entry.0, (i + 1) as i64, entry.1, entry.2) {
                break;
            }
        }
        reply.ok();
    }

    /// `read` is called when the kernel wants to read the contents of a file.
    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        log::info!("read(ino={}, offset={}, size={})", ino, offset, size);

        if ino == HELLO_TXT_INO {
            let content = HELLO_TXT_CONTENT.as_bytes();
            let end = (offset as usize).saturating_add(size as usize);
            reply.data(&content[offset as usize..end.min(content.len())]);
        } else {
            reply.error(ENOENT);
        }
    }
}
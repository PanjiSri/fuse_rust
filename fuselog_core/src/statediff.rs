use bincode::{Decode, Encode};
use std::collections::HashMap;

#[derive(Encode, Decode, Debug)]
pub enum StateDiffAction {
    Write {
        fid: u64,
        offset: u64,
        data: Vec<u8>,
    },
    Unlink {
        fid: u64,
    },
    Rename {
        from_fid: u64,
        to_fid: u64,
    },
    Truncate {
        fid: u64,
        size: u64,
    },
    Link {
        source_fid: u64,
        new_link_fid: u64,
    },
    Chown {
        fid: u64,
        uid: u32,
        gid: u32,
    },
    Mkdir {
        fid: u64,
    },
    Rmdir {
        fid: u64,
    },
}

#[derive(Encode, Decode, Debug, Default)]
pub struct StateDiffLog {
    pub fid_map: HashMap<u64, String>,
    pub actions: Vec<StateDiffAction>,
}
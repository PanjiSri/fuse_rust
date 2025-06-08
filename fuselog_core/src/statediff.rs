use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug)]
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
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct StateDiffLog {
    pub fid_map: HashMap<u64, String>,
    pub actions: Vec<StateDiffAction>,
}
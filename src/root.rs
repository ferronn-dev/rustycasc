use std::{collections::HashMap, convert::TryInto};

use anyhow::{ensure, Context, Result};
use bytes::Buf;

struct RootData {
    fdid: u32,
    content_key: u128,
    name_hash: Option<u64>,
}

pub struct Root {
    data: Vec<RootData>,
    fmap: HashMap<u32, usize>,
    nmap: HashMap<u64, usize>,
}

impl Root {
    pub fn f2c(&self, fdid: u32) -> Result<u128> {
        Ok(self.data[*self.fmap.get(&fdid).context("missing fdid in root")?].content_key)
    }
    pub fn n2c(&self, name: &str) -> Result<u128> {
        let hash = hashers::jenkins::lookup3(name.to_uppercase().as_bytes());
        Ok(self.data[*self.nmap.get(&hash).context("missing name hash in root")?].content_key)
    }
}

pub fn parse(data: &[u8]) -> Result<Root> {
    let mut p = data;
    ensure!(p.remaining() >= 4, "empty root?");
    let interleave;
    let can_skip;
    if p[..4] == *b"TSFM" {
        p.advance(4);
        ensure!(p.remaining() >= 8, "truncated root header");
        let total_file_count = p.get_u32_le();
        let named_file_count = p.get_u32_le();
        interleave = false;
        can_skip = total_file_count != named_file_count;
    } else {
        interleave = true;
        can_skip = false;
    }
    let mut result = Vec::<RootData>::new();
    while p.has_remaining() {
        ensure!(p.remaining() >= 12, "truncated root cas block");
        let num_records: usize = p.get_u32_le().try_into()?;
        let content_flags = p.get_u32_le();
        let _locale_flags = p.get_u32_le();
        ensure!(
            p.remaining() >= 4 * num_records,
            "truncated filedataid delta block"
        );
        let mut fdids = Vec::<u32>::new();
        let mut fdid = -1;
        for _ in 0..num_records {
            fdid = fdid + p.get_i32_le() + 1;
            fdids.push(fdid.try_into()?)
        }
        let mut content_keys = Vec::<u128>::new();
        let mut name_hashes = Vec::<Option<u64>>::new();
        if interleave {
            for _ in 0..num_records {
                content_keys.push(p.get_u128());
                name_hashes.push(Some(p.get_u64_le()));
            }
        } else {
            for _ in 0..num_records {
                content_keys.push(p.get_u128());
            }
            if !can_skip || content_flags & 0x10000000 == 0 {
                for _ in 0..num_records {
                    name_hashes.push(Some(p.get_u64_le()));
                }
            } else {
                name_hashes.resize(num_records, None);
            }
        }
        for i in 0..num_records {
            result.push(RootData {
                fdid: fdids[i],
                content_key: content_keys[i],
                name_hash: name_hashes[i],
            })
        }
    }
    Ok(Root {
        fmap: result
            .iter()
            .enumerate()
            .map(|(k, d)| (d.fdid, k))
            .collect(),
        nmap: result
            .iter()
            .enumerate()
            .filter_map(|(k, d)| d.name_hash.map(|h| (h, k)))
            .collect(),
        data: result,
    })
}

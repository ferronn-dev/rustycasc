use std::convert::TryInto;

use anyhow::{bail, ensure, Result};
use bytes::Buf;

struct RootData {
    fdid: u32,
    content_key: u128,
    _name_hash: u64,
}

pub struct Root(Vec<RootData>);

impl Root {
    pub fn f2c(&self, fdid: u32) -> Result<u128> {
        for d in &self.0 {
            if d.fdid == fdid {
                return Ok(d.content_key);
            }
        }
        bail!("no content key for fdid {}", fdid)
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
        let mut name_hashes = Vec::<u64>::new();
        if interleave {
            for _ in 0..num_records {
                content_keys.push(p.get_u128());
                name_hashes.push(p.get_u64_le());
            }
        } else {
            for _ in 0..num_records {
                content_keys.push(p.get_u128());
            }
            if !can_skip || content_flags & 0x10000000 == 0 {
                for _ in 0..num_records {
                    name_hashes.push(p.get_u64_le());
                }
            } else {
                name_hashes.resize(num_records, 0);
            }
        }
        for i in 0..num_records {
            result.push(RootData {
                fdid: fdids[i],
                content_key: content_keys[i],
                _name_hash: name_hashes[i],
            })
        }
    }
    Ok(Root(result))
}

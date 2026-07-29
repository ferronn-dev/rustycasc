use std::{collections::HashMap, convert::TryInto};

use crate::types::{ContentKey, FileDataID};
use anyhow::{ensure, Context, Result};
use bytes::Buf;

struct RootData {
    fdid: FileDataID,
    content_key: ContentKey,
    name_hash: Option<u64>,
}

pub(crate) struct Root {
    data: Vec<RootData>,
    fmap: HashMap<FileDataID, usize>,
    nmap: HashMap<u64, usize>,
}

impl Root {
    pub(crate) fn f2c(&self, fdid: FileDataID) -> Result<ContentKey> {
        Ok(self.data[*self.fmap.get(&fdid).context("missing fdid in root")?].content_key)
    }
    pub(crate) fn n2c(&self, name: &str) -> Result<ContentKey> {
        let hash: u64 = hashers::jenkins::lookup3(name.to_uppercase().as_bytes());
        // The hi and lo words are swapped for some reason.
        let hi = (hash >> 32) as u32;
        let lo = (hash & 0xffffffff) as u32;
        let hash: u64 = ((lo as u64) << 32) | (hi as u64);
        Ok(self.data[*self
            .nmap
            .get(&hash)
            .with_context(|| format!("missing name hash in root: {name}"))?]
        .content_key)
    }
}

pub(crate) fn parse(data: &[u8]) -> Result<Root> {
    let mut p = data;
    ensure!(p.remaining() >= 24, "truncated root header");
    ensure!(p[..4] == *b"TSFM", "unsupported root format");
    p.advance(4);
    let header_size = p.get_u32_le();
    ensure!(
        header_size == 24,
        "unexpected root header size {header_size}"
    );
    let version = p.get_u32_le();
    ensure!(
        version == 1 || version == 2,
        "unsupported root version {version}"
    );
    let total_file_count = p.get_u32_le();
    let named_file_count = p.get_u32_le();
    p.advance(4); // reserved
    let can_skip = total_file_count != named_file_count;
    let block_header_size = if version == 1 { 12 } else { 17 };
    let mut result = Vec::<RootData>::new();
    while p.has_remaining() {
        ensure!(
            p.remaining() >= block_header_size,
            "truncated root cas block"
        );
        let num_records: usize = p.get_u32_le().try_into()?;
        let content_flags = if version == 1 {
            let flags = p.get_u32_le();
            let _locale_flags = p.get_u32_le();
            flags
        } else {
            let _locale_flags = p.get_u32_le();
            let flags_lo = p.get_u32_le();
            let flags_hi = p.get_u32_le();
            let flags_ext = p.get_u8();
            flags_lo | flags_hi | ((flags_ext as u32) << 17)
        };
        ensure!(
            p.remaining() >= 4 * num_records,
            "truncated filedataid delta block"
        );
        let mut fdids = Vec::<FileDataID>::new();
        let mut fdid = -1;
        for _ in 0..num_records {
            fdid = fdid + p.get_i32_le() + 1;
            fdids.push(FileDataID(fdid.try_into()?))
        }
        let mut content_keys = Vec::<ContentKey>::new();
        for _ in 0..num_records {
            content_keys.push(ContentKey(p.get_u128()));
        }
        let mut name_hashes = Vec::<Option<u64>>::new();
        if !can_skip || content_flags & 0x10000000 == 0 {
            for _ in 0..num_records {
                name_hashes.push(Some(p.get_u64_le()));
            }
        } else {
            name_hashes.resize(num_records, None);
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

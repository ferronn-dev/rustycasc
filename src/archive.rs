use std::{collections::HashMap, convert::TryInto};

use anyhow::{ensure, Result};
use bytes::Buf;

use crate::types::EncodingKey;
use crate::util;

#[derive(Debug)]
pub(crate) struct Index {
    pub(crate) map: HashMap<EncodingKey, (u128, usize, usize)>,
}

pub(crate) fn parse_index(name: u128, data: &[u8]) -> Result<Index> {
    ensure!(data.len() >= 28, "truncated archive index data");
    let non_footer_size = data.len() - 28;
    let bytes_per_block = 4096 + 24;
    let num_blocks = non_footer_size / bytes_per_block;
    ensure!(
        non_footer_size % bytes_per_block == 0,
        "invalid archive index format"
    );
    let mut footer = &data[non_footer_size..];
    ensure!(util::md5hash(footer) == name, "bad footer name");
    let toc_size = num_blocks * 24;
    let toc = &data[non_footer_size - toc_size..non_footer_size];
    ensure!(
        (util::md5hash(toc) >> 64) as u64 == footer.get_u64(),
        "archive index toc checksum"
    );
    ensure!(footer.get_u8() == 1, "unexpected archive index version");
    ensure!(
        footer.get_u8() == 0,
        "unexpected archive index nonzero byte"
    );
    ensure!(
        footer.get_u8() == 0,
        "unexpected archive index nonzero byte"
    );
    ensure!(footer.get_u8() == 4, "unexpected archive index block size");
    ensure!(
        footer.get_u8() == 4,
        "unexpected archive index offset bytes"
    );
    ensure!(footer.get_u8() == 4, "unexpected archive index size bytes");
    ensure!(footer.get_u8() == 16, "unexpected archive index key size");
    ensure!(
        footer.get_u8() == 8,
        "unexpected archive index checksum size"
    );
    let num_elements = footer.get_u32_le().try_into()?;
    let footer_checksum = footer.get_u64();
    assert!(!footer.has_remaining());
    {
        let mut footer_to_check = data[non_footer_size + 8..non_footer_size + 20].to_vec();
        footer_to_check.resize(20, 0);
        ensure!(
            (util::md5hash(&footer_to_check) >> 64) as u64 == footer_checksum,
            "archive index footer checksum"
        );
    };
    let mut map = HashMap::<EncodingKey, (u128, usize, usize)>::new();
    let mut p = &data[..non_footer_size - toc_size];
    let mut entries = &toc[..(16 * num_blocks)];
    let mut blockhashes = &toc[(16 * num_blocks)..];
    for _ in 0..num_blocks {
        let mut block = &p[..4096];
        let block_checksum = blockhashes.get_u64();
        ensure!(
            (util::md5hash(block) >> 64) as u64 == block_checksum,
            "archive index block checksum"
        );
        let last_ekey = EncodingKey(entries.get_u128());
        let mut found = false;
        while block.remaining() >= 24 {
            let ekey = EncodingKey(block.get_u128());
            let size = block.get_u32().try_into()?;
            let offset = block.get_u32().try_into()?;
            ensure!(
                map.insert(ekey, (name, size, offset)).is_none(),
                "duplicate key in index"
            );
            if ekey == last_ekey {
                found = true;
                break;
            }
        }
        ensure!(found, "last ekey mismatch");
        p.advance(4096);
    }
    assert!(!p.has_remaining());
    assert!(!entries.has_remaining());
    assert!(!blockhashes.has_remaining());
    ensure!(map.len() == num_elements, "num_elements wrong in index");
    Ok(Index { map })
}

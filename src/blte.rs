use crate::util;
use anyhow::{anyhow, bail, ensure, Result};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::convert::TryInto;

fn parse_blte_chunk(data: &[u8]) -> Result<bytes::Bytes> {
    let inflate = miniz_oxide::inflate::decompress_to_vec_zlib;
    let chunk_data = &data[1..];
    Ok(match data[0] as char {
        'N' => Bytes::from(chunk_data.to_vec()),
        'Z' => Bytes::from(
            inflate(&chunk_data).map_err(|s| anyhow!(format!("inflate error {:?}", s)))?,
        ),
        _ => bail!("invalid encoding"),
    })
}

pub fn parse(data: &[u8]) -> Result<Vec<u8>> {
    let mut p = data;
    ensure!(p.remaining() >= 12, "truncated header");
    ensure!(&p.get_u32().to_be_bytes() == b"BLTE", "not BLTE format");
    let header_size = p.get_u32();
    if header_size == 0 {
        return Ok(parse_blte_chunk(p)?.to_vec());
    }
    ensure!(p.get_u8() == 0xf, "bad flag byte");
    let chunk_count = (u32::from(p.get_u8()) << 16) | u32::from(p.get_u16());
    ensure!(header_size == chunk_count * 24 + 12, "header size mismatch");
    let mut chunkinfo = Vec::<(usize, usize, u128)>::new();
    for _ in 0..chunk_count {
        let compressed_size = p.get_u32().try_into()?;
        let uncompressed_size = p.get_u32().try_into()?;
        let checksum = p.get_u128();
        chunkinfo.push((compressed_size, uncompressed_size, checksum))
    }
    let mut result = BytesMut::with_capacity(chunkinfo.iter().map(|x| x.1).sum::<usize>());
    for (compressed_size, uncompressed_size, checksum) in chunkinfo {
        let chunk = &p[0..compressed_size];
        ensure!(checksum == util::md5hash(chunk), "chunk checksum error");
        let data = parse_blte_chunk(chunk)?;
        ensure!(data.len() == uncompressed_size, "invalid uncompressed size");
        result.put(data);
        p.advance(compressed_size)
    }
    ensure!(!p.has_remaining(), "trailing blte data");
    Ok(result.to_vec())
}

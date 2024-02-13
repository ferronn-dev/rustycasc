use std::{collections::HashMap, convert::TryInto};

use anyhow::{ensure, Context, Result};
use bytes::Buf;

use crate::types::{ContentKey, EncodingKey};
use crate::util;

#[derive(Debug)]
pub(crate) struct Encoding {
    _especs: Vec<String>,
    cmap: HashMap<ContentKey, (Vec<EncodingKey>, u64)>,
    _emap: HashMap<u128, (usize, u64)>,
    _espec: String,
}

impl Encoding {
    pub(crate) fn c2e(&self, c: ContentKey) -> Result<EncodingKey> {
        Ok(*self
            .cmap
            .get(&c)
            .context(format!("no encoding key for content key {}", c))?
            .0
            .first()
            .context(format!("missing encoding key for content key {}", c))?)
    }
}

pub(crate) fn parse(data: &[u8]) -> Result<Encoding> {
    let mut p = data;
    ensure!(p.remaining() >= 16, "truncated encoding header");
    ensure!(&p.get_u16().to_be_bytes() == b"EN", "not encoding format");
    ensure!(p.get_u8() == 1, "unsupported encoding version");
    ensure!(p.get_u8() == 16, "unsupported ckey hash size");
    ensure!(p.get_u8() == 16, "unsupported ekey hash size");
    let cpagekb: usize = p.get_u16().into();
    let epagekb: usize = p.get_u16().into();
    let ccount: usize = p.get_u32().try_into()?;
    let ecount: usize = p.get_u32().try_into()?;
    ensure!(p.get_u8() == 0, "unexpected nonzero byte in header");
    let espec_size = p.get_u32().try_into()?;
    ensure!(p.remaining() >= espec_size, "truncated espec table");
    let especs = p[0..espec_size]
        .split(|b| *b == 0)
        .map(|s| String::from_utf8(s.to_vec()).context("parsing encoding espec"))
        .collect::<Result<Vec<String>>>()?;
    p.advance(espec_size);
    ensure!(p.remaining() >= ccount * 32);
    let mut cpages = Vec::<(ContentKey, u128)>::new();
    for _ in 0..ccount {
        cpages.push((ContentKey(p.get_u128()), p.get_u128()));
    }
    let mut cmap = HashMap::<ContentKey, (Vec<EncodingKey>, u64)>::new();
    for (first_key, hash) in cpages {
        let pagesize = cpagekb * 1024;
        ensure!(
            hash == util::md5hash(&p[0..pagesize]),
            "content page checksum"
        );
        let mut page = p.take(pagesize);
        let mut first = true;
        while page.remaining() >= 22 && page.chunk()[0] != b'0' {
            let key_count = page.get_u8().into();
            let file_size = (u64::from(page.get_u8()) << 32) | u64::from(page.get_u32());
            let ckey = ContentKey(page.get_u128());
            ensure!(!first || first_key == ckey, "first key mismatch in content");
            first = false;
            ensure!(page.remaining() >= key_count * 16_usize);
            let mut ekeys = Vec::<EncodingKey>::new();
            for _ in 0..key_count {
                ekeys.push(EncodingKey(page.get_u128()));
            }
            cmap.insert(ckey, (ekeys, file_size));
        }
        p.advance(pagesize)
    }
    ensure!(p.remaining() >= ecount * 32);
    let mut epages = Vec::<(u128, u128)>::new();
    for _ in 0..ecount {
        epages.push((p.get_u128(), p.get_u128()));
    }
    let mut emap = HashMap::<u128, (usize, u64)>::new();
    for (first_key, hash) in epages {
        let pagesize = epagekb * 1024;
        ensure!(
            hash == util::md5hash(&p[0..pagesize]),
            "encoding page checksum"
        );
        let mut page = p.take(pagesize);
        let mut first = true;
        while page.remaining() >= 25 && page.chunk()[0] != b'0' {
            let ekey = page.get_u128();
            let index = page.get_u32().try_into()?;
            let file_size = (u64::from(page.get_u8()) << 32) | u64::from(page.get_u32());
            ensure!(!first || first_key == ekey, "first key mismatch in content");
            first = false;
            emap.insert(ekey, (index, file_size));
        }
        p.advance(pagesize)
    }
    let espec = String::from_utf8(p.to_vec())?;
    Ok(Encoding {
        _especs: especs,
        cmap,
        _emap: emap,
        _espec: espec,
    })
}

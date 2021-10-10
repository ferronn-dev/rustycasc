use anyhow::{anyhow, bail, ensure, Context, Result};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::collections::HashMap;
use std::convert::TryInto;
use structopt::StructOpt;

fn parse_info(s: &str) -> Vec<HashMap<&str, &str>> {
    if s == "" {
        // Empty string special case because lines() returns an empty iterator.
        return vec![];
    }
    let mut lines = s.lines().map(|x| x.split("|"));
    let tags = lines
        .next()
        .unwrap()
        .map(|x| x.split("!").next().unwrap())
        .collect::<Vec<&str>>();
    lines
        .skip(1)
        .map(|v| tags.iter().map(|x| *x).zip(v).collect())
        .collect()
}

fn parse_config(s: &str) -> HashMap<&str, &str> {
    s.lines().filter_map(|x| x.split_once(" = ")).collect()
}

struct BuildConfig {
    root: u128,
    encoding: u128,
}

fn parse_hash(s: &str) -> Result<u128> {
    u128::from_str_radix(s, 16).context("parse hash")
}

fn parse_build_config(config: &HashMap<&str, &str>) -> Result<BuildConfig> {
    Ok(BuildConfig {
        root: parse_hash(config.get("root").context("build config: root")?)?,
        encoding: parse_hash(
            config
                .get("encoding")
                .context("missing encoding field in buildinfo")?
                .split(" ")
                .nth(1)
                .context("missing data in encoding field in buildinfo")?,
        )?,
    })
}

fn md5hash(p: &[u8]) -> u128 {
    u128::from_be_bytes(*md5::compute(p))
}

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

fn parse_blte(data: &[u8]) -> Result<Vec<u8>> {
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
        ensure!(checksum == md5hash(chunk), "chunk checksum error");
        let data = parse_blte_chunk(chunk)?;
        ensure!(data.len() == uncompressed_size, "invalid uncompressed size");
        result.put(data);
        p.advance(compressed_size)
    }
    ensure!(!p.has_remaining(), "trailing blte data");
    Ok(result.to_vec())
}

#[derive(Debug)]
struct Encoding {
    especs: Vec<String>,
    cmap: HashMap<u128, (Vec<u128>, u64)>,
    emap: HashMap<u128, (usize, u64)>,
    espec: String,
}

impl Encoding {
    fn c2e(&self, c: u128) -> Result<u128> {
        Ok(*self
            .cmap
            .get(&c)
            .context(format!("no encoding key for content key {:032x}", c))?
            .0
            .get(0)
            .context(format!("missing encoding key for content key {:032x}", c))?)
    }
}

fn parse_encoding(data: &[u8]) -> Result<Encoding> {
    let mut p = data;
    ensure!(p.remaining() >= 16, "truncated encoding header");
    ensure!(&p.get_u16().to_be_bytes() == b"EN", "not encoding format");
    ensure!(p.get_u8() == 1, "unsupported encoding version");
    ensure!(p.get_u8() == 16, "unsupported ckey hash size");
    ensure!(p.get_u8() == 16, "unsupported ekey hash size");
    let cpagekb: usize = p.get_u16().try_into()?;
    let epagekb: usize = p.get_u16().try_into()?;
    let ccount: usize = p.get_u32().try_into()?;
    let ecount: usize = p.get_u32().try_into()?;
    ensure!(p.get_u8() == 0, "unexpected nonzero byte in header");
    let espec_size = p.get_u32().try_into()?;
    ensure!(p.remaining() >= espec_size, "truncated espec table");
    let especs = p[0..espec_size]
        .split(|b| *b == b'0')
        .map(|s| String::from_utf8(s.to_vec()).context("parsing encoding espec"))
        .collect::<Result<Vec<String>>>()?;
    p.advance(espec_size);
    ensure!(p.remaining() >= ccount * 32);
    let mut cpages = Vec::<(u128, u128)>::new();
    for _ in 0..ccount {
        cpages.push((p.get_u128(), p.get_u128()));
    }
    let mut cmap = HashMap::<u128, (Vec<u128>, u64)>::new();
    for (first_key, hash) in cpages {
        let pagesize = cpagekb * 1024;
        ensure!(hash == md5hash(&p[0..pagesize]), "content page checksum");
        let mut page = p.take(pagesize);
        let mut first = true;
        while page.remaining() >= 22 && page.chunk()[0] != b'0' {
            let key_count = page.get_u8().try_into()?;
            let file_size = (u64::from(page.get_u8()) << 32) | u64::from(page.get_u32());
            let ckey = page.get_u128();
            ensure!(!first || first_key == ckey, "first key mismatch in content");
            first = false;
            ensure!(page.remaining() >= key_count * 16 as usize);
            let mut ekeys = Vec::<u128>::new();
            for _ in 0..key_count {
                ekeys.push(page.get_u128());
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
        ensure!(hash == md5hash(&p[0..pagesize]), "encoding page checksum");
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
        especs,
        cmap,
        emap,
        espec,
    })
}

struct RootData {
    fdid: i32,
    content_key: u128,
    _name_hash: u64,
}

struct Root(Vec<RootData>);

impl Root {
    fn f2c(&self, fdid: i32) -> Result<u128> {
        for d in &self.0 {
            if d.fdid == fdid {
                return Ok(d.content_key);
            }
        }
        bail!("no content key for fdid {}", fdid)
    }
}

fn parse_root(data: &[u8]) -> Result<Root> {
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
        let mut fdids = Vec::<i32>::new();
        let mut fdid = -1;
        for _ in 0..num_records {
            fdid = fdid + p.get_i32_le() + 1;
            fdids.push(fdid)
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

#[derive(Debug)]
struct ArchiveIndex {
    map: HashMap<u128, (usize, usize)>,
}

fn parse_archive_index(data: &[u8]) -> Result<ArchiveIndex> {
    ensure!(data.len() >= 28, "truncated archive index data");
    let non_footer_size = data.len() - 28;
    let bytes_per_block = 4096 + 24;
    let _num_blocks = non_footer_size / bytes_per_block;
    ensure!(
        non_footer_size % bytes_per_block == 0,
        "invalid archive index format"
    );
    let mut footer = &data[non_footer_size..];
    let _toc_checksum = footer.get_u64();
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
    let _num_elements = footer.get_u32_le();
    let _footer_checksum = footer.get_u64();
    assert!(!footer.has_remaining());
    let mut map = HashMap::<u128, (usize, usize)>::new();
    let mut p = data;
    while p.remaining() >= 100 {
        let ekey = p.get_u128();
        let size = p.get_u32_le().try_into()?;
        let offset = p.get_u32_le().try_into()?;
        map.insert(ekey, (size, offset));
    }
    Ok(ArchiveIndex { map })
}

#[derive(StructOpt)]
struct Cli {
    product: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::from_args_safe()?;
    let patch_base = format!("http://us.patch.battle.net:1119/{}", cli.product);
    let client = reqwest::Client::new();
    let fetch = |url| async {
        let send_ctx = format!("sending request to {}", url);
        let status_ctx = format!("http error on {}", url);
        let recv_ctx = format!("receiving content on {}", url);
        let response = client.get(url).send().await.context(send_ctx)?;
        ensure!(response.status().is_success(), status_ctx);
        response.bytes().await.context(recv_ctx)
    };
    let utf8 = std::str::from_utf8;
    let (versions, cdns) = futures::join!(
        fetch(format!("{}/versions", patch_base)),
        fetch(format!("{}/cdns", patch_base))
    );
    let (versions, cdns) = (versions?, cdns?);
    let (build_config, cdn_config) = (|| {
        let info = utf8(&*versions)?;
        let version = parse_info(info)
            .into_iter()
            .find(|m| m.get("Region") == Some(&"us"))
            .context("missing us version")?;
        let build = parse_hash(
            version
                .get("BuildConfig")
                .context("missing us build config version")?,
        )?;
        let cdn = parse_hash(
            version
                .get("CDNConfig")
                .context("missing us cdn config version")?,
        )?;
        Result::<(u128, u128)>::Ok((build, cdn))
    })()?;
    let ref cdn_prefixes = (|| {
        let info = utf8(&*cdns)?;
        let cdn = parse_info(info)
            .into_iter()
            .find(|m| m.get("Name") == Some(&"us"))
            .context("missing us cdn")?;
        let hosts = cdn.get("Hosts").context("missing us cdn hosts")?.split(" ");
        let path = cdn.get("Path").context("missing us cdn path")?;
        Result::<Vec<String>>::Ok(hosts.map(|s| format!("http://{}/{}", s, path)).collect())
    })()?;
    let do_cdn_fetch = |path: String, cache_file: String| async move {
        let cache_file = format!("cache/{}", cache_file);
        let cached = std::fs::read(&cache_file);
        if cached.is_ok() {
            return Result::<Bytes>::Ok(Bytes::from(cached.unwrap()));
        }
        for cdn_prefix in cdn_prefixes {
            let data = fetch(format!("{}/{}", cdn_prefix, path)).await;
            if data.is_ok() {
                let data = data.unwrap();
                std::fs::write(&cache_file, &data)?;
                return Ok(data);
            }
        }
        bail!("fetch failed on all hosts: {}", path)
    };
    let cdn_fetch = |tag: &'static str, hash: u128, suffix: &'static str| async move {
        let h = format!("{:032x}", hash);
        let path = format!("{}/{}/{}/{}{}", tag, &h[0..2], &h[2..4], h, suffix);
        let cache_file = format!("{}{}.{}", tag, suffix, h);
        do_cdn_fetch(path, cache_file).await
    };
    let archive_index = async {
        let archives = parse_config(&utf8(&(cdn_fetch("config", cdn_config, "").await?))?)
            .get("archives")
            .context("missing archives in cdninfo")?
            .split(" ")
            .map(parse_hash)
            .collect::<Result<Vec<u128>>>()?;
        let indices =
            futures::future::join_all(archives.into_iter().map(|h| async move {
                parse_archive_index(&(cdn_fetch("data", h, ".index").await?))
            }))
            .await
            .into_iter()
            .collect::<Result<Vec<ArchiveIndex>>>()?;
        println!("{:#?}", indices[0]);
        Result::<ArchiveIndex>::Ok(ArchiveIndex {
            map: indices[0].map.clone(),
        })
    };
    let encoding_and_root = async {
        let buildinfo = parse_build_config(&parse_config(&utf8(
            &(cdn_fetch("config", build_config, "").await?),
        )?))?;
        let encoding = parse_encoding(&parse_blte(
            &(cdn_fetch("data", buildinfo.encoding, "").await?),
        )?)?;
        let root = parse_root(&parse_blte(
            &cdn_fetch("data", encoding.c2e(buildinfo.root)?, "").await?,
        )?)?;
        Result::<(Encoding, Root)>::Ok((encoding, root))
    };
    let (archive_index, encoding_and_root) = futures::join!(archive_index, encoding_and_root);
    let (_archive_index, (encoding, root)) = (archive_index?, encoding_and_root?);
    let _tocbase = cdn_fetch("data", encoding.c2e(root.f2c(1267335)?)?, "").await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use velcro::hash_map as m;
    use velcro::vec as v;

    #[test]
    fn test_parse_info() {
        let tests = [
            ("empty string", "", v![]),
            ("space", " ", v![]),
            ("one field", "moo\n\ncow", v![m! {"moo":"cow"}]),
            (
                "several fields",
                "f1!x|f2!y\n\nv11|v12\nv21|v22",
                v![m! {"f1":"v11", "f2":"v12"}, m! {"f1":"v21", "f2":"v22"},],
            ),
        ];
        for (name, input, output) in tests {
            assert_eq!(super::parse_info(input), output, "{}", name);
        }
    }

    #[test]
    fn test_parse_config() {
        let tests = [
            ("empty string", "", m! {}),
            ("space", " ", m! {}),
            ("one field", "foo\n\nbar = baz\nx=y", m! {"bar":"baz"}),
        ];
        for (name, input, output) in tests {
            assert_eq!(super::parse_config(input), output, "{}", name);
        }
    }
}

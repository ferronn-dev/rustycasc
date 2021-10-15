mod blte;
mod encoding;
mod util;
mod wdc3;

use anyhow::{bail, ensure, Context, Result};
use bytes::{Buf, Bytes};
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
    map: HashMap<u128, (u128, usize, usize)>,
}

fn parse_archive_index(name: u128, data: &[u8]) -> Result<ArchiveIndex> {
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
    let mut map = HashMap::<u128, (u128, usize, usize)>::new();
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
        let last_ekey = entries.get_u128();
        let mut found = false;
        while block.remaining() >= 24 {
            let ekey = block.get_u128();
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
    Ok(ArchiveIndex { map })
}

#[derive(StructOpt)]
struct Cli {
    product: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::from_args_safe()?;
    if cli.product == "db2" {
        println!(
            "{:#?}",
            wdc3::strings(&std::fs::read("ManifestInterfaceTOCData.db2")?)?
        );
        return Ok(());
    }
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
        let index_shards = futures::future::join_all(
            parse_config(&utf8(&(cdn_fetch("config", cdn_config, "").await?))?)
                .get("archives")
                .context("missing archives in cdninfo")?
                .split(" ")
                .map(|s| async move {
                    let h = parse_hash(s)?;
                    parse_archive_index(h, &(cdn_fetch("data", h, ".index").await?))
                }),
        )
        .await;
        let mut map = HashMap::<u128, (u128, usize, usize)>::new();
        for r in index_shards.into_iter() {
            map.extend(r?.map.drain());
        }
        return Result::<ArchiveIndex>::Ok(ArchiveIndex { map });
    };
    let encoding_and_root = async {
        let buildinfo = parse_build_config(&parse_config(&utf8(
            &(cdn_fetch("config", build_config, "").await?),
        )?))?;
        let encoding = encoding::parse(&blte::parse(
            &(cdn_fetch("data", buildinfo.encoding, "").await?),
        )?)?;
        let root = parse_root(&blte::parse(
            &cdn_fetch("data", encoding.c2e(buildinfo.root)?, "").await?,
        )?)?;
        Result::<(encoding::Encoding, Root)>::Ok((encoding, root))
    };
    let (archive_index, encoding_and_root) = futures::join!(archive_index, encoding_and_root);
    let (archive_index, (encoding, root)) = (archive_index?, encoding_and_root?);
    {
        let (archive, size, offset) = archive_index
            .map
            .get(&encoding.c2e(root.f2c(1267335)?)?)
            .context("missing index key")?;
        let h = format!("{:032x}", archive);
        let url = format!("{}/data/{}/{}/{}", cdn_prefixes[0], &h[0..2], &h[2..4], h);
        let response = client
            .get(url)
            .header("Range", format!("bytes={}-{}", offset, offset + size - 1))
            .send()
            .await
            .context("send fail")?;
        ensure!(response.status().is_success(), "status fail");
        std::fs::write(
            "ManifestInterfaceTOCData.db2",
            &blte::parse(&response.bytes().await.context("recv fail")?)?,
        )?;
    }
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

mod archive;
mod blte;
mod encoding;
mod root;
mod util;
mod wdc3;

use anyhow::{bail, ensure, Context, Result};
use bytes::Bytes;
use log::trace;
use std::collections::HashMap;
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

#[derive(StructOpt)]
struct Cli {
    product: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::from_args_safe()?;
    stderrlog::new()
        .module(module_path!())
        .verbosity(100)
        .init()?;
    let patch_base = format!("http://us.patch.battle.net:1119/{}", cli.product);
    let ref client = reqwest::Client::new();
    let fetch = |url| async {
        let urlcopy = format!("{}", url);
        trace!("starting fetch of {}", urlcopy);
        let response = client
            .get(url)
            .send()
            .await
            .context(format!("sending request to {}", urlcopy))?;
        ensure!(
            response.status().is_success(),
            format!("http error on {}", urlcopy)
        );
        trace!("receiving content on {}", urlcopy);
        let data = response
            .bytes()
            .await
            .context(format!("receiving content on {}", urlcopy))?;
        trace!("done retrieving {}", urlcopy);
        Ok(data)
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
        trace!("cdn fetch {}", path);
        let cache_file = format!("cache/{}", cache_file);
        let cached = async_fs::read(&cache_file).await;
        if cached.is_ok() {
            trace!("loading {} from local cache", path);
            return Result::<Bytes>::Ok(Bytes::from(cached.unwrap()));
        }
        for cdn_prefix in cdn_prefixes {
            let data = fetch(format!("{}/{}", cdn_prefix, path)).await;
            if data.is_ok() {
                let data = data.unwrap();
                async_fs::write(&cache_file, &data).await?;
                trace!("wrote {} to local cache", path);
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
                    archive::parse_index(h, &(cdn_fetch("data", h, ".index").await?))
                }),
        )
        .await;
        let mut map = HashMap::<u128, (u128, usize, usize)>::new();
        for r in index_shards.into_iter() {
            map.extend(r?.map.drain());
        }
        return Result::<archive::Index>::Ok(archive::Index { map });
    };
    let encoding_and_root = async {
        let buildinfo = parse_build_config(&parse_config(&utf8(
            &(cdn_fetch("config", build_config, "").await?),
        )?))?;
        let encoding = encoding::parse(&blte::parse(
            &(cdn_fetch("data", buildinfo.encoding, "").await?),
        )?)?;
        let root = root::parse(&blte::parse(
            &cdn_fetch("data", encoding.c2e(buildinfo.root)?, "").await?,
        )?)?;
        Result::<(encoding::Encoding, root::Root)>::Ok((encoding, root))
    };
    let (archive_index, encoding_and_root) = futures::join!(archive_index, encoding_and_root);
    let (archive_index, (encoding, root)) = (archive_index?, encoding_and_root?);
    let (archive_index, encoding, root) = (&archive_index, &encoding, &root);
    let fetch_fdid = |fdid| async move {
        let (archive, size, offset) = archive_index
            .map
            .get(&encoding.c2e(root.f2c(fdid)?)?)
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
        blte::parse(&response.bytes().await.context("recv fail")?)
    };
    println!(
        "{:#?}",
        futures::future::join_all(
            wdc3::strings(&fetch_fdid(1267335).await?)?
                .into_keys()
                .map(|fdid| fdid as i32)
                .filter(|fdid| root.f2c(*fdid).is_ok())
                .map(fetch_fdid),
        )
        .await
        .into_iter()
        .collect::<Result<Vec<Vec<u8>>>>()?
        .into_iter()
        .map(|x| String::from_utf8(x).context("utf8 conversion"))
        .collect::<Result<Vec<String>>>()?
        .into_iter()
        .map(|x| x.lines().map(|y| y.to_string()).collect::<Vec<String>>())
        .collect::<Vec<Vec<String>>>()
    );
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

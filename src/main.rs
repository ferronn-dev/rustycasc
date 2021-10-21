mod archive;
mod blte;
mod encoding;
mod root;
mod util;
mod wdc3;

use anyhow::{bail, ensure, Context, Result};
use bytes::Bytes;
use log::trace;
use reqwest::Request;
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

async fn process(product: &str, product_suffix: &str) -> Result<()> {
    let patch_base = format!("http://us.patch.battle.net:1119/{}", product);
    let ref client = reqwest::Client::new();
    let fetch = |req: Request| async move {
        let url = req.url().to_string();
        trace!("starting fetch of {}", url);
        let response = client
            .execute(req)
            .await
            .context(format!("sending request to {}", url))?;
        ensure!(
            response.status().is_success(),
            format!("http error on {}", url)
        );
        trace!("receiving content on {}", url);
        let data = response
            .bytes()
            .await
            .context(format!("receiving content on {}", url))?;
        trace!("done retrieving {}", url);
        Ok(data)
    };
    let utf8 = std::str::from_utf8;
    let (versions, cdns) = futures::join!(
        fetch(client.get(format!("{}/versions", patch_base)).build()?),
        fetch(client.get(format!("{}/cdns", patch_base)).build()?)
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
    let ref fetchmap = tokio::sync::Mutex::new(HashMap::<String, tokio::sync::Mutex<()>>::new());
    let do_cdn_fetch = |tag: &'static str,
                        hash: u128,
                        suffix: Option<&'static str>,
                        range: Option<(String, usize, usize)>| async move {
        let h = format!("{:032x}", hash);
        let path = format!(
            "{}/{}/{}/{}{}",
            tag,
            &h[0..2],
            &h[2..4],
            h,
            suffix.unwrap_or("")
        );
        let cache_file = format!(
            "{}{}{}.{}",
            tag,
            suffix.unwrap_or(""),
            range.clone().map_or("".to_string(), |(s, _, _)| s),
            h
        );
        let _guard = async {
            let mut fm = fetchmap.lock().await;
            fm.entry(cache_file.clone())
                .or_insert(tokio::sync::Mutex::new(()))
                .lock()
                .await;
        }
        .await;
        trace!("cdn fetch {}", path);
        let cache_file = format!("cache/{}", cache_file);
        let cached = tokio::fs::read(&cache_file).await;
        if cached.is_ok() {
            trace!("loading local {}", cache_file);
            return Result::<Bytes>::Ok(Bytes::from(cached.unwrap()));
        }
        for cdn_prefix in cdn_prefixes {
            let mut req = client.get(format!("{}/{}", cdn_prefix, path));
            if let Some((_, start, end)) = range {
                req = req.header("Range", format!("bytes={}-{}", start, end));
            }
            let data = fetch(req.build()?).await;
            if data.is_ok() {
                let data = data.unwrap();
                tokio::fs::write(&cache_file, &data).await?;
                trace!("wrote local {}", cache_file);
                return Ok(data);
            }
        }
        bail!("fetch failed on all hosts: {}", path)
    };
    let cdn_fetch =
        |tag: &'static str, hash: u128| async move { do_cdn_fetch(tag, hash, None, None).await };
    let archive_index = async {
        let index_shards = futures::future::join_all(
            parse_config(&utf8(&(cdn_fetch("config", cdn_config).await?))?)
                .get("archives")
                .context("missing archives in cdninfo")?
                .split(" ")
                .map(|s| async move {
                    let h = parse_hash(s)?;
                    archive::parse_index(h, &(do_cdn_fetch("data", h, Some(".index"), None).await?))
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
            &(cdn_fetch("config", build_config).await?),
        )?))?;
        let encoding = encoding::parse(&blte::parse(
            &(cdn_fetch("data", buildinfo.encoding).await?),
        )?)?;
        let root = root::parse(&blte::parse(
            &cdn_fetch("data", encoding.c2e(buildinfo.root)?).await?,
        )?)?;
        Result::<(encoding::Encoding, root::Root)>::Ok((encoding, root))
    };
    let (archive_index, encoding_and_root) = futures::join!(archive_index, encoding_and_root);
    let (archive_index, (encoding, root)) = (archive_index?, encoding_and_root?);
    let (archive_index, encoding, root) = (&archive_index, &encoding, &root);
    let fetch_content = |ckey| async move {
        let (archive, size, offset) = archive_index
            .map
            .get(&encoding.c2e(ckey)?)
            .context("missing index key")?;
        let response = do_cdn_fetch(
            "data",
            *archive,
            None,
            Some((format!(".{:032x}", ckey), *offset, *offset + *size - 1)),
        )
        .await?;
        let bytes = blte::parse(&response)?;
        ensure!(
            util::md5hash(&bytes) == ckey,
            "checksum fail on {:032x}",
            ckey
        );
        Ok(bytes)
    };
    let fetch_fdid = |fdid| async move { fetch_content(root.f2c(fdid)?).await };
    let _fetch_name = |name| async move { fetch_content(root.n2c(name)?).await };
    tokio::fs::write(
        format!("{}.txt", product),
        wdc3::strings(&fetch_fdid(1267335).await?)?
            .into_values()
            .chain(["Interface\\FrameXML\\".to_string()])
            .filter_map(|s| {
                let dirname = s[..s.len() - 1].split("\\").last()?;
                let toc1 = format!("{}{}_{}.toc", s, dirname, product_suffix);
                match root.n2c(&toc1) {
                    Ok(_) => Some(toc1),
                    _ => {
                        let toc2 = format!("{}{}.toc", s, dirname);
                        match root.n2c(&toc2) {
                            Ok(_) => Some(toc2),
                            _ => None,
                        }
                    }
                }
            })
            .chain(["".to_string()])
            .collect::<Vec<String>>()
            .join("\n"),
    )
    .await?;
    Ok(())
}

#[derive(StructOpt)]
struct Cli {
    #[structopt(short = "v", long = "verbose", parse(from_occurrences))]
    verbose: usize,
    #[structopt(short = "p", long = "product")]
    products: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::from_args_safe()?;
    stderrlog::new()
        .module(module_path!())
        .verbosity(cli.verbose)
        .init()?;
    let all_products = velcro::hash_map! {
        "wow": "Mainline",
        "wowt": "Mainline",
        "wow_classic": "TBC",
        "wow_classic_era": "Vanilla",
        "wow_classic_era_ptr": "Vanilla",
        "wow_classic_ptr": "TBC",
    };
    let products: HashMap<String, String> = if cli.products.is_empty() {
        all_products
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    } else {
        cli.products
            .iter()
            .map(|s| (s.clone(), all_products[s.as_str()].to_string()))
            .collect()
    };
    futures::future::join_all(products.iter().map(|(k, v)| process(k, v)))
        .await
        .into_iter()
        .collect::<Result<Vec<()>>>()?;
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

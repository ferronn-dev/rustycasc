mod archive;
mod blte;
mod db2;
mod encoding;
mod ribbit;
mod root;
mod types;
mod util;

use crate::types::{ArchiveKey, ContentKey, EncodingKey, FileDataID};
use anyhow::{bail, ensure, Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::future::FutureExt;
use log::{trace, warn};
use std::collections::HashMap;
use std::str::from_utf8;

#[async_trait]
trait BytesFetcher {
    async fn fetch_bytes(&self, url: String, range: Option<(usize, usize)>) -> Result<Bytes>;
}

#[async_trait]
impl BytesFetcher for reqwest::Client {
    async fn fetch_bytes(&self, url: String, range: Option<(usize, usize)>) -> Result<Bytes> {
        let mut req = self.get(&url);
        if let Some((start, end)) = range {
            req = req.header("Range", format!("bytes={}-{}", start, end));
        }
        trace!("starting fetch of {}", url);
        let response = req
            .send()
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
    }
}

#[async_trait]
trait TextFetcher {
    async fn fetch_text(&self, url: String) -> Result<String>;
}

#[async_trait]
impl<T: BytesFetcher + Sync> TextFetcher for T {
    async fn fetch_text(&self, url: String) -> Result<String> {
        Ok(from_utf8(&self.fetch_bytes(url, None).await?)?.to_string())
    }
}

#[async_trait]
trait PatchDataFetcher {
    async fn fetch_version(&self, suffix: &str) -> Result<(u128, u128)>;
    async fn fetch_cdns(&self, suffix: &str) -> Result<Vec<String>>;
}

#[async_trait]
impl<T: TextFetcher + Sync> PatchDataFetcher for T {
    async fn fetch_version(&self, suffix: &str) -> Result<(u128, u128)> {
        let info = self
            .fetch_text(format!(
                "http://us.patch.battle.net:1119/{}/versions",
                suffix
            ))
            .await?;
        let version = parse_info(&info)
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
        Ok((build, cdn))
    }
    async fn fetch_cdns(&self, suffix: &str) -> Result<Vec<String>> {
        let info = self
            .fetch_text(format!("http://us.patch.battle.net:1119/{}/cdns", suffix))
            .await?;
        let cdn = parse_info(&info)
            .into_iter()
            .find(|m| m.get("Name") == Some(&"us"))
            .context("missing us cdn")?;
        let hosts = cdn.get("Hosts").context("missing us cdn hosts")?.split(' ');
        let path = cdn.get("Path").context("missing us cdn path")?;
        Ok(hosts.map(|s| format!("http://{}/{}", s, path)).collect())
    }
}

trait HasCdnPrefixes {
    fn cdn_prefixes(&self) -> &Vec<String>;
}

#[async_trait]
trait CdnBytesFetcher {
    async fn fetch_cdn_bytes(
        &self,
        tag: &str,
        hash: u128,
        suffix: Option<&str>,
        range: Option<(usize, usize)>,
    ) -> Result<Bytes>;
}

#[async_trait]
impl<T: BytesFetcher + HasCdnPrefixes + Sync> CdnBytesFetcher for T {
    async fn fetch_cdn_bytes(
        &self,
        tag: &str,
        hash: u128,
        suffix: Option<&str>,
        range: Option<(usize, usize)>,
    ) -> Result<Bytes> {
        let h = format!("{:032x}", hash);
        let path = format!(
            "{}/{}/{}/{}{}",
            tag,
            &h[0..2],
            &h[2..4],
            h,
            suffix.unwrap_or("")
        );
        trace!("cdn fetch {}", path);
        for _ in 1..10 {
            for cdn_prefix in self.cdn_prefixes() {
                let url = format!("{}/{}", cdn_prefix, path);
                match self.fetch_bytes(url, range).await {
                    Ok(data) => return Ok(data),
                    Err(msg) => warn!("fetch failed: {:#?}", msg),
                }
            }
        }
        bail!("fetch failed on all hosts: {}", path)
    }
}

fn parse_info(s: &str) -> Vec<HashMap<&str, &str>> {
    if s.is_empty() {
        // Empty string special case because lines() returns an empty iterator.
        return vec![];
    }
    let mut lines = s.lines().map(|x| x.split('|'));
    let tags = lines
        .next()
        .unwrap()
        .map(|x| x.split('!').next().unwrap())
        .collect::<Vec<&str>>();
    lines
        .skip(1)
        .map(|v| tags.iter().copied().zip(v).collect())
        .collect()
}

fn parse_config(s: &str) -> HashMap<&str, &str> {
    s.lines().filter_map(|x| x.split_once(" = ")).collect()
}

struct BuildConfig {
    root: ContentKey,
    encoding: EncodingKey,
}

fn parse_hash(s: &str) -> Result<u128> {
    u128::from_str_radix(s, 16).context("parse hash")
}

fn parse_build_config(config: &HashMap<&str, &str>) -> Result<BuildConfig> {
    Ok(BuildConfig {
        root: ContentKey(parse_hash(
            config.get("root").context("build config: root")?,
        )?),
        encoding: EncodingKey(parse_hash(
            config
                .get("encoding")
                .context("missing encoding field in buildinfo")?
                .split(' ')
                .nth(1)
                .context("missing data in encoding field in buildinfo")?,
        )?),
    })
}

fn normalize_path(base: &str, file: &str) -> String {
    let base = base.replace('/', "\\");
    let file = file.replace('/', "\\");
    let mut base: Vec<&str> = base.split('\\').collect();
    if base.is_empty() {
        return file;
    }
    base.pop();
    for part in file.split('\\') {
        if part == ".." {
            base.pop();
        } else {
            base.push(part);
        }
    }
    base.join("\\")
}

fn to_zip_archive_bytes(m: HashMap<String, Vec<u8>>) -> Result<Vec<u8>> {
    let mut zipbuf = Vec::<u8>::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zipbuf));
        for (name, data) in m {
            use std::io::Write;
            zip.start_file(
                name.replace('\\', "/"),
                zip::write::SimpleFileOptions::default(),
            )?;
            zip.write_all(&data)?;
        }
        zip.finish().context("zip archive failed to close")?;
    }
    Ok(zipbuf)
}

async fn process(product: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let ((build_config, cdn_config), cdn_prefixes) =
        futures::future::try_join(client.fetch_version(product), client.fetch_cdns(product))
            .await?;
    struct CdnClient {
        client: reqwest::Client,
        cdn_prefixes: Vec<String>,
        throttle: tokio::sync::Semaphore,
    }
    #[async_trait]
    impl BytesFetcher for CdnClient {
        async fn fetch_bytes(&self, url: String, range: Option<(usize, usize)>) -> Result<Bytes> {
            let _ = self.throttle.acquire().await?;
            self.client.fetch_bytes(url, range).await
        }
    }
    impl HasCdnPrefixes for CdnClient {
        fn cdn_prefixes(&self) -> &Vec<String> {
            &self.cdn_prefixes
        }
    }
    let cdn_client = &CdnClient {
        client,
        cdn_prefixes,
        throttle: tokio::sync::Semaphore::new(5),
    };
    let do_cdn_fetch = |tag: &'static str,
                        hash: u128,
                        suffix: Option<&'static str>,
                        range: Option<(usize, usize)>| async move {
        cdn_client.fetch_cdn_bytes(tag, hash, suffix, range).await
    };
    let cdn_fetch =
        |tag: &'static str, hash: u128| async move { do_cdn_fetch(tag, hash, None, None).await };
    let archive_index = async {
        let hashes = parse_config(from_utf8(&(cdn_fetch("config", cdn_config).await?))?)
            .get("archives")
            .context("missing archives in cdninfo")?
            .split(' ')
            .map(parse_hash)
            .collect::<Result<Vec<_>>>()?;
        let pb = &indicatif::ProgressBar::new(hashes.len() as u64);
        Result::<_>::Ok(archive::Index {
            map: futures::future::try_join_all(hashes.into_iter().map(|h| async move {
                archive::parse_index(
                    ArchiveKey(h),
                    &(do_cdn_fetch("data", h, Some(".index"), None)
                        .inspect(|_| pb.inc(1))
                        .await?),
                )
            }))
            .await?
            .into_iter()
            .flat_map(|archive::Index { map }| map)
            .collect(),
        })
    };
    let encoding_and_root = async {
        let buildinfo = parse_build_config(&parse_config(from_utf8(
            &(cdn_fetch("config", build_config).await?),
        )?))?;
        let encoding_key = buildinfo.encoding.0;
        let encoding = encoding::parse(&blte::parse(
            encoding_key,
            &(cdn_fetch("data", encoding_key).await?),
        )?)?;
        let root_key = encoding.c2e(buildinfo.root)?.0;
        let root = root::parse(&blte::parse(root_key, &cdn_fetch("data", root_key).await?)?)?;
        Result::<_>::Ok((encoding, root))
    };
    let (archive_index, (encoding, root)) =
        futures::future::try_join(archive_index, encoding_and_root).await?;
    let (archive_index, encoding, root) = (&archive_index, &encoding, &root);
    let fetch_content = |ckey| async move {
        let ekey = encoding.c2e(ckey)?;
        let (archive, size, offset) = archive_index.map.get(&ekey).context("missing index key")?;
        let response = do_cdn_fetch(
            "data",
            archive.0,
            None,
            Some((*offset, *offset + *size - 1)),
        )
        .await?;
        let bytes = blte::parse(ekey.0, &response)?;
        ensure!(util::md5hash(&bytes) == ckey.0, "checksum fail on {}", ckey);
        Ok(bytes)
    };
    let fetch_fdid = |fdid| async move { fetch_content(root.f2c(fdid)?).await };
    let fdids = db2::strings(&fetch_fdid(FileDataID(1375801)).await?)?
        .into_iter()
        .map(|(k, v)| (v.join("").to_lowercase(), FileDataID(k)))
        .collect::<HashMap<String, FileDataID>>();
    tokio::fs::write(
        format!("zips/{}.zip", product),
        to_zip_archive_bytes({
            let mut stack: Vec<String> = db2::strings(&fetch_fdid(FileDataID(1267335)).await?)?
                .into_values()
                .flatten()
                .chain(["Interface\\FrameXML\\".to_string()])
                .filter_map(|s| {
                    let dirname = s[..s.len() - 1].split('\\').last()?;
                    let toc1 = format!("{}{}_{}.toc", s, dirname, product);
                    let toc2 = format!("{}{}.toc", s, dirname);
                    root.n2c(&toc1)
                        .and(Ok(toc1))
                        .or_else(|_| root.n2c(&toc2).and(Ok(toc2)))
                        .ok()
                })
                .collect();
            let pb = &indicatif::ProgressBar::new(stack.len() as u64);
            let mut result = HashMap::<String, Vec<u8>>::new();
            while let Some(file) = stack.pop() {
                let content = match root.n2c(&file).ok().or_else(|| {
                    fdids
                        .get(&file.to_lowercase())
                        .and_then(|k| root.f2c(*k).ok())
                }) {
                    Some(ckey) => fetch_content(ckey).inspect(|_| pb.inc(1)).await?,
                    None => {
                        eprintln!("skipping file with no content key: {}", file);
                        pb.inc(1);
                        continue;
                    }
                };
                if file.ends_with(".toc") {
                    from_utf8(&content)?
                        .lines()
                        .map(|line| line.trim())
                        .filter(|line| !line.is_empty())
                        .filter(|line| !line.starts_with('#'))
                        .for_each(|line| {
                            pb.inc_length(1);
                            stack.push(normalize_path(&file, line))
                        });
                } else if file.ends_with(".xml") {
                    use xml::reader::{EventReader, XmlEvent::StartElement};
                    let xml = &content.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&content);
                    itertools::process_results(
                        EventReader::new(std::io::Cursor::new(xml)),
                        |iter| {
                            iter.filter_map(|e| {
                                if let StartElement {
                                    name, attributes, ..
                                } = e
                                {
                                    Some((name.local_name.to_lowercase(), attributes))
                                } else {
                                    None
                                }
                            })
                            .filter(|(name, _)| name == "script" || name == "include")
                            .flat_map(|(_, attrs)| attrs)
                            .filter(|attr| attr.name.local_name == "file")
                            .map(|attr| attr.value)
                            .for_each(|value| {
                                pb.inc_length(1);
                                stack.push(normalize_path(&file, &value))
                            })
                        },
                    )?;
                }
                result.insert(file, content);
            }
            Result::<_>::Ok(result)
        }?)?,
    )
    .await
    .context("zip writing")
}

fn ensuredir(dir: &str) -> Result<()> {
    match std::fs::metadata(dir).map_or(None, |m| Some(m.is_dir())) {
        Some(true) => Ok(()),
        Some(false) => bail!("{} is not a directory", dir),
        None => {
            trace!("creating directory {}", dir);
            std::fs::create_dir(dir).context(format!("error creating {}", dir))
        }
    }
}

#[derive(clap::Parser)]
#[clap(version, about)]
struct Cli {
    #[clap(subcommand)]
    command: CliCommands,
    #[clap(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(clap::Subcommand)]
enum CliCommands {
    #[clap(name = "framexml")]
    FrameXml(CliFrameXmlArgs),
    #[clap(name = "ribbit")]
    Ribbit(CliRibbitArgs),
}

#[derive(clap::Args)]
struct CliFrameXmlArgs {
    #[clap(value_parser)]
    product: String,
}

#[derive(clap::Args)]
struct CliRibbitArgs {
    #[clap(subcommand)]
    command: CliRibbitCommands,
}

#[derive(clap::Subcommand)]
enum CliRibbitCommands {
    #[clap(name = "summary")]
    Summary,
    #[clap(name = "versions")]
    Versions(CliRibbitVersionsArgs),
    #[clap(name = "cdns")]
    CDNs(CliRibbitCDNsArgs),
    #[clap(name = "check")]
    Check,
}

#[derive(clap::Args)]
struct CliRibbitVersionsArgs {
    #[clap(value_parser)]
    product: String,
}

#[derive(clap::Args)]
struct CliRibbitCDNsArgs {
    #[clap(value_parser)]
    product: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    use clap::Parser;
    let cli = Cli::parse();
    stderrlog::new()
        .module(module_path!())
        .timestamp(stderrlog::Timestamp::Millisecond)
        .verbosity(cli.verbose as usize)
        .init()?;
    match &cli.command {
        CliCommands::FrameXml(args) => {
            ensuredir("zips")?;
            process(&args.product).await
        }
        CliCommands::Ribbit(args) => match &args.command {
            CliRibbitCommands::Summary => {
                println!("{:#?}", ribbit::Ribbit::new()?.summary()?);
                Ok(())
            }
            CliRibbitCommands::Versions(args) => {
                println!("{:#?}", ribbit::Ribbit::new()?.versions(&args.product)?);
                Ok(())
            }
            CliRibbitCommands::CDNs(args) => {
                println!("{:#?}", ribbit::Ribbit::new()?.cdns(&args.product)?);
                Ok(())
            }
            CliRibbitCommands::Check => {
                let mut ribbit = ribbit::Ribbit::new()?;
                let summary = ribbit.summary()?;
                println!("summary seqn = {}", summary.seqn);
                for (k, v) in summary.entries {
                    println!("looking at {}", k);
                    if v.seqn.is_some() {
                        println!("{} versions seqn = {}", k, ribbit.versions(&k)?.seqn);
                    }
                    if v.cdn.is_some() {
                        println!("{} cdns seqn = {}", k, ribbit.cdns(&k)?.seqn);
                    }
                }
                Ok(())
            }
        },
    }
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

    #[test]
    fn test_normalize_path() {
        let tests = [
            ("empty string", "", "", ""),
            ("no dir", "a", "b", "b"),
            ("same dir", "dir\\a", "b", "dir\\b"),
        ];
        for (name, in_base, in_file, output) in tests {
            assert_eq!(super::normalize_path(in_base, in_file), output, "{}", name);
        }
    }
}

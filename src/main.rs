use futures::future::FutureExt;
use std::collections::HashMap;
use std::convert::TryInto;

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

fn parse_blte(data: &[u8]) -> Result<Vec<u8>> {
    use bytes::Buf;
    let mut p = data;
    if &p[0..4] != b"BLTE" {
        return Result::Err(Error::E("not BLTE format"));
    }
    p.advance(4);
    let header_size = p.get_u32();
    if header_size == 0 {
        return Result::Err(Error::E("0 header unimplemented"));
    }
    let _flags = p.get_u8();
    let chunk_count = (u32::from(p.get_u8()) << 16) | u32::from(p.get_u16());
    if header_size != chunk_count * 24 + 12 {
        return Result::Err(Error::E("header size mismatch"));
    }
    let mut chunkinfo = Vec::<(usize, usize, u128)>::new();
    for _ in 0..chunk_count {
        let compressed_size = p.get_u32().try_into()?;
        let uncompressed_size = p.get_u32().try_into()?;
        let checksum = p.get_u128();
        chunkinfo.push((compressed_size, uncompressed_size, checksum))
    }
    let mut result = bytes::BytesMut::with_capacity(chunkinfo.iter().map(|x| x.1).sum::<usize>());
    let inflate = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit;
    for (compressed_size, uncompressed_size, checksum) in chunkinfo {
        if checksum != u128::from_be_bytes(*md5::compute(&p[0..compressed_size])) {
            return Result::Err(Error::E("chunk checksum error"));
        }
        use bytes::BufMut;
        let encoding_mode = p.get_u8();
        let chunk_data = p.copy_to_bytes(compressed_size - 1);
        let data = match encoding_mode as char {
            'N' => {
                println!("{:?}", chunk_data);
                chunk_data
            }
            'Z' => bytes::Bytes::from(inflate(data, uncompressed_size)?),
            _ => return Result::Err(Error::E("invalid encoding")),
        };
        if data.len() != uncompressed_size {
            return Result::Err(Error::E("invalid uncompressed size"));
        }
        result.put(data)
    }
    Result::Ok(result.to_vec())
}

#[derive(Clone, Debug)]
enum Error {
    E(&'static str),
}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Error::E("io error")
    }
}
impl From<reqwest::Error> for Error {
    fn from(_: reqwest::Error) -> Self {
        Error::E("http error")
    }
}
impl From<std::str::Utf8Error> for Error {
    fn from(_: std::str::Utf8Error) -> Self {
        Error::E("utf8 error")
    }
}
impl From<&'static str> for Error {
    fn from(s: &'static str) -> Self {
        Error::E(s)
    }
}
impl From<std::num::TryFromIntError> for Error {
    fn from(_: std::num::TryFromIntError) -> Self {
        Error::E("numeric conversion error")
    }
}
impl From<miniz_oxide::inflate::TINFLStatus> for Error {
    fn from(_: miniz_oxide::inflate::TINFLStatus) -> Self {
        Error::E("decompression error")
    }
}
type Result<T> = std::result::Result<T, Error>;

#[tokio::main]
async fn main() -> Result<()> {
    let patch_base = "http://us.patch.battle.net:1119/wow_classic_era";
    let client = reqwest::Client::new();
    let fetch = |url| async { Result::Ok(client.get(url).send().await?.bytes().await?) };
    let utf8 = std::str::from_utf8;
    let version = async {
        let bytes = fetch(format!("{}/versions", patch_base)).await?;
        let info = utf8(&*bytes)?;
        let version = parse_info(info)
            .into_iter()
            .find(|m| m["Region"] == "us")
            .ok_or("missing us version")?;
        let build = version
            .get("BuildConfig")
            .ok_or("missing us build config version")?
            .to_string();
        let cdn = version
            .get("CDNConfig")
            .ok_or("missing us cdn config version")?
            .to_string();
        Result::Ok((build, cdn))
    }
    .shared();
    let cdn_fetch = async {
        let bytes = fetch(format!("{}/cdns", patch_base)).await?;
        let info = utf8(&*bytes)?;
        let cdn = parse_info(info)
            .into_iter()
            .find(|m| m["Name"] == "us")
            .ok_or("missing us cdn")?;
        let host = cdn
            .get("Hosts")
            .ok_or("missing us cdn hosts")?
            .split(" ")
            .next()
            .unwrap();
        let path = cdn.get("Path").ok_or("missing us cdn path")?;
        let prefix = format!("http://{}/{}", host, path);
        Result::Ok(move |tag: &'static str, hash: String| async move {
            let cache_file = format!("cache/{}.{}", tag, hash);
            let cached = std::fs::read(&cache_file);
            if cached.is_ok() {
                return Result::Ok(bytes::Bytes::from(cached.unwrap()));
            }
            let url = format!(
                "{}/{}/{}/{}/{}",
                prefix,
                tag,
                &hash[0..2],
                &hash[2..4],
                hash
            );
            let data = fetch(url).await?;
            std::fs::write(&cache_file, &data)?;
            //assert_eq!(hash, format!("{:x}", md5::compute(&data)), "{}", data.len());
            Result::Ok(data)
        })
    }
    .shared();
    let buildinfo = async {
        let (version, cdn_fetch) = futures::join!(version.clone(), cdn_fetch.clone());
        Result::Ok(
            parse_config(&utf8(&*cdn_fetch?("config", version?.0).await?)?)
                .get("encoding")
                .ok_or("missing encoding in buildinfo")?
                .split(" ")
                .map(|x| x.to_string())
                .collect::<Vec<String>>(),
        )
    };
    let cdninfo = async {
        let (version, cdn_fetch) = futures::join!(version.clone(), cdn_fetch.clone());
        let _archives = parse_config(&utf8(&*cdn_fetch?("config", version?.1).await?)?)
            .get("archives")
            .ok_or("missing archives in cdninfo")?
            .split(" ")
            .map(|x| x.to_string())
            .collect::<Vec<String>>();
        Result::Ok(())
    };
    let encoding = async {
        let (buildinfo, cdn_fetch) = futures::join!(buildinfo, cdn_fetch.clone());
        let data = cdn_fetch?("data", buildinfo?.remove(1)).await?;
        parse_blte(&data)
    };
    let _ = cdninfo.await?;
    println!("{}", utf8(&encoding.await?)?);
    Result::Ok(())
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

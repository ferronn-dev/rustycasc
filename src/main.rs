use futures::future::FutureExt;
use std::collections::HashMap;

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

#[derive(Clone, Debug)]
enum Error {
    E(&'static str),
}
impl From<reqwest::Error> for Error {
    fn from(_: reqwest::Error) -> Self {
        Error::E("http error")
    }
}
impl From<&'static str> for Error {
    fn from(s: &'static str) -> Self {
        Error::E(s)
    }
}
type Result<T> = std::result::Result<T, Error>;

#[tokio::main]
async fn main() -> Result<()> {
    let patch_base = "http://us.patch.battle.net:1119/wow_classic_era";
    let client = reqwest::Client::new();
    let fetch = |url| async { client.get(url).send().await?.text().await };
    let version = async {
        let info = fetch(format!("{}/versions", patch_base)).await?;
        let version = parse_info(&info)
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
        let info = fetch(format!("{}/cdns", patch_base)).await?;
        let cdn = parse_info(&info)
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
            let url = format!(
                "{}/{}/{}/{}/{}",
                prefix,
                tag,
                hash[0..2].to_string(),
                hash[2..4].to_string(),
                hash
            );
            let data = fetch(url).await?;
            assert_eq!(hash, format!("{:x}", md5::compute(&data)));
            Result::Ok(data)
        })
    }
    .shared();
    let buildinfo = async {
        let (version, cdn_fetch) = futures::join!(version.clone(), cdn_fetch.clone());
        Result::Ok(
            parse_config(&cdn_fetch?("config", version?.0).await?)
                .get("encoding")
                .ok_or("missing encoding in buildinfo")?
                .split(" ")
                .map(|x| x.to_string())
                .collect::<Vec<String>>(),
        )
    };
    let cdninfo = async {
        let (version, cdn_fetch) = futures::join!(version.clone(), cdn_fetch.clone());
        Result::Ok(
            parse_config(&cdn_fetch?("config", version?.1).await?)
                .get("archives")
                .ok_or("missing archives in cdninfo")?
                .split(" ")
                .map(|x| x.to_string())
                .collect::<Vec<String>>(),
        )
    };
    let (_, _) = futures::join!(
        buildinfo.inspect(|x| if x.is_ok() {
            println!("{:?}", x.as_ref().unwrap())
        }),
        cdninfo.inspect(|x| if x.is_ok() {
            println!("{:?}", x.as_ref().unwrap())
        }),
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

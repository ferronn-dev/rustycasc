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

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> Result<()> {
    let base = "http://us.patch.battle.net:1119/wow_classic_era";
    let ref client = reqwest::Client::new();
    let fetch = |path| async move {
        client
            .get(format!("{}/{}", base, path))
            .send()
            .await?
            .text()
            .await
    };
    let get_version = || async move {
        let info = fetch("versions").await?;
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
    };
    let get_cdn_fetcher = || async move {
        let info = fetch("cdns").await?;
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
        Result::Ok(move |tag: String, hash: String| async move {
            let url = format!(
                "{}/{}/{}/{}/{}",
                prefix,
                tag,
                hash[0..2].to_string(),
                hash[2..4].to_string(),
                hash
            );
            let data = client.get(url).send().await?.text().await?;
            assert_eq!(hash, format!("{:x}", md5::compute(&data)));
            Result::Ok(data)
        })
    };
    let get_buildinfo = || async move {
        let (version, cdn_fetcher) = futures::join!(get_version(), get_cdn_fetcher());
        Result::Ok(
            parse_config(&cdn_fetcher?("config".to_string(), version?.0).await?)
                .get("encoding")
                .ok_or("missing encoding in buildinfo")?
                .to_string(),
        )
    };
    println!("{:?}", get_buildinfo().await?);
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

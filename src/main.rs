use std::collections::HashMap;

fn parse_info(s: &str) -> Vec<HashMap<&str, &str>> {
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
        Result::<String>::Ok(
            parse_info(&fetch("versions").await?)
                .into_iter()
                .find(|m| m["Region"] == "us")
                .ok_or("missing us version")?
                .remove("BuildConfig")
                .ok_or("missing us build config version")?
                .to_string(),
        )
    };
    let get_cdn = || async move {
        let info = fetch("cdns").await?;
        let mut cdn = parse_info(&info)
            .into_iter()
            .find(|m| m["Name"] == "us")
            .ok_or("missing us cdn")?;
        let host = cdn
            .remove("Hosts")
            .ok_or("missing us cdn hosts")?
            .split(" ")
            .next()
            .unwrap();
        let path = cdn.remove("Path").ok_or("missing us cdn path")?;
        Result::<(String, String)>::Ok((host.to_string(), path.to_string()))
    };
    let (version, cdn) = futures::join!(get_version(), get_cdn());
    let (host, path) = cdn?;
    println!("{} {} {}", version?, host, path);
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    #[should_panic]
    fn test_parse_info() {
        super::parse_info("");
    }
}

use std::collections::HashMap;

fn parse_info(s: String) -> Vec<HashMap<String, String>> {
    let mut lines = s.lines().map(|x| x.split("|"));
    let tags = lines
        .next()
        .unwrap()
        .map(|x| x.split("!").next().unwrap())
        .collect::<Vec<&str>>();
    lines
        .skip(1)
        .map(|v| {
            tags.iter()
                .zip(v)
                .map(|(t, x)| (t.to_string(), x.to_string()))
                .collect()
        })
        .collect()
}

type Error = Box<dyn std::error::Error>;

#[tokio::main]
async fn main() -> Result<(), Error> {
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
        Ok::<String, Error>(
            parse_info(fetch("versions").await?)
                .into_iter()
                .find(|m| m["Region"] == "us")
                .ok_or("missing us version")?
                .remove("BuildConfig")
                .ok_or("missing us build config version")?,
        )
    };
    let get_cdn = || async move {
        Ok::<String, Error>(
            parse_info(fetch("cdns").await?)
                .into_iter()
                .find(|m| m["Name"] == "us")
                .ok_or("missing us cdn")?
                .remove("Hosts")
                .ok_or("missing us cdn hosts")?
                .split(" ")
                .next()
                .unwrap()
                .to_string(),
        )
    };
    let (version, cdn) = futures::join!(get_version(), get_cdn());
    println!("{} {}", version?, cdn?);
    Ok(())
}

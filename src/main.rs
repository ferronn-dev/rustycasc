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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    let (versions, cdns) = futures::join!(fetch("versions"), fetch("cdns"));
    println!("{:#?}", parse_info(versions?));
    println!("{:#?}", parse_info(cdns?));
    Ok(())
}

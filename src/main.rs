use std::collections::HashMap;

fn parse_info(s: String) -> Vec<HashMap<String, String>> {
    let ts = |x: &str| x.to_string();
    let parts = |x: String| x.split("|").map(ts).collect::<Vec<String>>();
    let lines = s.lines().map(ts).collect::<Vec<String>>();
    let tags = parts(lines[0].clone())
        .into_iter()
        .map(|x| x.split("!").next().unwrap().to_string())
        .collect::<Vec<String>>();
    lines
        .into_iter()
        .skip(2)
        .map(parts)
        .map(|v| {
            v.into_iter()
                .enumerate()
                .fold(HashMap::new(), |mut acc, (i, x)| {
                    acc.entry(tags[i].clone()).or_insert(x.clone());
                    acc
                })
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

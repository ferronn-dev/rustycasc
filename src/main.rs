async fn fetch_text(client: &reqwest::Client, url: String) -> reqwest::Result<String> {
    client.get(url).send().await?.text().await
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = "http://us.patch.battle.net:1119/wow_classic_era";
    let client = reqwest::Client::new();
    let (versions, cdns) = futures::join!(
        fetch_text(&client, format!("{}/versions", base)),
        fetch_text(&client, format!("{}/cdns", base))
    );
    println!("{}", versions?);
    println!("{}", cdns?);
    Ok(())
}

async fn fetch_text(client: &reqwest::Client, url: String) -> reqwest::Result<String> {
    return client.get(url).send().await.expect("nope").text().await;
}

#[tokio::main]
async fn main() {
    let base = "http://us.patch.battle.net:1119/wow_classic_era";
    let client = reqwest::Client::new();
    let (versions, cdns) = futures::join!(
        fetch_text(&client, format!("{}/versions", base)),
        fetch_text(&client, format!("{}/cdns", base))
    );
    println!("{}", versions.expect("moo"));
    println!("{}", cdns.expect("moo"));
}

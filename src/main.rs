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
    println!("{}", versions?);
    println!("{}", cdns?);
    Ok(())
}

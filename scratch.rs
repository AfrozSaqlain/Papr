use reqwest::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::builder().build()?;
    let res = client.get("https://export.arxiv.org/api/query").send().await?;
    println!("Status: {}", res.status());
    Ok(())
}

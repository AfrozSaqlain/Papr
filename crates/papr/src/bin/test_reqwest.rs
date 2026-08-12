#[tokio::main]
async fn main() {
    let client = reqwest::Client::builder().user_agent("papr/0.1.1").build().unwrap();
    println!("Testing ArXiv...");
    match client.get("https://export.arxiv.org/api/query").send().await {
        Ok(_) => println!("ArXiv Success"),
        Err(e) => println!("ArXiv Error: {:?}", e),
    }
}

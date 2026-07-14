fn main() {
    let urls = vec![
        "http://arxiv.org/abs/2104.12345v1",
        "https://arxiv.org/abs/2104.12345",
        "2104.12345",
        "arxiv:2104.12345",
        "https://doi.org/10.1234/5678",
        "doi:10.1234/5678",
        "10.1234/5678",
    ];
    for u in urls {
        println!("{} -> arXiv: {:?}, DOI: {:?}", u, extract_arxiv(u), extract_doi(u));
    }
}
fn extract_arxiv(s: &str) -> Option<String> {
    let mut s = s.trim();
    if s.starts_with("http://arxiv.org/abs/") || s.starts_with("https://arxiv.org/abs/") {
        s = s.split("/abs/").last().unwrap_or(s);
    } else if s.starts_with("arxiv:") {
        s = &s[6..];
    }
    let s = s.split('v').next().unwrap_or(s);
    if s.chars().all(|c| c.is_ascii_digit() || c == '.') {
        Some(s.to_string())
    } else {
        None
    }
}
fn extract_doi(s: &str) -> Option<String> {
    let s = s.trim();
    let s = if let Some(idx) = s.find("doi.org/") {
        &s[idx + 8..]
    } else if let Some(idx) = s.find("doi:") {
        &s[idx + 4..]
    } else {
        s
    };
    if s.starts_with("10.") {
        Some(s.to_string())
    } else {
        None
    }
}

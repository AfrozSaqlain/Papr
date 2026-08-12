use std::path::Path;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: String,
}

/// Generates an AI summary for a given PDF document.
/// 
/// This extracts text from the PDF at `pdf_path`, optionally chunking it if it's too long,
/// and uses the provided AI `model` and `api_key` to request a summarization.
pub async fn generate_summary(
    api_key: &str,
    model: &str,
    pdf_path: &Path,
) -> Result<String> {
    // 1. Extract text
    let bytes = std::fs::read(pdf_path).context("Failed to read PDF file")?;
    let text = pdf_extract::extract_text_from_mem(&bytes).context("Failed to extract text from PDF")?;

    // 2. Chunking
    // We chunk by roughly 25,000 characters to fit within a conservative context window, 
    // though many models support much larger contexts now.
    let chunk_size = 25_000;
    let chunks = text.as_bytes().chunks(chunk_size);
    let num_chunks = chunks.len();

    let client = Client::new();
    let mut combined_summary = String::new();

    for (i, chunk_bytes) in chunks.enumerate() {
        let chunk_text = String::from_utf8_lossy(chunk_bytes);
        
        let prompt = if num_chunks == 1 {
            format!(
                "You are an expert scientist. Read the following paper and generate a structured scientific summary covering EXACTLY these sections:\n\
                - Research question\n\
                - Motivation\n\
                - Methodology\n\
                - Important equations/assumptions\n\
                - Key results\n\
                - Physical/scientific interpretation\n\
                - Conclusions\n\
                - Limitations\n\
                - Overall summary\n\n\
                Paper text:\n{chunk_text}"
            )
        } else if i == 0 {
            format!(
                "You are an expert scientist. Read the first part of the following paper and start generating a structured scientific summary covering these sections (extract what you can from this part):\n\
                - Research question\n\
                - Motivation\n\
                - Methodology\n\
                - Important equations/assumptions\n\
                - Key results\n\
                - Physical/scientific interpretation\n\
                - Conclusions\n\
                - Limitations\n\
                - Overall summary\n\n\
                Part 1 text:\n{chunk_text}"
            )
        } else if i == num_chunks - 1 {
            format!(
                "You are an expert scientist. Here is the summary you've built so far from previous parts of a paper:\n{combined_summary}\n\n\
                Here is the final part of the paper. Please synthesize everything into a final, complete, well-formatted structured scientific summary covering EXACTLY these sections:\n\
                - Research question\n\
                - Motivation\n\
                - Methodology\n\
                - Important equations/assumptions\n\
                - Key results\n\
                - Physical/scientific interpretation\n\
                - Conclusions\n\
                - Limitations\n\
                - Overall summary\n\n\
                Final part text:\n{chunk_text}"
            )
        } else {
            format!(
                "You are an expert scientist. Here is the summary you've built so far from previous parts of a paper:\n{combined_summary}\n\n\
                Here is the next part of the paper. Please update and expand the summary with any new information found in this part, keeping the same structure:\n\
                - Research question\n\
                - Motivation\n\
                - Methodology\n\
                - Important equations/assumptions\n\
                - Key results\n\
                - Physical/scientific interpretation\n\
                - Conclusions\n\
                - Limitations\n\
                - Overall summary\n\n\
                Next part text:\n{chunk_text}"
            )
        };

        let request_body = ChatRequest {
            model: model.to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: prompt,
            }],
            max_tokens: Some(8000), // Request up to 8000 tokens for the summary
        };

        let response = client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to OpenRouter")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            
            if status.as_u16() == 402 {
                anyhow::bail!("OpenRouter credits exhausted or provider limit reached (402). Try a free model or add credits.");
            } else if status.as_u16() == 429 {
                anyhow::bail!("OpenRouter rate limit exceeded (429). Please try again later.");
            } else {
                #[derive(Deserialize)]
                struct OpenRouterErrorResponse {
                    error: OpenRouterErrorDetail,
                }
                #[derive(Deserialize)]
                struct OpenRouterErrorDetail {
                    message: String,
                }
                
                if let Ok(parsed) = serde_json::from_str::<OpenRouterErrorResponse>(&error_text) {
                    anyhow::bail!("OpenRouter error: {}", parsed.error.message);
                }
                
                anyhow::bail!("OpenRouter API error ({}): {}", status, error_text);
            }
        }

        let mut chat_response: ChatResponse = response
            .json()
            .await
            .context("Failed to parse OpenRouter response")?;

        if let Some(choice) = chat_response.choices.pop() {
            combined_summary = choice.message.content;
        } else {
            anyhow::bail!("No choices returned from OpenRouter");
        }
    }

    Ok(combined_summary)
}

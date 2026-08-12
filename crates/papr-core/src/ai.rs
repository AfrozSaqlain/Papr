use std::path::Path;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

/// Represents an available AI model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiModel {
    /// The unique identifier for the model (e.g. "google/gemini-pro:free")
    pub id: String,
    /// The human-readable name of the model
    pub name: String,
}

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

/// Fetches the list of free models available on OpenRouter.
pub async fn fetch_free_models() -> Result<Vec<AiModel>> {
    let client = Client::builder()
        .user_agent(concat!("papr/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
        
    let response: serde_json::Value = client
        .get("https://openrouter.ai/api/v1/models")
        .send()
        .await
        .context("Failed to connect to OpenRouter models API")?
        .error_for_status()
        .context("OpenRouter models API returned an error")?
        .json()
        .await
        .context("Failed to parse OpenRouter models response")?;
        
    let mut free_models = Vec::new();
    if let Some(data) = response.get("data").and_then(|d| d.as_array()) {
        for model in data {
            let id = model.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let name = model.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            
            if let Some(pricing) = model.get("pricing") {
                let prompt = pricing.get("prompt").and_then(|v| v.as_str()).unwrap_or("1");
                let completion = pricing.get("completion").and_then(|v| v.as_str()).unwrap_or("1");
                
                if let (Ok(p), Ok(c)) = (prompt.parse::<f64>(), completion.parse::<f64>()) {
                    if p == 0.0 && c == 0.0 && !id.is_empty() {
                        free_models.push(AiModel { id, name });
                    }
                }
            }
        }
    }
    
    free_models.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(free_models)
}

fn split_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for paragraph in text.split("\n\n") {
        if current.len() + paragraph.len() + 2 > max_chars && !current.is_empty() {
            chunks.push(current);
            current = String::new();
        }

        current.push_str(paragraph);
        current.push_str("\n\n");
    }

    if !current.trim().is_empty() {
        chunks.push(current);
    }

    chunks
}

/// Generates an AI summary for a given PDF document.
/// 
/// This extracts text from the PDF at `pdf_path`, optionally chunking it if it's too long,
/// and uses the provided AI `model` and `api_key` to request a summarization.
pub async fn generate_summary(
    api_key: &str,
    model: &str,
    pdf_path: &Path,
    progress_tx: UnboundedSender<String>,
    output_dir: &Path,
) -> Result<String> {
    progress_tx.send(format!("Extracting text from {}...", pdf_path.display())).ok();

    let bytes = std::fs::read(pdf_path).context("Failed to read PDF file")?;
    let text = pdf_extract::extract_text_from_mem(&bytes).context("Failed to extract text from PDF")?;

    if text.trim().is_empty() {
        anyhow::bail!("No text could be extracted from the PDF");
    }

    progress_tx.send(format!("Extracted {} characters.", text.len())).ok();

    let chunks = split_text(&text, 20_000);

    progress_tx.send(format!("Split paper into {} chunks.", chunks.len())).ok();

    let client = Client::new();
    let mut summaries = Vec::with_capacity(chunks.len());

    for (index, chunk) in chunks.iter().enumerate() {
        progress_tx.send(format!("Summarizing chunk {}/{}...", index + 1, chunks.len())).ok();

        let prompt = format!(
            r#"You are summarizing a scientific research paper.

Summarize the following section accurately and concisely.

Focus on:
- scientific motivation
- research question
- methodology
- important equations and physical assumptions
- datasets or simulations
- key quantitative results
- important conclusions
- limitations

Do not invent information.
Preserve technical terminology.
If an equation or numerical result is important, retain it.

Paper section:

{}"#,
            chunk
        );

        let request_body = ChatRequest {
            model: model.to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: prompt,
            }],
            max_tokens: None,
        };

        let summary = chat(&client, api_key, &request_body).await?;
        summaries.push(summary);
    }

    progress_tx.send("Generating final synthesis...".to_string()).ok();

    let combined = summaries
        .iter()
        .enumerate()
        .map(|(i, summary)| format!("SECTION {}:\n{}", i + 1, summary))
        .collect::<Vec<_>>()
        .join("\n\n");

    let final_prompt = format!(
        r#"You are an expert scientific research assistant.

Using the section summaries below, produce a rigorous summary of the research paper.

Structure the response as:

# Paper Summary

## Research Question
What problem does the paper address?

## Motivation
Why is the problem important?

## Methodology
Describe the methods, models, simulations, datasets, or observations used.

## Key Results
Give the most important findings, including quantitative results when available.

## Physical Interpretation
Explain what the results mean scientifically.

## Important Equations
Include the most important equations and briefly explain their role.

## Conclusions
Summarize the main conclusions.

## Limitations
Mention limitations explicitly discussed or clearly evident from the paper.

## Overall Summary
Give a concise paragraph capturing the entire paper.

Do not invent information or attribute results that are not present in the supplied summaries.

Section summaries:

{}"#,
        combined
    );

    let request_body = ChatRequest {
        model: model.to_string(),
        messages: vec![Message {
            role: "user".to_string(),
            content: final_prompt,
        }],
        max_tokens: None,
    };

    let final_summary = chat(&client, api_key, &request_body).await?;
    
    progress_tx.send("Saving summary...".to_string()).ok();
    
    std::fs::create_dir_all(output_dir)?;
    let summary_md = output_dir.join("summary.md");
    std::fs::write(&summary_md, final_summary.clone())?;

    progress_tx.send("Finalizing summary...".to_string()).ok();

    Ok(final_summary)
}

async fn chat(client: &Client, api_key: &str, request_body: &ChatRequest) -> Result<String> {
    let response = client
        .post("https://openrouter.ai/api/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(request_body)
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
        Ok(choice.message.content)
    } else {
        anyhow::bail!("No choices returned from OpenRouter")
    }
}

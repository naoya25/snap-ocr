use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const BASE_URL: &str = "https://api.japan-ai.co.jp";
const OCR_PROMPT: &str =
    "画像内のテキストを一字一句そのまま出力してください。説明や前置きは不要です。";
const MAX_IMAGE_BYTES: u64 = 11 * 1024 * 1024;

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: [Message; 1],
    temperature: u8,
}

#[derive(Debug, Serialize)]
struct Message {
    role: &'static str,
    content: [Content; 2],
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum Content {
    #[serde(rename = "text")]
    Text { text: &'static str },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Serialize)]
struct ImageUrl {
    url: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: String,
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .context("failed to build HTTP client")
}

/// GET /v1/models?userId=... — returns the list of available model ids.
/// Handles both the OpenAI-compatible response shape (`data[].id`) and the
/// JAPAN AI-native rich shape (`data[].name`).
pub fn fetch_models(api_key: &str, user_id: &str) -> Result<Vec<String>> {
    let client = client()?;
    let url = format!("{BASE_URL}/v1/models");

    let resp = client
        .get(&url)
        .query(&[("userId", user_id)])
        .bearer_auth(api_key)
        .send()
        .context("request to /v1/models failed")?;

    let status = resp.status();
    let body = resp
        .text()
        .context("failed to read /v1/models response body")?;
    if !status.is_success() {
        bail!("/v1/models returned HTTP {status}: {body}");
    }

    let json: Value = serde_json::from_str(&body).context("failed to parse /v1/models JSON")?;
    let data = json
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("/v1/models response missing \"data\" array"))?;

    let mut models: Vec<String> = data
        .iter()
        .filter_map(|entry| {
            entry
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| entry.get("name").and_then(Value::as_str))
                .map(str::to_string)
        })
        .collect();
    models.dedup();

    if models.is_empty() {
        bail!("/v1/models returned no usable model entries");
    }

    Ok(models)
}

/// Sends the screenshot to the OpenAI-compatible LLM Gateway and returns its OCR text.
pub fn ocr_image(api_key: &str, user_id: &str, model: &str, image_path: &Path) -> Result<String> {
    let metadata = std::fs::metadata(image_path)
        .with_context(|| format!("failed to inspect {}", image_path.display()))?;
    if metadata.len() > MAX_IMAGE_BYTES {
        bail!(
            "image is too large for the LLM Gateway: {} bytes (maximum {MAX_IMAGE_BYTES} bytes)",
            metadata.len()
        );
    }

    let image = std::fs::read(image_path)
        .with_context(|| format!("failed to read {}", image_path.display()))?;
    let request = chat_request(model, image);

    let client = client()?;
    let url = format!("{BASE_URL}/v1/chat/completions");
    let resp = client
        .post(&url)
        .query(&[("userId", user_id)])
        .bearer_auth(api_key)
        .json(&request)
        .send()
        .context("request to /v1/chat/completions failed")?;

    let status = resp.status();
    let body = resp
        .text()
        .context("failed to read /v1/chat/completions response body")?;
    if !status.is_success() {
        bail!("/v1/chat/completions returned HTTP {status}: {body}");
    }

    parse_chat_response(&body)
}

fn chat_request(model: &str, image: Vec<u8>) -> ChatRequest<'_> {
    ChatRequest {
        model,
        messages: [Message {
            role: "user",
            content: [
                Content::Text { text: OCR_PROMPT },
                Content::ImageUrl {
                    image_url: ImageUrl {
                        url: format!("data:image/png;base64,{}", BASE64_STANDARD.encode(image)),
                    },
                },
            ],
        }],
        temperature: 0,
    }
}

fn parse_chat_response(body: &str) -> Result<String> {
    let parsed: ChatResponse =
        serde_json::from_str(body).context("failed to parse /v1/chat/completions JSON")?;
    let content = parsed
        .choices
        .into_iter()
        .next()
        .map(|choice| choice.message.content)
        .ok_or_else(|| anyhow!("/v1/chat/completions response missing choice content"))?;

    if content.trim().is_empty() {
        bail!("/v1/chat/completions returned empty content");
    }

    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_request_should_encode_png_as_openai_image_url() {
        let json = serde_json::to_value(chat_request("vision-model", vec![0, 1, 2]))
            .expect("request should serialize");

        assert_eq!(
            json["messages"][0]["content"][1]["image_url"]["url"],
            "data:image/png;base64,AAEC"
        );
    }

    #[test]
    fn parse_chat_response_should_return_first_choice_content() {
        let body = r#"{"choices":[{"message":{"content":"読み取り結果"}}]}"#;

        assert_eq!(
            parse_chat_response(body).expect("response should parse"),
            "読み取り結果"
        );
    }

    #[test]
    fn parse_chat_response_should_reject_empty_choices() {
        let error =
            parse_chat_response(r#"{"choices":[]}"#).expect_err("empty choices should be rejected");

        assert_eq!(
            error.to_string(),
            "/v1/chat/completions response missing choice content"
        );
    }

    #[test]
    fn parse_chat_response_should_reject_blank_content() {
        let body = r#"{"choices":[{"message":{"content":"  "}}]}"#;
        let error = parse_chat_response(body).expect_err("blank content should be rejected");

        assert_eq!(
            error.to_string(),
            "/v1/chat/completions returned empty content"
        );
    }
}

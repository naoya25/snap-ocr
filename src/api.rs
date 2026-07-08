use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

const BASE_URL: &str = "https://api.japan-ai.co.jp";
const OCR_PROMPT: &str = "画像内のテキストを一字一句そのまま出力してください。説明や前置きは不要です。";

#[derive(Debug, Deserialize)]
struct ChatResponse {
    status: Option<String>,
    #[serde(rename = "chatMessage")]
    chat_message: Option<String>,
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
    let body = resp.text().context("failed to read /v1/models response body")?;
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

/// POST /chat/v2?userId=... (multipart) with the screenshot attached.
/// Returns the OCR'd text (`chatMessage`).
pub fn ocr_image(api_key: &str, user_id: &str, model: &str, image_path: &Path) -> Result<String> {
    let client = client()?;
    let url = format!("{BASE_URL}/chat/v2");

    // userId は query(認証ガード用)と form フィールド(コントローラのユーザー解決用)の両方に必要
    let form = reqwest::blocking::multipart::Form::new()
        .text("prompt", OCR_PROMPT)
        .text("model", model.to_string())
        .text("userId", user_id.to_string())
        .file("files", image_path)
        .with_context(|| format!("failed to attach {}", image_path.display()))?;

    let resp = client
        .post(&url)
        .query(&[("userId", user_id)])
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .context("request to /chat/v2 failed")?;

    let status = resp.status();
    let body = resp.text().context("failed to read /chat/v2 response body")?;
    if !status.is_success() {
        bail!("/chat/v2 returned HTTP {status}: {body}");
    }

    let parsed: ChatResponse =
        serde_json::from_str(&body).context("failed to parse /chat/v2 JSON")?;

    match parsed.status.as_deref() {
        Some("succeeded") => {}
        Some(other) => bail!("/chat/v2 returned status=\"{other}\""),
        None => bail!("/chat/v2 response missing \"status\" field"),
    }

    parsed
        .chat_message
        .ok_or_else(|| anyhow!("/chat/v2 response missing \"chatMessage\" field"))
}

//! SLM (Small Language Model) client for Text-to-SQL inference.
//!
//! Sends natural language prompts to a configurable model endpoint (e.g. Baseten)
//! along with schema context, and parses the generated SQL from the response.
//!
//! The model endpoint is fully configurable via `SLM_URL` and `SLM_API_KEY`
//! environment variables, allowing testing with different models.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::Config;

/// Request body sent to the SLM endpoint.
#[derive(Debug, Serialize)]
pub struct SlmRequest {
    pub prompt: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub stop: Vec<String>,
}

/// Response from the SLM endpoint.
/// Supports both OpenAI-compatible and simple text responses.
#[derive(Debug, Deserialize)]
pub struct SlmResponse {
    /// OpenAI-compatible: choices[0].text or choices[0].message.content
    pub choices: Option<Vec<SlmChoice>>,
    /// Simple text response (some endpoints return this directly)
    pub output: Option<String>,
    /// Baseten-style: model_output
    pub model_output: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct SlmChoice {
    pub text: Option<String>,
    pub message: Option<SlmMessage>,
}

#[derive(Debug, Deserialize)]
pub struct SlmMessage {
    pub content: Option<String>,
}

/// The result of SLM inference — either a SQL query or an error explanation.
#[derive(Debug)]
pub enum SlmResult {
    /// Successfully generated SQL query.
    Sql {
        query: String,
        /// Which data source to execute against: "delta" or "turso"
        target: QueryTarget,
    },
    /// The model couldn't generate SQL — returns an explanation.
    Explanation(String),
}

/// Where to execute the generated SQL.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryTarget {
    /// Execute against Delta Lake via DataFusion (raw events).
    Delta,
    /// Execute against plugin-runtime Turso (structured plugin data).
    Turso,
}

/// Build the system prompt for the SLM.
fn build_system_prompt(schema_context: &str) -> String {
    format!(
        r#"You are a SQL query generator for a business analytics platform.

Given a natural language question, generate a SQL query to answer it.

RULES:
1. Output ONLY the SQL query, nothing else. No markdown, no explanation, no backticks.
2. If the question is about raw tracking events (clicks, impressions, postbacks, event history),
   query from the `scoped` CTE (Delta Lake). Start your query with "SELECT ... FROM scoped ...".
3. If the question is about structured business data (Stripe charges, Shopify orders, etc.),
   query from the plugin tables using their full_name. Prefix your query with "-- turso" on the first line.
4. For Delta Lake queries, use json_get_str(params, '$.key') to extract param values.
5. For Delta Lake queries, timestamps are in Unix milliseconds. Use to_timestamp(timestamp_ms / 1000) for date functions.
6. Always include reasonable LIMIT (default 100) unless aggregating.
7. If you cannot generate a valid SQL query, respond with "EXPLAIN: " followed by why.

AVAILABLE SCHEMAS:
{}"#,
        schema_context
    )
}

/// Call the SLM endpoint to generate SQL from a natural language prompt.
pub async fn generate_sql(
    config: &Config,
    schema_context: &str,
    user_prompt: &str,
) -> Result<SlmResult, String> {
    let slm_url = match &config.slm_url {
        Some(url) => url.clone(),
        None => {
            // No SLM configured — return a mock response for testing
            return Ok(mock_generate_sql(user_prompt));
        }
    };

    let system_prompt = build_system_prompt(schema_context);
    let full_prompt = format!("{}\n\nQuestion: {}\nSQL:", system_prompt, user_prompt);

    let client = Client::new();
    let mut req = client.post(&slm_url).json(&SlmRequest {
        prompt: full_prompt,
        max_tokens: 512,
        temperature: 0.0,
        stop: vec![";".into(), "\n\n".into()],
    });

    if let Some(ref api_key) = config.slm_api_key {
        req = req.header("Authorization", format!("Api-Key {}", api_key));
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("SLM request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("SLM returned {}: {}", status, body));
    }

    let slm_resp: SlmResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse SLM response: {}", e))?;

    // Extract the generated text from various response formats
    let generated = extract_text(&slm_resp)
        .ok_or_else(|| "SLM returned empty response".to_string())?;

    info!("SLM generated: {}", generated.trim());

    parse_slm_output(&generated)
}

/// Extract text from the SLM response (supports multiple formats).
fn extract_text(resp: &SlmResponse) -> Option<String> {
    // Try OpenAI-compatible format
    if let Some(choices) = &resp.choices {
        if let Some(choice) = choices.first() {
            if let Some(ref text) = choice.text {
                return Some(text.clone());
            }
            if let Some(ref msg) = choice.message {
                if let Some(ref content) = msg.content {
                    return Some(content.clone());
                }
            }
        }
    }

    // Try simple output format
    if let Some(ref output) = resp.output {
        return Some(output.clone());
    }

    // Try Baseten model_output format
    if let Some(ref model_output) = resp.model_output {
        if let Some(text) = model_output.as_str() {
            return Some(text.to_string());
        }
        if let Some(obj) = model_output.as_object() {
            if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                return Some(text.to_string());
            }
            if let Some(text) = obj.get("generated_text").and_then(|v| v.as_str()) {
                return Some(text.to_string());
            }
        }
    }

    None
}

/// Parse the SLM output into a SQL query or explanation.
fn parse_slm_output(text: &str) -> Result<SlmResult, String> {
    let trimmed = text.trim();

    // Check if the model returned an explanation instead of SQL
    if trimmed.starts_with("EXPLAIN:") {
        return Ok(SlmResult::Explanation(
            trimmed.trim_start_matches("EXPLAIN:").trim().to_string(),
        ));
    }

    // Determine target: if the SQL starts with "-- turso", route to plugin-runtime
    let (target, sql) = if trimmed.starts_with("-- turso") {
        let sql = trimmed
            .lines()
            .skip(1)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();
        (QueryTarget::Turso, sql)
    } else {
        (QueryTarget::Delta, trimmed.to_string())
    };

    // Strip trailing semicolons
    let sql = sql.trim_end_matches(';').trim().to_string();

    if sql.is_empty() {
        return Ok(SlmResult::Explanation(
            "The model returned an empty query.".into(),
        ));
    }

    // Basic safety check — reject write operations
    let lower = sql.to_lowercase();
    for forbidden in &[
        "drop ", "delete ", "insert ", "update ", "alter ", "create ", "truncate ",
    ] {
        if lower.contains(forbidden) {
            return Err(format!(
                "Generated SQL contains forbidden keyword: {}",
                forbidden.trim()
            ));
        }
    }

    Ok(SlmResult::Sql { query: sql, target })
}

/// Mock SQL generation for testing when no SLM is configured.
fn mock_generate_sql(prompt: &str) -> SlmResult {
    let lower = prompt.to_lowercase();

    warn!("SLM_URL not configured — using mock SQL generation");

    if lower.contains("count") || lower.contains("how many") {
        SlmResult::Sql {
            query: "SELECT event_type, COUNT(*) AS count FROM scoped GROUP BY event_type ORDER BY count DESC".into(),
            target: QueryTarget::Delta,
        }
    } else if lower.contains("recent") || lower.contains("latest") || lower.contains("last") {
        SlmResult::Sql {
            query: "SELECT event_id, event_type, timestamp_ms, ip, user_agent FROM scoped ORDER BY timestamp_ms DESC LIMIT 20".into(),
            target: QueryTarget::Delta,
        }
    } else if lower.contains("top") || lower.contains("most") {
        SlmResult::Sql {
            query: "SELECT event_type, COUNT(*) AS count FROM scoped GROUP BY event_type ORDER BY count DESC LIMIT 10".into(),
            target: QueryTarget::Delta,
        }
    } else {
        SlmResult::Sql {
            query: "SELECT event_type, COUNT(*) AS count FROM scoped GROUP BY event_type ORDER BY count DESC LIMIT 100".into(),
            target: QueryTarget::Delta,
        }
    }
}

//! Query introspection and embedding configuration types.
//!
//! Loads query definitions from `introspect.json` including parameter types,
//! return types, and optional embedding configurations for vector search queries.

use serde::{Deserialize, Serialize};
use sonic_rs::Value;
use std::collections::HashMap;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EmbeddingConfig {
    pub provider_config: ProviderConfig,
    pub embedded_variables: Vec<String>,
}

/// Embedding provider configuration, deserialized from JSON.
///
/// Example JSON:
/// ```json
/// {
///   "provider": "openai",
///   "config": { "model": "text_embedding_3_large" }
/// }
/// ```
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "provider", content = "config")]
pub enum ProviderConfig {
    #[serde(rename = "openai")]
    OpenAI(OpenAIEmbeddingConfig),

    #[serde(rename = "azure")]
    Azure(AzureEmbeddingConfig),

    #[serde(rename = "gemini")]
    Gemini(GeminiEmbeddingConfig),

    #[serde(rename = "local")]
    Local(LocalEmbeddingConfig),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OpenAIEmbeddingConfig {
    pub model: OpenAIModel,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAIModel {
    TextEmbedding3Large,
    TextEmbedding3Small,
    TextEmbeddingAda002,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AzureEmbeddingConfig {
    pub model: AzureModel,
    pub deployment_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AzureModel {
    TextEmbedding3Large,
    TextEmbedding3Small,
    TextEmbeddingAda002,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GeminiEmbeddingConfig {
    pub model: GeminiModel,
    #[serde(default = "default_task_type")]
    pub task_type: String,
}

fn default_task_type() -> String {
    "RETRIEVAL_DOCUMENT".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GeminiModel {
    Embedding001,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LocalEmbeddingConfig {
    #[serde(default = "default_local_url")]
    pub url: String,
}

fn default_local_url() -> String {
    "http://localhost:8699/embed".to_string()
}

/// A single query definition loaded from introspect.json.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DbQuery {
    pub request_type: i32,
    pub parameters: pbjson_types::Struct,
    pub return_types: pbjson_types::Struct,
    pub embedding_config: Option<EmbeddingConfig>,
    pub is_mcp: bool,
}

/// Container for query definitions loaded from introspect.json.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Introspection {
    pub queries: HashMap<String, DbQuery>,
    pub schema: HashMap<String, Value>,
}

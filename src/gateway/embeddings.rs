//! Text embedding generation using multiple providers with client pooling.
//!
//! Supports OpenAI, Azure OpenAI, and Gemini via the Rig library,
//! plus a custom local embedding service. Clients are created once at
//! startup and reused across all requests for optimal throughput.

use crate::error::GatewayError;
use crate::gateway::introspection::{
    AzureEmbeddingConfig, AzureModel, EmbeddingConfig, GeminiEmbeddingConfig, GeminiModel,
    LocalEmbeddingConfig, OpenAIEmbeddingConfig, OpenAIModel, ProviderConfig, RequiredProviders,
};
use futures::future::try_join_all;
use rig::OneOrMany;
use rig::client::{EmbeddingsClient, ProviderClient};
use rig::embeddings::EmbeddingsBuilder;
use rig::providers::{azure, gemini, openai};
use sonic_rs::{JsonContainerTrait, JsonValueTrait, json};
use std::collections::HashMap;
use std::sync::Arc;

/// Pre-initialized pool of embedding provider clients.
///
/// All clients are thread-safe (Arc-wrapped) and can be cloned cheaply
/// for use across concurrent requests. This eliminates the overhead of
/// creating new clients and TLS connections per embedding request.
#[derive(Clone, Default)]
pub struct EmbeddingClientPool {
    openai: Option<Arc<openai::Client>>,
    azure: Option<Arc<azure::Client>>,
    gemini: Option<Arc<gemini::Client>>,
    /// Local embedding service clients, keyed by URL.
    /// Each unique URL gets its own connection-pooled reqwest client.
    local_clients: HashMap<String, Arc<reqwest::Client>>,
}

impl EmbeddingClientPool {
    /// Creates a new client pool based on the required providers.
    ///
    /// This should be called once at startup. Provider clients are
    /// initialized from environment variables per rig-core conventions.
    pub fn new(required: &RequiredProviders) -> Result<Self, GatewayError> {
        let mut pool = Self::default();

        if required.needs_openai {
            pool.openai = Some(Arc::new(openai::Client::from_env()));
        }

        if required.needs_azure {
            pool.azure = Some(Arc::new(azure::Client::from_env()));
        }

        if required.needs_gemini {
            pool.gemini = Some(Arc::new(gemini::Client::from_env()));
        }

        // Create a shared reqwest client for each unique local URL.
        // The reqwest::Client internally pools connections.
        for url in &required.local_urls {
            let client = reqwest::Client::builder()
                .pool_max_idle_per_host(10)
                .build()
                .map_err(|e| {
                    GatewayError::EmbeddingError(format!(
                        "Failed to create HTTP client for {}: {}",
                        url, e
                    ))
                })?;
            pool.local_clients.insert(url.clone(), Arc::new(client));
        }

        Ok(pool)
    }

    fn openai(&self) -> Result<&Arc<openai::Client>, GatewayError> {
        self.openai
            .as_ref()
            .ok_or_else(|| GatewayError::EmbeddingError("OpenAI client not initialized".into()))
    }

    fn azure(&self) -> Result<&Arc<azure::Client>, GatewayError> {
        self.azure
            .as_ref()
            .ok_or_else(|| GatewayError::EmbeddingError("Azure client not initialized".into()))
    }

    fn gemini(&self) -> Result<&Arc<gemini::Client>, GatewayError> {
        self.gemini
            .as_ref()
            .ok_or_else(|| GatewayError::EmbeddingError("Gemini client not initialized".into()))
    }

    fn local(&self, url: &str) -> Result<&Arc<reqwest::Client>, GatewayError> {
        self.local_clients.get(url).ok_or_else(|| {
            GatewayError::EmbeddingError(format!("Local client for {} not initialized", url))
        })
    }
}

/// Generates an embedding vector for a single text using the pooled client.
pub async fn embed(
    pool: &EmbeddingClientPool,
    config: &EmbeddingConfig,
    text: &str,
) -> Result<Vec<f64>, GatewayError> {
    match &config.provider_config {
        ProviderConfig::OpenAI(cfg) => embed_openai(pool, cfg, text).await,
        ProviderConfig::Azure(cfg) => embed_azure(pool, cfg, text).await,
        ProviderConfig::Gemini(cfg) => embed_gemini(pool, cfg, text).await,
        ProviderConfig::Local(cfg) => embed_local(pool, cfg, text).await,
    }
}

/// Batch embed multiple texts in a single API call where supported.
///
/// Returns a map of field_key -> embedding vector. For providers that support
/// batching (OpenAI, Azure, Gemini), this makes a single API call for all texts.
/// For the local service, this uses concurrent requests with connection pooling.
pub async fn embed_batch(
    pool: &EmbeddingClientPool,
    config: &EmbeddingConfig,
    texts: Vec<(&str, &str)>, // (field_key, text_value)
) -> Result<HashMap<String, Vec<f64>>, GatewayError> {
    if texts.is_empty() {
        return Ok(HashMap::new());
    }

    match &config.provider_config {
        ProviderConfig::OpenAI(cfg) => embed_batch_openai(pool, cfg, texts).await,
        ProviderConfig::Azure(cfg) => embed_batch_azure(pool, cfg, texts).await,
        ProviderConfig::Gemini(cfg) => embed_batch_gemini(pool, cfg, texts).await,
        ProviderConfig::Local(cfg) => embed_batch_local(pool, cfg, texts).await,
    }
}

// === OpenAI Implementation ===

async fn embed_openai(
    pool: &EmbeddingClientPool,
    cfg: &OpenAIEmbeddingConfig,
    text: &str,
) -> Result<Vec<f64>, GatewayError> {
    let client = pool.openai()?;
    let model_id = openai_model_id(cfg);
    let model = client.embedding_model(model_id);

    let embeddings = EmbeddingsBuilder::new(model)
        .document(text.to_string())
        .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?
        .build()
        .await
        .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?;

    extract_first_embedding(embeddings)
}

async fn embed_batch_openai(
    pool: &EmbeddingClientPool,
    cfg: &OpenAIEmbeddingConfig,
    texts: Vec<(&str, &str)>,
) -> Result<HashMap<String, Vec<f64>>, GatewayError> {
    let client = pool.openai()?;
    let model_id = openai_model_id(cfg);
    let model = client.embedding_model(model_id);

    // Build with all documents - rig-core batches them into a single API call

    let mut builder = EmbeddingsBuilder::new(model);
    for (_, text) in &texts {
        builder = builder
            .document(text)
            .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?;
    }

    let embeddings = builder
        .build()
        .await
        .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?;

    // Map results back to keys (embeddings are returned in order)
    let mut result = HashMap::new();
    for (key, (_, one_or_many)) in texts.iter().map(|(k, _)| k).zip(embeddings) {
        let vec: Vec<f64> = one_or_many.first().vec.iter().map(|&x| x as f64).collect();
        result.insert(key.to_string(), vec);
    }

    Ok(result)
}

fn openai_model_id(cfg: &OpenAIEmbeddingConfig) -> &'static str {
    match cfg.model {
        OpenAIModel::TextEmbedding3Large => openai::TEXT_EMBEDDING_3_LARGE,
        OpenAIModel::TextEmbedding3Small => openai::TEXT_EMBEDDING_3_SMALL,
        OpenAIModel::TextEmbeddingAda002 => openai::TEXT_EMBEDDING_ADA_002,
    }
}

// === Azure Implementation ===

async fn embed_azure(
    pool: &EmbeddingClientPool,
    cfg: &AzureEmbeddingConfig,
    text: &str,
) -> Result<Vec<f64>, GatewayError> {
    let client = pool.azure()?;
    let model_id = azure_model_id(cfg);
    let model = client.embedding_model(model_id);

    let embeddings = EmbeddingsBuilder::new(model)
        .document(text.to_string())
        .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?
        .build()
        .await
        .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?;

    extract_first_embedding(embeddings)
}

async fn embed_batch_azure(
    pool: &EmbeddingClientPool,
    cfg: &AzureEmbeddingConfig,
    texts: Vec<(&str, &str)>,
) -> Result<HashMap<String, Vec<f64>>, GatewayError> {
    let client = pool.azure()?;
    let model_id = azure_model_id(cfg);
    let model = client.embedding_model(model_id);

    let mut builder = EmbeddingsBuilder::new(model);
    for (_, text) in &texts {
        builder = builder
            .document(text)
            .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?;
    }

    let embeddings = builder
        .build()
        .await
        .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?;

    let mut result = HashMap::new();
    for (key, (_, one_or_many)) in texts.iter().map(|(k, _)| k).into_iter().zip(embeddings) {
        let vec: Vec<f64> = one_or_many.first().vec.iter().map(|&x| x as f64).collect();
        result.insert(key.to_string(), vec);
    }

    Ok(result)
}

fn azure_model_id(cfg: &AzureEmbeddingConfig) -> &'static str {
    match cfg.model {
        AzureModel::TextEmbedding3Large => azure::TEXT_EMBEDDING_3_LARGE,
        AzureModel::TextEmbedding3Small => azure::TEXT_EMBEDDING_3_SMALL,
        AzureModel::TextEmbeddingAda002 => azure::TEXT_EMBEDDING_ADA_002,
    }
}

// === Gemini Implementation ===

async fn embed_gemini(
    pool: &EmbeddingClientPool,
    cfg: &GeminiEmbeddingConfig,
    text: &str,
) -> Result<Vec<f64>, GatewayError> {
    let client = pool.gemini()?;
    let model_id = gemini_model_id(cfg);
    let model = client.embedding_model(model_id);

    let embeddings = EmbeddingsBuilder::new(model)
        .document(text.to_string())
        .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?
        .build()
        .await
        .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?;

    extract_first_embedding(embeddings)
}

async fn embed_batch_gemini(
    pool: &EmbeddingClientPool,
    cfg: &GeminiEmbeddingConfig,
    texts: Vec<(&str, &str)>,
) -> Result<HashMap<String, Vec<f64>>, GatewayError> {
    let client = pool.gemini()?;
    let model_id = gemini_model_id(cfg);
    let model = client.embedding_model(model_id);

    let mut builder = EmbeddingsBuilder::new(model);
    for (_, text) in &texts {
        builder = builder
            .document(text)
            .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?;
    }

    let embeddings = builder
        .build()
        .await
        .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?;

    let mut result = HashMap::new();
    for (key, (_, one_or_many)) in texts.iter().map(|(k, _)| k).zip(embeddings) {
        let vec: Vec<f64> = one_or_many.first().vec.iter().map(|&x| x as f64).collect();
        result.insert(key.to_string(), vec);
    }

    Ok(result)
}

fn gemini_model_id(cfg: &GeminiEmbeddingConfig) -> &'static str {
    match cfg.model {
        GeminiModel::Embedding001 => gemini::embedding::EMBEDDING_001,
    }
}

// === Local Service Implementation ===

async fn embed_local(
    pool: &EmbeddingClientPool,
    cfg: &LocalEmbeddingConfig,
    text: &str,
) -> Result<Vec<f64>, GatewayError> {
    let client = pool.local(&cfg.url)?;

    let response = client
        .post(&cfg.url)
        .json(&json!({
            "text": text,
            "chunk_style": "recursive",
            "chunk_size": 100
        }))
        .send()
        .await
        .map_err(|e| GatewayError::EmbeddingError(format!("Request failed: {e}")))?;

    parse_local_embedding_response(response).await
}

async fn embed_batch_local(
    pool: &EmbeddingClientPool,
    cfg: &LocalEmbeddingConfig,
    texts: Vec<(&str, &str)>,
) -> Result<HashMap<String, Vec<f64>>, GatewayError> {
    // Local service may not support batch - use concurrent individual calls.
    // This still benefits from connection pooling via the shared reqwest::Client.
    let client = pool.local(&cfg.url)?;

    let futures: Vec<_> = texts
        .into_iter()
        .map(|(key, text)| {
            let client = Arc::clone(client);
            let url = cfg.url.clone();
            async move {
                let response = client
                    .post(&url)
                    .json(&json!({
                        "text": text,
                        "chunk_style": "recursive",
                        "chunk_size": 100
                    }))
                    .send()
                    .await
                    .map_err(|e| GatewayError::EmbeddingError(format!("Request failed: {e}")))?;

                let embedding = parse_local_embedding_response(response).await?;
                Ok::<_, GatewayError>((key, embedding))
            }
        })
        .collect();

    let results = try_join_all(futures).await?;
    Ok(results
        .into_iter()
        .map(|(key, embedding)| (key.to_string(), embedding))
        .collect())
}

async fn parse_local_embedding_response(
    response: reqwest::Response,
) -> Result<Vec<f64>, GatewayError> {
    let text_response = response
        .text()
        .await
        .map_err(|e| GatewayError::EmbeddingError(format!("Failed to read response: {e}")))?;

    let json: sonic_rs::Value = sonic_rs::from_str(&text_response)
        .map_err(|e| GatewayError::EmbeddingError(format!("Failed to parse JSON: {e}")))?;

    json["embedding"]
        .as_array()
        .ok_or_else(|| GatewayError::EmbeddingError("Invalid embedding format".to_string()))?
        .iter()
        .map(|v| {
            v.as_f64()
                .ok_or_else(|| GatewayError::EmbeddingError("Invalid float value".to_string()))
        })
        .collect()
}

// === Helper Functions ===

fn extract_first_embedding(
    embeddings: Vec<(String, OneOrMany<rig::embeddings::Embedding>)>,
) -> Result<Vec<f64>, GatewayError> {
    let (_, one_or_many) = embeddings
        .into_iter()
        .next()
        .ok_or_else(|| GatewayError::EmbeddingError("No embedding returned".to_string()))?;

    Ok(one_or_many.first().vec.iter().map(|&x| x as f64).collect())
}

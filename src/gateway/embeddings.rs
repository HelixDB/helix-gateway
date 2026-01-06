//! Text embedding generation using multiple providers.
//!
//! Supports OpenAI, Azure OpenAI, and Gemini via the Rig library,
//! plus a custom local embedding service.

use crate::error::GatewayError;
use crate::gateway::introspection::{
    AzureEmbeddingConfig, AzureModel, EmbeddingConfig, GeminiEmbeddingConfig, GeminiModel,
    LocalEmbeddingConfig, OpenAIEmbeddingConfig, OpenAIModel, ProviderConfig,
};
use rig::client::{EmbeddingsClient, ProviderClient};
use rig::embeddings::EmbeddingsBuilder;
use rig::providers::{azure, gemini, openai};
use sonic_rs::{JsonContainerTrait, JsonValueTrait, json};

/// Generates an embedding vector for the given text using the configured provider.
pub async fn embed(config: &EmbeddingConfig, text: &str) -> Result<Vec<f64>, GatewayError> {
    match &config.provider_config {
        ProviderConfig::OpenAI(cfg) => embed_openai(cfg, text).await,
        ProviderConfig::Azure(cfg) => embed_azure(cfg, text).await,
        ProviderConfig::Gemini(cfg) => embed_gemini(cfg, text).await,
        ProviderConfig::Local(cfg) => embed_local(cfg, text).await,
    }
}

async fn embed_openai(cfg: &OpenAIEmbeddingConfig, text: &str) -> Result<Vec<f64>, GatewayError> {
    let client = openai::Client::from_env();

    let model_id = match cfg.model {
        OpenAIModel::TextEmbedding3Large => openai::TEXT_EMBEDDING_3_LARGE,
        OpenAIModel::TextEmbedding3Small => openai::TEXT_EMBEDDING_3_SMALL,
        OpenAIModel::TextEmbeddingAda002 => openai::TEXT_EMBEDDING_ADA_002,
    };

    let model = client.embedding_model(model_id);
    let embeddings = EmbeddingsBuilder::new(model)
        .document(text.to_string())
        .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?
        .build()
        .await
        .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?;

    // Get the first embedding's vector
    // Returns Vec<(String, OneOrMany<Embedding>)>
    let (_, one_or_many) = embeddings
        .into_iter()
        .next()
        .ok_or_else(|| GatewayError::EmbeddingError("No embedding returned".to_string()))?;

    let embedding = one_or_many.first();
    Ok(embedding.vec.iter().map(|&x| x as f64).collect())
}

async fn embed_azure(cfg: &AzureEmbeddingConfig, text: &str) -> Result<Vec<f64>, GatewayError> {
    let client = azure::Client::from_env();

    let model_id = match cfg.model {
        AzureModel::TextEmbedding3Large => azure::TEXT_EMBEDDING_3_LARGE,
        AzureModel::TextEmbedding3Small => azure::TEXT_EMBEDDING_3_SMALL,
        AzureModel::TextEmbeddingAda002 => azure::TEXT_EMBEDDING_ADA_002,
    };

    let model = client.embedding_model(model_id);
    let embeddings = EmbeddingsBuilder::new(model)
        .document(text.to_string())
        .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?
        .build()
        .await
        .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?;

    let (_, one_or_many) = embeddings
        .into_iter()
        .next()
        .ok_or_else(|| GatewayError::EmbeddingError("No embedding returned".to_string()))?;

    let embedding = one_or_many.first();
    Ok(embedding.vec.iter().map(|&x| x as f64).collect())
}

async fn embed_gemini(cfg: &GeminiEmbeddingConfig, text: &str) -> Result<Vec<f64>, GatewayError> {
    let client = gemini::Client::from_env();

    let model_id = match cfg.model {
        GeminiModel::Embedding001 => gemini::embedding::EMBEDDING_001,
    };

    let model = client.embedding_model(model_id);
    let embeddings = EmbeddingsBuilder::new(model)
        .document(text.to_string())
        .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?
        .build()
        .await
        .map_err(|e| GatewayError::EmbeddingError(e.to_string()))?;

    let (_, one_or_many) = embeddings
        .into_iter()
        .next()
        .ok_or_else(|| GatewayError::EmbeddingError("No embedding returned".to_string()))?;

    let embedding = one_or_many.first();
    Ok(embedding.vec.iter().map(|&x| x as f64).collect())
}

async fn embed_local(cfg: &LocalEmbeddingConfig, text: &str) -> Result<Vec<f64>, GatewayError> {
    // Local embedding service - keep custom implementation
    // since Rig doesn't have a built-in local provider (rig-fastembed is separate)
    let client = reqwest::Client::new();

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

    let text_response = response
        .text()
        .await
        .map_err(|e| GatewayError::EmbeddingError(format!("Failed to read response: {e}")))?;

    let json: sonic_rs::Value = sonic_rs::from_str(&text_response)
        .map_err(|e| GatewayError::EmbeddingError(format!("Failed to parse JSON: {e}")))?;

    let embedding = json["embedding"]
        .as_array()
        .ok_or_else(|| GatewayError::EmbeddingError("Invalid embedding format".to_string()))?
        .iter()
        .map(|v| {
            v.as_f64()
                .ok_or_else(|| GatewayError::EmbeddingError("Invalid float value".to_string()))
        })
        .collect::<Result<Vec<f64>, GatewayError>>()?;

    Ok(embedding)
}

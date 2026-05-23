use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
use tracing::{debug, warn};

use crate::config::LlmConfig;
use crate::types::{ChatCompletionRequest, ChatCompletionResponse, EmbeddingResponse};

#[derive(Clone)]
pub struct LlmClient {
    client: Client,
    pub base_url: String,
    pub api_key: Option<String>,
}

impl LlmClient {
    pub fn new(config: &LlmConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()?;
        Ok(Self {
            client,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            api_key: config.api_key.clone(),
        })
    }

    fn auth_header(&self) -> Option<String> {
        self.api_key.as_ref().map(|k| format!("Bearer {}", k))
    }

    pub async fn embed(&self, texts: &[&str], model: &str) -> Result<Vec<Vec<f32>>> {
        let input = if texts.len() == 1 { json!(texts[0]) } else { json!(texts) };
        let body = json!({ "model": model, "input": input });
        debug!("embedding {} texts with model {}", texts.len(), model);

        let mut req = self.client.post(format!("{}/v1/embeddings", self.base_url)).json(&body);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("embedding failed {}: {}", status, text));
        }

        let data: EmbeddingResponse = resp.json().await?;
        let mut result: Vec<(usize, Vec<f32>)> =
            data.data.into_iter().map(|d| (d.index, d.embedding)).collect();
        result.sort_by_key(|(i, _)| *i);
        Ok(result.into_iter().map(|(_, e)| e).collect())
    }

    pub async fn embed_one(&self, text: &str, model: &str) -> Result<Vec<f32>> {
        let mut results = self.embed(&[text], model).await?;
        results.pop().ok_or_else(|| anyhow!("no embedding returned"))
    }

    pub async fn chat(&self, request: &ChatCompletionRequest) -> Result<ChatCompletionResponse> {
        debug!("chat with model {}", request.model);

        let mut req = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(request);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("chat failed {}: {}", status, text));
        }

        Ok(resp.json::<ChatCompletionResponse>().await?)
    }

    pub async fn chat_stream(&self, request: &ChatCompletionRequest) -> Result<reqwest::Response> {
        let mut req = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(request);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("stream failed {}: {}", status, text));
        }
        Ok(resp)
    }

    pub async fn list_models(&self) -> Result<Vec<RemoteModel>> {
        let mut req = self.client.get(format!("{}/v1/models", self.base_url));
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                let data: ModelsResponse = resp.json().await?;
                Ok(data.data)
            }
            Ok(resp) => {
                warn!("models list returned {}", resp.status());
                Ok(vec![])
            }
            Err(e) => {
                warn!("llm server unreachable: {}", e);
                Ok(vec![])
            }
        }
    }

    pub async fn health(&self) -> bool {
        self.client
            .get(format!("{}/health", self.base_url))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    pub async fn rerank_with_llm(
        &self,
        query: &str,
        documents: &[String],
        model: &str,
    ) -> Result<Vec<f32>> {
        let query_emb = self.embed_one(query, model).await?;
        let mut scores = Vec::with_capacity(documents.len());
        for doc in documents {
            let doc_emb = self.embed_one(doc, model).await?;
            scores.push(crate::db::cosine_similarity(&query_emb, &doc_emb));
        }
        Ok(scores)
    }
}

#[derive(Debug, Deserialize)]
pub struct ModelsResponse {
    pub data: Vec<RemoteModel>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RemoteModel {
    pub id: String,
    pub object: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<String>,
}

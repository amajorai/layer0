use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use crate::config::{ChatConfig, LlmConfig};
use crate::types::{ChatCompletionRequest, ChatCompletionResponse, EmbeddingResponse};

/// Talks to two OpenAI-wire-format backends: a local embeddings server
/// (`base_url`, the llama-server sidecar) and a chat backend (`chat_base_url`,
/// Claude via Anthropic's OpenAI-compatible endpoint by default).
#[derive(Clone)]
pub struct LlmClient {
    client: Client,
    pub base_url: String,
    pub api_key: Option<String>,
    chat_base_url: String,
    chat_api_key: Option<String>,
    // All clones serving default/named databases share this admission budget.
    inference_slots: Arc<Semaphore>,
}

impl LlmClient {
    pub fn new(llm: &LlmConfig, chat: &ChatConfig) -> Result<Self> {
        let timeout = llm.timeout_secs.max(chat.timeout_secs);
        let client = Client::builder()
            .timeout(Duration::from_secs(timeout))
            .build()?;
        Ok(Self {
            client,
            base_url: llm.base_url.trim_end_matches('/').to_string(),
            api_key: llm.api_key.clone(),
            chat_base_url: chat.base_url.trim_end_matches('/').to_string(),
            chat_api_key: chat.api_key.clone(),
            inference_slots: Arc::new(Semaphore::new(4)),
        })
    }

    fn auth_header(&self) -> Option<String> {
        self.api_key.as_ref().map(|k| format!("Bearer {}", k))
    }

    fn chat_auth_header(&self) -> Option<String> {
        self.chat_api_key.as_ref().map(|k| format!("Bearer {}", k))
    }

    pub async fn embed(&self, texts: &[&str], model: &str) -> Result<Vec<Vec<f32>>> {
        let _permit = self
            .inference_slots
            .try_acquire()
            .map_err(|_| anyhow!("inference capacity exhausted"))?;
        let input = if texts.len() == 1 {
            json!(texts[0])
        } else {
            json!(texts)
        };
        let body = json!({ "model": model, "input": input });
        debug!("embedding {} texts with model {}", texts.len(), model);

        let mut req = self
            .client
            .post(format!("{}/v1/embeddings", self.base_url))
            .json(&body);
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
        let mut result: Vec<(usize, Vec<f32>)> = data
            .data
            .into_iter()
            .map(|d| (d.index, d.embedding))
            .collect();
        result.sort_by_key(|(i, _)| *i);
        Ok(result.into_iter().map(|(_, e)| e).collect())
    }

    pub async fn embed_one(&self, text: &str, model: &str) -> Result<Vec<f32>> {
        let mut results = self.embed(&[text], model).await?;
        results
            .pop()
            .ok_or_else(|| anyhow!("no embedding returned"))
    }

    pub async fn chat(&self, request: &ChatCompletionRequest) -> Result<ChatCompletionResponse> {
        let _permit = self
            .inference_slots
            .try_acquire()
            .map_err(|_| anyhow!("inference capacity exhausted"))?;
        debug!("chat with model {}", request.model);

        let mut req = self
            .client
            .post(format!("{}/v1/chat/completions", self.chat_base_url))
            .json(request);
        if let Some(auth) = self.chat_auth_header() {
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
            .post(format!("{}/v1/chat/completions", self.chat_base_url))
            .json(request);
        if let Some(auth) = self.chat_auth_header() {
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

#[cfg(test)]
mod concurrency_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn embedding_and_graph_requests_share_a_fail_fast_budget_and_release_it() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let (arrived, mut arrivals) = tokio::sync::mpsc::channel(16);
        let (release, wait) = tokio::sync::watch::channel(false);
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let arrived = arrived.clone();
                let mut wait = wait.clone();
                tokio::spawn(async move {
                    let mut bytes = [0u8; 8192];
                    let size = socket.read(&mut bytes).await.unwrap();
                    let chat =
                        String::from_utf8_lossy(&bytes[..size]).contains("/chat/completions");
                    arrived.send(()).await.unwrap();
                    if !*wait.borrow() {
                        wait.changed().await.unwrap();
                    }
                    let body = if chat {
                        r#"{"id":"test","object":"chat.completion","created":0,"model":"test","choices":[]}"#
                    } else {
                        r#"{"object":"list","model":"test","data":[{"object":"embedding","index":0,"embedding":[1.0]}],"usage":{"prompt_tokens":1,"total_tokens":1}}"#
                    };
                    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
                });
            }
        });
        let config = LlmConfig {
            base_url: base_url.clone(),
            ..crate::config::Config::default().llm
        };
        let chat = ChatConfig {
            base_url,
            ..crate::config::Config::default().chat
        };
        let client = LlmClient::new(&config, &chat).unwrap();
        let mut requests = Vec::new();
        for _ in 0..4 {
            let client = client.clone();
            requests.push(tokio::spawn(async move {
                client.embed_one("ordinary document", "test").await
            }));
        }
        for _ in 0..4 {
            tokio::time::timeout(Duration::from_secs(5), arrivals.recv())
                .await
                .unwrap()
                .unwrap();
        }
        let graph_request = ChatCompletionRequest {
            model: "test".into(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: None,
            top_p: None,
            stop: None,
        };
        for _ in 0..20 {
            assert!(client
                .embed_one("extra document", "test")
                .await
                .unwrap_err()
                .to_string()
                .contains("capacity exhausted"));
            assert!(client
                .chat(&graph_request)
                .await
                .unwrap_err()
                .to_string()
                .contains("capacity exhausted"));
        }
        assert!(
            arrivals.try_recv().is_err(),
            "denied requests must not reach the provider"
        );
        release.send(true).unwrap();
        for request in requests {
            assert_eq!(request.await.unwrap().unwrap(), vec![1.0]);
        }
        assert!(
            client.chat(&graph_request).await.is_ok(),
            "ordinary graph work resumes after completion"
        );
        assert!(client.embed_one("next document", "test").await.is_ok());
        server.abort();
    }
}

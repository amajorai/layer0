use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcResponse {
    pub fn ok(id: Option<Value>, result: Value) -> Self {
        Self { jsonrpc: "2.0".into(), id, result: Some(result), error: None }
    }
    pub fn err(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self { jsonrpc: "2.0".into(), id, result: None, error: Some(RpcError { code, message: message.into() }) }
    }
}

#[derive(Debug, Serialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

pub fn all_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "store_memory".into(),
            description: "Store a document in layerzero memory with automatic embedding.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "Content to store" },
                    "source": { "type": "string", "description": "Optional source identifier" },
                    "metadata": { "type": "object", "description": "Optional JSON metadata" }
                },
                "required": ["content"]
            }),
        },
        McpTool {
            name: "search_memory".into(),
            description: "Search memory using semantic vector search and keyword search.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query" },
                    "limit": { "type": "integer", "description": "Max results (default: 5)" },
                    "rerank": { "type": "boolean", "description": "Apply reranking" }
                },
                "required": ["query"]
            }),
        },
        McpTool {
            name: "rag_query".into(),
            description: "Ask a question and get an answer grounded in stored memory via RAG.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Question to answer" },
                    "limit": { "type": "integer", "description": "Context documents to use (default: 5)" },
                    "system_prompt": { "type": "string", "description": "Custom system prompt" }
                },
                "required": ["query"]
            }),
        },
        McpTool {
            name: "get_document".into(),
            description: "Retrieve a document by ID.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
        },
        McpTool {
            name: "delete_memory".into(),
            description: "Delete a document from memory.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
        },
        McpTool {
            name: "graph_query".into(),
            description: "Traverse the knowledge graph from a starting node.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "start_node_id": { "type": "string" },
                    "start_label": { "type": "string" },
                    "depth": { "type": "integer", "description": "Traversal depth (default: 2)" },
                    "relation": { "type": "string", "description": "Filter by edge relation" }
                }
            }),
        },
        McpTool {
            name: "memory_stats".into(),
            description: "Get statistics about the layerzero memory store.".into(),
            input_schema: serde_json::json!({ "type": "object", "properties": {} }),
        },
    ]
}

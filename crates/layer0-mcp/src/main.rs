mod protocol;
mod tools;

use anyhow::Result;
use clap::Parser;
use layer0_core::config::Config;
use protocol::{all_tools, JsonRpcRequest, JsonRpcResponse};
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tools::ToolContext;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[derive(Parser)]
#[command(name = "layer0-mcp", about = "layer0 MCP server (stdio JSON-RPC 2.0)")]
struct Args {
    #[arg(short, long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "layer0=info".into()))
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr).without_time())
        .init();

    let args = Args::parse();
    let config = Config::load(args.config.as_ref())?;
    let ctx = ToolContext::new(config).await?;
    tracing::info!("layer0 MCP server ready");

    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();

    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse::err(None, -32700, format!("parse error: {}", e));
                write_line(&mut stdout, &resp).await?;
                continue;
            }
        };

        let resp = handle(&ctx, request).await;
        write_line(&mut stdout, &resp).await?;
    }

    Ok(())
}

async fn write_line(stdout: &mut tokio::io::Stdout, resp: &JsonRpcResponse) -> Result<()> {
    let mut s = serde_json::to_string(resp)?;
    s.push('\n');
    stdout.write_all(s.as_bytes()).await?;
    stdout.flush().await?;
    Ok(())
}

async fn handle(ctx: &ToolContext, req: JsonRpcRequest) -> JsonRpcResponse {
    let id = req.id.clone();
    match req.method.as_str() {
        "initialize" => JsonRpcResponse::ok(id, json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "layer0", "version": env!("CARGO_PKG_VERSION") }
        })),

        "notifications/initialized" | "ping" => JsonRpcResponse::ok(id, json!({})),

        "tools/list" => JsonRpcResponse::ok(id, json!({ "tools": all_tools() })),

        "tools/call" => {
            let params = req.params.as_ref().and_then(|p| p.as_object());
            let name = params.and_then(|p| p.get("name")).and_then(|n| n.as_str()).unwrap_or("");
            let args = params.and_then(|p| p.get("arguments")).cloned().unwrap_or(json!({}));

            match dispatch(ctx, name, &args).await {
                Ok(content) => JsonRpcResponse::ok(id, json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&content).unwrap_or_default() }],
                    "isError": false
                })),
                Err(e) => JsonRpcResponse::ok(id, json!({
                    "content": [{ "type": "text", "text": format!("Error: {}", e) }],
                    "isError": true
                })),
            }
        }

        "resources/list" => JsonRpcResponse::ok(id, json!({ "resources": [] })),
        "prompts/list" => JsonRpcResponse::ok(id, json!({ "prompts": [] })),

        m => {
            tracing::warn!("unknown method: {}", m);
            JsonRpcResponse::err(id, -32601, format!("method not found: {}", m))
        }
    }
}

async fn dispatch(ctx: &ToolContext, name: &str, args: &Value) -> anyhow::Result<Value> {
    match name {
        "store_memory" => tools::store_memory(ctx, args).await,
        "search_memory" => tools::search_memory(ctx, args).await,
        "rag_query" => tools::rag_tool(ctx, args).await,
        "get_document" => tools::get_document_tool(ctx, args).await,
        "delete_memory" => tools::delete_memory_tool(ctx, args).await,
        "graph_query" => tools::graph_query_tool(ctx, args).await,
        "memory_stats" => tools::memory_stats_tool(ctx, args).await,
        "list_databases" => tools::list_databases_tool(ctx, args).await,
        "create_database" => tools::create_database_tool(ctx, args).await,
        "delete_database" => tools::delete_database_tool(ctx, args).await,
        "list_collections" => tools::list_collections_tool(ctx, args).await,
        "create_collection" => tools::create_collection_tool(ctx, args).await,
        "delete_collection" => tools::delete_collection_tool(ctx, args).await,
        other => Err(anyhow::anyhow!("unknown tool: {}", other)),
    }
}

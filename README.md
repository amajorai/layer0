# layerzero

A fully self-hostable memory and RAG layer for AI agents. Written entirely in Rust.

Stores documents with local vector embeddings, builds a knowledge graph, indexes for full-text search, and answers questions via RAG - all without leaving your machine. Plugs into any AI agent through an OpenAI-compatible HTTP API, an MCP server for Claude Code, or directly via CLI.

## Features

- Local vector embeddings via llama.cpp (no cloud, no API keys required)
- Hybrid semantic + keyword search using vector similarity and FTS5 BM25
- Knowledge graph stored in SQLite with BFS traversal
- Full RAG pipeline with Reciprocal Rank Fusion and optional reranking
- OpenAI-compatible API endpoints for drop-in compatibility
- MCP server for Claude Code, Cursor, Codeium, and other MCP clients
- One-command llama.cpp installation from GitHub releases
- HuggingFace model downloads with progress tracking
- Single SQLite database - no extra services to run
- Works with any OpenAI-compatible backend (Ollama, vLLM, OpenAI)

## Architecture

```
layerzero/
  crates/
    layerzero-core/    core library: DB, embeddings, graph, RAG, LLM client
    layerzero-server/  HTTP API server (OpenAI-compatible)
    layerzero-cli/     CLI (layerzero binary)
    layerzero-mcp/     MCP server for Claude Code and other agents
```

Data is stored in a single SQLite database at `~/.layerzero/layerzero.db`:

- `documents` - raw content and metadata with FTS5 full-text index
- `embeddings` - float32 vectors stored as BLOBs, searched with cosine similarity in Rust
- `graph_nodes` + `graph_edges` - knowledge graph with adjacency list
- `models` - model registry

## Quick start

### 1. Build

```bash
git clone https://github.com/amajorai/layerzero
cd layerzero
cargo build --release
```

Binaries land in `target/release/`: `layerzero`, `layerzero-server`, `layerzero-mcp`.

### 2. Install llama.cpp

```bash
# Downloads the latest release for your platform from GitHub
layerzero install llama
```

Or point to an existing server by setting `LAYERZERO_LLM_BASE_URL` (Ollama, vLLM, OpenAI, etc.).

### 3. Download a model

```bash
# Small chat model
layerzero model download bartowski/Llama-3.2-1B-Instruct-GGUF Llama-3.2-1B-Instruct-Q4_K_M.gguf

# Embedding model
layerzero model download nomic-ai/nomic-embed-text-v1.5-GGUF nomic-embed-text-v1.5.Q4_K_M.gguf
```

### 4. Start llama-server

```bash
llama-server \
  --model ~/.layerzero/models/Llama-3.2-1B-Instruct-Q4_K_M.gguf \
  --port 8081 \
  --embedding \
  --parallel 4
```

### 5. Start layerzero

```bash
layerzero init
layerzero-server
# Server on http://127.0.0.1:8080
```

## CLI

```bash
# Store documents
layerzero store "Paris is the capital of France, located in northern France."
echo "Long document..." | layerzero store --source "notes/europe.txt"

# Search memory
layerzero search "capital of France"
layerzero search "European capitals" --limit 10 --rerank

# RAG - ask a question grounded in stored memory
layerzero ask "What do we know about France?"

# Database stats
layerzero db stats
layerzero db list

# Check status
layerzero status
```

## HTTP API

All routes are under `http://localhost:8080`.

### Documents

```bash
# Store
curl -X POST /v1/documents \
  -H "Content-Type: application/json" \
  -d '{"content": "The Eiffel Tower is 330m tall.", "source": "wiki"}'

# Hybrid search (vector + keyword + optional graph expansion + optional rerank)
curl -X POST /v1/search \
  -H "Content-Type: application/json" \
  -d '{"query": "Eiffel Tower height", "limit": 5, "rerank": true, "use_graph": true}'

# RAG
curl -X POST /v1/rag \
  -H "Content-Type: application/json" \
  -d '{"query": "How tall is the Eiffel Tower?", "limit": 3}'

# List / get / delete
curl /v1/documents
curl /v1/documents/{id}
curl -X DELETE /v1/documents/{id}
```

### Graph

```bash
# Create node
curl -X POST /v1/graph/nodes \
  -d '{"label": "Paris", "properties": {"country": "France"}}'

# Create edge
curl -X POST /v1/graph/edges \
  -d '{"source_id": "...", "target_id": "...", "relation": "capital_of"}'

# BFS traversal
curl -X POST /v1/graph/query \
  -d '{"start_label": "Paris", "depth": 2, "relation": "capital_of"}'

# Store document with auto-graph creation
curl -X POST /v1/documents -d '{
  "content": "Paris is the capital of France.",
  "nodes": [
    {
      "label": "Paris",
      "edges": [{"target_label": "France", "relation": "capital_of"}]
    }
  ]
}'
```

### OpenAI-compatible endpoints

These are drop-in compatible with any OpenAI client:

```bash
# Embeddings
curl -X POST /v1/embeddings \
  -d '{"model": "local", "input": "Hello world"}'

# Chat completions (streaming supported)
curl -X POST /v1/chat/completions \
  -d '{"model": "local", "messages": [{"role": "user", "content": "Hello"}], "stream": true}'

# List models
curl /v1/models
```

### Models

```bash
# Download from HuggingFace
curl -X POST /v1/models/download \
  -d '{"repo": "bartowski/Llama-3.2-1B-Instruct-GGUF", "filename": "Llama-3.2-1B-Instruct-Q4_K_M.gguf", "model_type": "chat"}'

# Install llama.cpp
curl -X POST /v1/models/install-llama

# Stats
curl /v1/stats
```

## MCP integration

Add to `~/.claude/mcp.json` or project `.claude/mcp.json`:

```json
{
  "layerzero": {
    "command": "/path/to/layerzero-mcp"
  }
}
```

With environment overrides:

```json
{
  "layerzero": {
    "command": "layerzero-mcp",
    "env": {
      "LAYERZERO_LLM_BASE_URL": "http://127.0.0.1:8081"
    }
  }
}
```

### MCP tools

| Tool | What it does |
|------|-------------|
| `store_memory` | Store a document with automatic embedding |
| `search_memory` | Semantic + keyword hybrid search |
| `rag_query` | Answer a question grounded in memory |
| `get_document` | Fetch a document by ID |
| `delete_memory` | Remove a document |
| `graph_query` | Traverse the knowledge graph |
| `memory_stats` | Database statistics |

## Connecting Cursor, Codex, or any OpenAI client

Point the base URL at `http://localhost:8080` and use any API key (or none). Embeddings route to your local llama.cpp instance. Chat routes through too, giving you a unified local LLM proxy with memory.

## Configuration

Global config: `~/.layerzero/config.toml`

```toml
[server]
host = "127.0.0.1"
port = 8080

[llm]
base_url = "http://127.0.0.1:8081"
chat_model = "local"
embedding_model = "local"
timeout_secs = 120
# api_key = "sk-..."

[embeddings]
dimensions = 1536
batch_size = 16

[installer]
llama_server_port = 8081
# hf_token = "hf_..."
```

Environment variables (prefix `LAYERZERO_`):

```bash
LAYERZERO_SERVER_PORT=9090
LAYERZERO_LLM_BASE_URL=http://localhost:11434/v1
LAYERZERO_LLM_API_KEY=sk-...
HF_TOKEN=hf_...
```

## Using a remote LLM

layerzero works with any OpenAI-compatible backend:

```toml
# Ollama
[llm]
base_url = "http://localhost:11434/v1"
chat_model = "llama3.2"
embedding_model = "nomic-embed-text"

# OpenAI
[llm]
base_url = "https://api.openai.com"
api_key = "sk-..."
chat_model = "gpt-4o-mini"
embedding_model = "text-embedding-3-small"
```

## RAG pipeline

1. Query embedded with configured embedding model
2. Cosine similarity over all stored vectors (pure Rust, no index service needed)
3. FTS5 BM25 keyword search runs in parallel
4. Results fused with Reciprocal Rank Fusion (k=60)
5. Optional graph expansion: top document's linked nodes fetched and merged
6. Optional reranking: re-scores all results by embedding similarity to query
7. Top-k context passed to LLM with the question

## Building from source

Requires Rust 1.75+. SQLite is bundled - no system dependencies.

```bash
cargo build --release
```

## License

MIT

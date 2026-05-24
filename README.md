# 🌐 layer0

A self-hostable RAG / long-term-memory server for AI agents, written in Rust.

It stores documents, splits them into overlapping chunks, embeds each chunk
locally, and indexes them with **sqlite-vec** (vector ANN) + **FTS5** (BM25
keyword) for hybrid retrieval — then answers questions with RAG. Everything lives
in a single SQLite file. Plug it into any agent via an OpenAI-compatible HTTP
API, an MCP server (Claude Code, Cursor, …), or the CLI.

It is **frictionless**: `layer0 serve` auto-installs llama.cpp, auto-downloads
the default models, and starts the local sidecar for you. It runs **fully
offline on any computer** out of the box.

## Features

- **Chunked retrieval** — documents are chunked and embedded per chunk; RAG uses
  the matched chunk for tight context.
- **sqlite-vec ANN index** — cosine KNN over a `vec0` virtual table, not a brute
  force scan.
- **Configurable RAG modes** — `hybrid` (vector + knowledge graph, then rerank,
  default), `vector` (semantic only), or `graph` (graph-led). Vector + FTS5 BM25
  are fused with Reciprocal Rank Fusion.
- **Knowledge graph, auto-built at ingest** — entities + relationships are
  extracted by the chat LLM when documents are stored, so graph/hybrid retrieval
  has real data (not just manually-added nodes).
- **Local-first, zero-config** — `serve` installs llama.cpp, downloads the
  default embedding (nomic) + chat (gemma-4-E4B) models, and starts the sidecar(s).
- **Flexible chat backend** — resolves ACP (planned) → a remote backend like
  Claude (when an API key is set) → a local gemma model. No key required.
- **OpenAI-compatible API**, **MCP server**, and a **CLI** (incl. a `layer0
  config` TUI for editing settings).
- **Optional API-key auth**, multi-database / multi-collection scoping.
- **Self-update** from GitHub releases (`layer0 update`), configurable.
- Single SQLite database — no external services.

## Architecture

```
layer0/
  crates/
    layer0-core/    DB, chunking, embeddings, sqlite-vec, graph, RAG, LLM client, installer, updater
    layer0-server/  HTTP API server (OpenAI-compatible) + auth + bootstrap
    layer0-cli/     CLI (layer0 binary)
    layer0-mcp/     MCP server for Claude Code and other agents
  skills/              agentskills.io skills (installable via `npx skills`)
  .github/workflows/   CI (all platforms) + release (builds + GitHub Release)
```

### How chat is resolved

1. **ACP client** — *planned*: an editor/agent drives generation over the Agent
   Client Protocol (no model needed locally).
2. **Remote backend** — used only when a key is available (e.g.
   `ANTHROPIC_API_KEY`). Defaults to Claude via Anthropic's OpenAI-compatible
   endpoint. Any OpenAI-compatible server works.
3. **Local gemma fallback** — when no key is set, a local gemma model served by
   the llama.cpp sidecar handles chat, fully offline.

Embeddings are always local (nomic via the sidecar), unless you point
`[llm].base_url` at a remote embeddings endpoint — in which case the sidecar is
skipped automatically.

## Quick start

### 1. Build (or grab a release)

```sh
cargo build --release
# binaries: target/release/{layer0, layer0-server, layer0-mcp}
```

Requires Rust stable + a C toolchain (MSVC on Windows, gcc/clang elsewhere).
SQLite is bundled. Prebuilt archives are on the GitHub Releases page, named
`layer0-<target-triple>.{zip,tar.gz}` and containing all three binaries.

### 2. Initialize

```sh
layer0 init
```

Writes `~/.layer0/config.toml`, creates data dirs, and generates
`.claude/mcp.json` + `.cursor/mcp.json` in the current directory.

### 3. Serve (frictionless)

```sh
layer0 serve
```

On first run this installs llama.cpp, downloads the default embedding model
(`nomic-embed-text-v1.5`) and — if no chat key is set — the local chat model
(`gemma-4-E4B-it`), starts the sidecar(s), and serves on
`http://127.0.0.1:8080`. To use Claude instead of local gemma, set
`ANTHROPIC_API_KEY` before serving.

### 4. Use it

```sh
layer0 store "layer0 indexes chunks with sqlite-vec."
layer0 search "vector search"
layer0 ask "What does layer0 use for vector search?"
layer0 status
```

## Configuration

Global config lives at `~/.layer0/config.toml`. Edit it interactively with
`layer0 config` (a ratatui TUI) or by hand. The fully-commented template is at
`config/default.toml` in the repo.

**Environment overrides** use a `LAYER0__` prefix with double underscores for
nesting, e.g. `LAYER0__SERVER__PORT=9000` or `LAYER0__RAG__MODE=vector`.
`ANTHROPIC_API_KEY` is picked up automatically for `[chat].api_key`.

Priority (highest first): env vars → config file → built-in defaults.

### Example config

```toml
[server]
host = "127.0.0.1"
port = 8080
cors_origins = ["*"]
# api_key = "change-me"          # uncomment to require auth

[database]
max_connections = 5

[llm]
base_url = "http://127.0.0.1:8081"
embedding_model = "nomic-embed-text-v1.5"
timeout_secs = 120
context_length = 2048

[chat]
provider = "anthropic"
base_url = "https://api.anthropic.com"
model = "claude-haiku-4-5"
timeout_secs = 120

[embeddings]
dimensions = 768                  # must match the embedding model
batch_size = 16
search_limit = 1000

[chunking]
chunk_size = 512
chunk_overlap = 64

[rag]
mode = "hybrid"                   # hybrid | vector | graph
rerank = true
extract_graph = true

[installer]
llama_server_port = 8081
embedding_repo = "nomic-ai/nomic-embed-text-v1.5-GGUF"
embedding_file = "nomic-embed-text-v1.5.Q4_K_M.gguf"
chat_repo = "bartowski/google_gemma-4-E4B-it-GGUF"
chat_file = "google_gemma-4-E4B-it-Q4_K_M.gguf"
chat_server_port = 8082
auto_start = true

[update]
repo = "amajorai/layer0"
auto_check = true
auto_update = false
```

### `[server]`

| Key | Default | Description |
|-----|---------|-------------|
| `host` | `"127.0.0.1"` | Bind address. Use `"0.0.0.0"` to expose on the network. |
| `port` | `8080` | HTTP port. |
| `cors_origins` | `["*"]` | Allowed CORS origins. Restrict in production. |
| `api_key` | _(unset)_ | When set, all requests (except `GET /health`) must supply `X-API-Key: <key>` or `Authorization: Bearer <key>`. |

### `[database]`

| Key | Default | Description |
|-----|---------|-------------|
| `max_connections` | `5` | SQLite connection pool size. |

### `[llm]` — local embeddings sidecar

The local llama.cpp sidecar that serves embeddings (and optionally chat when no remote key is set). Uses the OpenAI wire format.

| Key | Default | Description |
|-----|---------|-------------|
| `base_url` | `"http://127.0.0.1:8081"` | Embeddings API endpoint. Point at any OpenAI-compatible server to skip the local sidecar. |
| `embedding_model` | `"nomic-embed-text-v1.5"` | Model name sent in embedding requests. |
| `rerank_model` | _(unset)_ | Optional reranking model. When set, reranking uses this model instead of the embedding model. |
| `api_key` | _(unset)_ | API key for the embeddings endpoint, if required. |
| `timeout_secs` | `120` | Per-request timeout. |
| `context_length` | `2048` | Model context window size (tokens). |

### `[chat]` — remote chat backend

Used for RAG answers and graph extraction. Resolution order: ACP client *(planned)* → this remote backend (only when `api_key` / `ANTHROPIC_API_KEY` is available) → local gemma sidecar fallback.

| Key | Default | Description |
|-----|---------|-------------|
| `provider` | `"anthropic"` | Provider label (informational). |
| `base_url` | `"https://api.anthropic.com"` | Chat API base URL. Any OpenAI-compatible endpoint works. |
| `model` | `"claude-haiku-4-5"` | Model identifier passed in requests. |
| `api_key` | _(unset)_ | API key. Falls back to `ANTHROPIC_API_KEY` env var. When absent, chat falls back to the local gemma model. |
| `timeout_secs` | `120` | Per-request timeout. |

### `[embeddings]`

| Key | Default | Description |
|-----|---------|-------------|
| `dimensions` | `768` | Embedding vector size. **Must match the model** (`nomic-embed-text-v1.5` = 768). Changing this requires re-embedding all documents. |
| `batch_size` | `16` | Number of chunks embedded per request to the sidecar. |
| `search_limit` | `1000` | Maximum candidate results from the vector index before reranking/filtering. |

### `[chunking]`

| Key | Default | Description |
|-----|---------|-------------|
| `chunk_size` | `512` | Target chunk size in tokens. Larger chunks give more context per result; smaller chunks give tighter matches. |
| `chunk_overlap` | `64` | Overlap between adjacent chunks in tokens. Prevents boundary splits from losing context. |

### `[rag]`

| Key | Default | Description |
|-----|---------|-------------|
| `mode` | `"hybrid"` | Retrieval strategy. `hybrid` — vector ANN + knowledge graph, fused with RRF, then reranked. `vector` — semantic similarity only. `graph` — graph-led traversal seeded by vector results. |
| `rerank` | `true` | Apply a reranking pass to the final result set before returning. |
| `extract_graph` | `true` | Extract entities and relationships at ingest time to populate the knowledge graph. Auto-skipped when `mode = "vector"`. |

### `[installer]` — model & sidecar management

Controls the auto-managed llama.cpp installation and model downloads. All paths default under `~/.layer0/`.

| Key | Default | Description |
|-----|---------|-------------|
| `llama_server_port` | `8081` | Port the embedding llama-server listens on. Must match `[llm].base_url`. |
| `embedding_repo` | `"nomic-ai/nomic-embed-text-v1.5-GGUF"` | Hugging Face repo for the embedding model. |
| `embedding_file` | `"nomic-embed-text-v1.5.Q4_K_M.gguf"` | GGUF filename to download from `embedding_repo`. |
| `chat_repo` | `"bartowski/google_gemma-4-E4B-it-GGUF"` | Hugging Face repo for the local chat fallback model. For lighter hardware try `bartowski/google_gemma-4-E2B-it-GGUF`. |
| `chat_file` | `"google_gemma-4-E4B-it-Q4_K_M.gguf"` | GGUF filename to download from `chat_repo`. |
| `chat_server_port` | `8082` | Port the chat llama-server listens on. |
| `hf_token` | _(unset)_ | Hugging Face token for downloading gated models. |
| `auto_start` | `true` | Install llama.cpp, download models, and start sidecar(s) automatically on `layer0 serve`. |

### `[update]`

| Key | Default | Description |
|-----|---------|-------------|
| `repo` | `"amajorai/layer0"` | GitHub repo (`owner/name`) to pull releases from. |
| `auto_check` | `true` | Check for a newer release at startup and log if one is available. |
| `auto_update` | `false` | Automatically download and apply the latest release at startup (takes effect on next restart). |

## HTTP API

Base: `http://localhost:8080`. Highlights:

```
POST /v1/documents              store (auto-chunked + embedded)
POST /v1/search                 hybrid search (vector + BM25 [+ graph] [+ rerank])
POST /v1/rag                    answer grounded in memory
GET/DELETE /v1/documents[/:id]  list / fetch / delete
/v1/graph/...                   nodes, edges, BFS query
POST /v1/embeddings             OpenAI-compatible
POST /v1/chat/completions       OpenAI-compatible (routes to the chat backend)
/v1/db/:database/:collection/... scoped variants of the above
GET  /v1/stats                  counts
GET  /health                    liveness (no auth)
```

## MCP

```sh
layer0 mcp        # stdio JSON-RPC 2.0
```

Tools: `store_memory`, `search_memory`, `rag_query`, `get_document`,
`delete_memory`, `graph_query`, `memory_stats`. `layer0 init` writes the
client config; or add it manually to `.claude/mcp.json` / `.cursor/mcp.json`.

## Skills

`skills/` contains [agentskills.io](https://agentskills.io)-compatible skills
(`layer0-setup`, `layer0-memory`) — install them into any skills-aware
agent (e.g. `npx skills add <repo>`).

## Updating

```sh
layer0 update     # self-update from the latest GitHub release
```

`[update].auto_check` logs when a newer release exists on `serve`;
`[update].auto_update` applies it on startup (takes effect on next restart).

## Releases & CI

GitHub Actions build and test on Linux/macOS/Windows. Pushing a `v*.*.*` tag
builds release binaries for five targets (linux x64/arm64, macOS x64/arm64,
windows x64) and publishes them to a GitHub Release. Release asset names embed
the Rust target triple, which the self-updater matches.

## Database schema (single SQLite file)

| Table | Contents |
|-------|----------|
| `documents` | Source documents + metadata (FTS5 mirror in `documents_fts`) |
| `chunks` | Per-document chunks (the retrieval unit) |
| `vec_chunks` | sqlite-vec `vec0` cosine index over chunk embeddings |
| `graph_nodes` / `graph_edges` | Knowledge graph |
| `databases` / `collections` | Named scopes |
| `models` | Model registry |

## License

MIT

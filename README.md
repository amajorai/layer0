# layerzero

A self-hostable RAG / long-term-memory server for AI agents, written in Rust.

It stores documents, splits them into overlapping chunks, embeds each chunk
locally, and indexes them with **sqlite-vec** (vector ANN) + **FTS5** (BM25
keyword) for hybrid retrieval — then answers questions with RAG. Everything lives
in a single SQLite file. Plug it into any agent via an OpenAI-compatible HTTP
API, an MCP server (Claude Code, Cursor, …), or the CLI.

It is **frictionless**: `layerzero serve` auto-installs llama.cpp, auto-downloads
the default models, and starts the local sidecar for you. It runs **fully
offline on any computer** out of the box.

---

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
- **OpenAI-compatible API**, **MCP server**, and a **CLI** (incl. a `layerzero
  config` TUI for editing settings).
- **Optional API-key auth**, multi-database / multi-collection scoping.
- **Self-update** from GitHub releases (`layerzero update`), configurable.
- Single SQLite database — no external services.

---

## Architecture

```
layerzero/
  crates/
    layerzero-core/    DB, chunking, embeddings, sqlite-vec, graph, RAG, LLM client, installer, updater
    layerzero-server/  HTTP API server (OpenAI-compatible) + auth + bootstrap
    layerzero-cli/     CLI (layerzero binary)
    layerzero-mcp/     MCP server for Claude Code and other agents
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

---

## Quick start

### 1. Build (or grab a release)

```sh
cargo build --release
# binaries: target/release/{layerzero, layerzero-server, layerzero-mcp}
```

Requires Rust stable + a C toolchain (MSVC on Windows, gcc/clang elsewhere).
SQLite is bundled. Prebuilt archives are on the GitHub Releases page, named
`layerzero-<target-triple>.{zip,tar.gz}` and containing all three binaries.

### 2. Initialize

```sh
layerzero init
```

Writes `~/.layerzero/config.toml`, creates data dirs, and generates
`.claude/mcp.json` + `.cursor/mcp.json` in the current directory.

### 3. Serve (frictionless)

```sh
layerzero serve
```

On first run this installs llama.cpp, downloads the default embedding model
(`nomic-embed-text-v1.5`) and — if no chat key is set — the local chat model
(`gemma-4-E4B-it`), starts the sidecar(s), and serves on
`http://127.0.0.1:8080`. To use Claude instead of local gemma, set
`ANTHROPIC_API_KEY` before serving.

### 4. Use it

```sh
layerzero store "layerzero indexes chunks with sqlite-vec."
layerzero search "vector search"
layerzero ask "What does layerzero use for vector search?"
layerzero status
```

---

## Configuration

Global config: `~/.layerzero/config.toml` (see `config/default.toml` for the
fully-commented template). Environment overrides use the `LAYERZERO__` prefix
(double underscore separates nested keys, e.g. `LAYERZERO__SERVER__PORT`).

Edit it interactively with `layerzero config` (a ratatui TUI) or by hand.

Key sections: `[server]` (host/port/cors, optional `api_key`), `[llm]` (local
embeddings backend), `[chat]` (remote chat backend), `[embeddings]` (dimensions
— must match the model, nomic = 768), `[chunking]` (chunk_size/overlap),
`[rag]` (`mode` = hybrid/vector/graph, `rerank`, `extract_graph`), `[installer]`
(model repos/files, ports, `auto_start`), `[update]` (repo, auto_check,
auto_update).

### Auth

Set `[server].api_key` to require `X-API-Key` (or `Authorization: Bearer`) on
every request except `/health`. Unset = open (local default).

---

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

---

## MCP

```sh
layerzero mcp        # stdio JSON-RPC 2.0
```

Tools: `store_memory`, `search_memory`, `rag_query`, `get_document`,
`delete_memory`, `graph_query`, `memory_stats`. `layerzero init` writes the
client config; or add it manually to `.claude/mcp.json` / `.cursor/mcp.json`.

---

## Skills

`skills/` contains [agentskills.io](https://agentskills.io)-compatible skills
(`layerzero-setup`, `layerzero-memory`) — install them into any skills-aware
agent (e.g. `npx skills add <repo>`).

---

## Updating

```sh
layerzero update     # self-update from the latest GitHub release
```

`[update].auto_check` logs when a newer release exists on `serve`;
`[update].auto_update` applies it on startup (takes effect on next restart).

---

## Releases & CI

GitHub Actions build and test on Linux/macOS/Windows. Pushing a `v*.*.*` tag
builds release binaries for five targets (linux x64/arm64, macOS x64/arm64,
windows x64) and publishes them to a GitHub Release. Release asset names embed
the Rust target triple, which the self-updater matches.

---

## Database schema (single SQLite file)

| Table | Contents |
|-------|----------|
| `documents` | Source documents + metadata (FTS5 mirror in `documents_fts`) |
| `chunks` | Per-document chunks (the retrieval unit) |
| `vec_chunks` | sqlite-vec `vec0` cosine index over chunk embeddings |
| `graph_nodes` / `graph_edges` | Knowledge graph |
| `databases` / `collections` | Named scopes |
| `models` | Model registry |

---

## License

MIT

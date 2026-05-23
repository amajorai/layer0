---
name: layerzero-setup
description: >-
  Install, configure, and run layerzero, a self-hostable Rust RAG / long-term
  memory server for AI agents. Use this when setting up layerzero from scratch:
  building from source or installing prebuilt binaries, running `layerzero init`,
  configuring the Anthropic / Claude chat key, starting the local embeddings
  sidecar with `layerzero serve`, verifying health, storing and searching
  memories, connecting agents over MCP, and self-updating. Triggers on: install
  layerzero, set up layerzero, layerzero serve, layerzero init, configure RAG
  memory server, start layerzero, llama.cpp embeddings sidecar, OpenAI-compatible
  memory API, ANTHROPIC_API_KEY, MCP memory server setup.
license: MIT
metadata:
  version: "0.0.1"
compatibility: >-
  To build from source: Rust stable toolchain + a C toolchain (MSVC on Windows,
  gcc/clang on Linux/macOS). SQLite is bundled (no system SQLite required).
  First run needs internet access to auto-install llama.cpp and download the
  default embedding model. Claude chat (the default) needs an ANTHROPIC_API_KEY;
  embeddings run fully locally and need no key.
---

# layerzero setup

layerzero is a self-hostable RAG / long-term-memory server for AI agents,
written in Rust. It stores documents, chunks them, embeds each chunk locally,
indexes with sqlite-vec (vector) + FTS5 (keyword), and answers questions via
RAG. It exposes an OpenAI-compatible HTTP API, an MCP server (for Claude Code,
Cursor, etc.), and a CLI. Embeddings always run locally via a llama.cpp sidecar.
Chat/generation resolves in this order: an ACP client (planned) → a remote
backend such as Claude (used only when an API key like `ANTHROPIC_API_KEY` is
set) → a local gemma model served by the sidecar. So out of the box it runs
fully offline on any computer; setting `ANTHROPIC_API_KEY` upgrades chat to Claude.

This skill walks through installing and running it from scratch.

## 1. Install

You get three binaries: `layerzero` (CLI), `layerzero-server` (HTTP API), and
`layerzero-mcp` (MCP stdio server).

### Option A — build from source

Requires Rust stable + a C toolchain (MSVC on Windows, gcc/clang on
Linux/macOS). SQLite is bundled, so no system SQLite is needed.

```sh
# from the repo root
cargo build --release
```

Binaries land in `target/release/`:

- `target/release/layerzero`
- `target/release/layerzero-server`
- `target/release/layerzero-mcp`

Put `target/release/` on your `PATH` (or copy the binaries somewhere on PATH).

### Option B — prebuilt archive (GitHub Releases)

Download the archive for your platform from the project's GitHub Releases page.
Each archive is named `layerzero-<target-triple>.zip` (Windows) or
`layerzero-<target-triple>.tar.gz` (Linux/macOS) and contains all three
binaries. Extract it and put the binaries on your `PATH`.

## 2. First-time setup

### Initialize config and data dirs

```sh
layerzero init
```

This writes `~/.layerzero/config.toml` and creates the data directories.

### Configure chat (offline by default)

No configuration is required: with no API key set, chat runs fully offline on a
local gemma model (auto-downloaded on first `serve`). Embeddings are always local
and need **no** API key.

To upgrade chat to Claude, set the `ANTHROPIC_API_KEY` environment variable:

```sh
# macOS / Linux
export ANTHROPIC_API_KEY="sk-ant-..."

# Windows PowerShell
$env:ANTHROPIC_API_KEY = "sk-ant-..."
```

To use a different remote OpenAI-compatible chat backend, point the `[chat]`
section of `~/.layerzero/config.toml` at it (`base_url`, `model`, and provide a
key). To change the local fallback model, edit `chat_repo`/`chat_file` under
`[installer]`.

### Start the server

```sh
layerzero serve
```

This is frictionless. On first run it automatically:

1. Installs llama.cpp.
2. Downloads the default embedding model (`nomic-embed-text-v1.5`).
3. Starts the local embeddings sidecar.
4. Serves the API on `http://127.0.0.1:8080`.

Advanced users can disable this auto-bootstrap by setting `auto_start = false`
under the `[installer]` section of the config (then manage llama.cpp and the
model yourself).

## 3. Verify

```sh
layerzero status
curl http://127.0.0.1:8080/health
```

`layerzero status` reports whether the server and embeddings sidecar are up;
`/health` should return an OK response.

## 4. Use it

CLI quickstart:

```sh
layerzero store "Project layerzero uses sqlite-vec for vector search."
layerzero search "vector search"
layerzero ask "What does layerzero use for vector search?"
```

The HTTP API base is `http://127.0.0.1:8080`, with OpenAI-compatible endpoints
under `/v1/...` (e.g. `/v1/documents`, `/v1/search`, `/v1/rag`). See the
`layerzero-memory` skill for using these as an agent's long-term memory.

## 5. Connect agents via MCP

Run the MCP server (stdio JSON-RPC):

```sh
layerzero mcp
```

Register it with your agent. For Claude Code add it to `.claude/mcp.json`; for
Cursor add it to `.cursor/mcp.json`. The `layerzero init` command can generate
these config files for you. A typical entry:

```json
{
  "mcpServers": {
    "layerzero": {
      "command": "layerzero",
      "args": ["mcp"]
    }
  }
}
```

## 6. Keep it updated

```sh
layerzero update
```

This self-updates from the latest GitHub release. Auto-update behavior is
configurable in the `[update]` section of `~/.layerzero/config.toml`.

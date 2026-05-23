---
name: layer0-memory
description: >-
  Use a running layer0 server as long-term, persistent memory for an AI
  agent. Use this whenever you need to remember something for later, recall past
  context, or answer "what do I know about X" across sessions. Covers when to
  store vs search, the MCP tools (store_memory, search_memory, rag_query,
  get_document, delete_memory, graph_query, memory_stats) and the equivalent
  HTTP endpoints (POST /v1/documents, POST /v1/search, POST /v1/rag), plus how
  to scope memory with databases/collections. Triggers on: remember this, recall,
  what do I know about, save to memory, long-term memory, persistent context,
  store a memory, search my memory, RAG query, retrieve past notes, forget /
  delete a memory, memory stats.
license: MIT
metadata:
  version: "0.0.1"
compatibility: >-
  Requires a running layer0 server (see the layer0-setup skill). MCP tools
  require the layer0 MCP server connected to your agent; HTTP calls require
  the API reachable at http://127.0.0.1:8080 (default).
---

# Using layer0 as long-term memory

layer0 is a self-hostable RAG memory server. Once it is running (see the
`layer0-setup` skill), use it to persist knowledge across sessions and to
recall it later via semantic + keyword search and RAG.

## When to store vs. search

**Store** a memory when the user states a durable fact, preference, decision,
identifier, or anything worth recalling later — names, project details, API
keys' locations (not the secrets themselves), conventions, "remember that...",
"from now on...", recurring context. Store proactively; storage is cheap.

**Search / recall** when the user asks "what do I know about X", "what did we
decide", "recall...", references something from a past session, or when you lack
context that may already be stored. Prefer `rag_query` when you want a
synthesized answer; prefer `search_memory` when you want raw matching chunks to
reason over yourself.

Rule of thumb: read (search) before you assume; write (store) when you learn
something durable.

## MCP tools

If the layer0 MCP server is connected to your agent, these tools are
available:

- `store_memory` — add a document/note to memory. Args typically include the
  `content` text and optionally `database`/`collection` and `metadata`.
- `search_memory` — hybrid (vector + keyword) search; returns matching chunks.
  Args: `query`, optional `top_k`, `database`/`collection`.
- `rag_query` — retrieve relevant context and return a synthesized answer.
  Args: `query`, optional scope.
- `get_document` — fetch a stored document by its id.
- `delete_memory` — remove a document by id ("forget this").
- `graph_query` — query relationships/links across stored memories.
- `memory_stats` — counts and stats about what is stored.

### Example MCP calls

Store:

```json
{
  "name": "store_memory",
  "arguments": {
    "content": "The user prefers Bun over npm for all JS/TS projects.",
    "collection": "preferences"
  }
}
```

Search:

```json
{
  "name": "search_memory",
  "arguments": { "query": "which package manager does the user prefer", "top_k": 5 }
}
```

RAG answer:

```json
{
  "name": "rag_query",
  "arguments": { "query": "What are the user's tooling preferences?" }
}
```

Delete:

```json
{ "name": "delete_memory", "arguments": { "id": "doc_123" } }
```

## HTTP endpoints

The same operations are available over the OpenAI-compatible HTTP API (default
base `http://127.0.0.1:8080`).

Store a document — `POST /v1/documents`:

```sh
curl -X POST http://127.0.0.1:8080/v1/documents \
  -H "Content-Type: application/json" \
  -d '{"content": "The user prefers Bun over npm.", "collection": "preferences"}'
```

Search — `POST /v1/search`:

```sh
curl -X POST http://127.0.0.1:8080/v1/search \
  -H "Content-Type: application/json" \
  -d '{"query": "package manager preference", "top_k": 5}'
```

RAG answer — `POST /v1/rag`:

```sh
curl -X POST http://127.0.0.1:8080/v1/rag \
  -H "Content-Type: application/json" \
  -d '{"query": "What are the user'\''s tooling preferences?"}'
```

## Scoping memory: databases and collections

layer0 scopes memory with **databases** and **collections**. Use them to keep
contexts separate so searches stay relevant:

- A **database** is a top-level store (e.g. per machine, per user, or per major
  domain).
- A **collection** groups related memories within a database (e.g.
  `preferences`, `project-layer0`, `meeting-notes`).

Pass `database` and/or `collection` on store, search, and RAG calls. Store into
the most specific collection that fits, and search the matching scope. Omitting
them uses the configured defaults. Keeping per-project or per-topic collections
prevents cross-talk and improves retrieval quality.

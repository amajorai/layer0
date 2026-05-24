---
name: layer0-memory
description: >-
  Use a running layer0 server as long-term, persistent memory for an AI
  agent. Use this whenever you need to remember something for later, recall past
  context, or answer "what do I know about X" across sessions. Covers when to
  store vs search, the 13 MCP tools (store_memory, search_memory, rag_query,
  get_document, delete_memory, graph_query, memory_stats, list_databases,
  create_database, delete_database, list_collections, create_collection,
  delete_collection) and the equivalent HTTP endpoints, plus how to scope memory
  with databases and collections. Each named database gets its own isolated
  SQLite file. Triggers on: remember this, recall, what do I know about, save to
  memory, long-term memory, persistent context, store a memory, search my memory,
  RAG query, retrieve past notes, forget / delete a memory, memory stats,
  create database, list databases, manage collections.
license: MIT
metadata:
  version: "0.2.0"
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

## Databases and collections

layer0 organises memory in two levels:

- **Database** — top-level store; each named database gets its own isolated
  SQLite file at `~/.layer0/databases/<name>.db`. The `default` database lives
  at `~/.layer0/layer0.db` and is backward-compatible with existing data.
- **Collection** — named sub-scope within a database (e.g. `preferences`,
  `project-notes`, `meeting-notes`). Collections keep related memories together
  and prevent cross-talk when searching.

Use a separate database per major domain (e.g. per project or per user). Use
collections within a database to organise by topic. Omitting `database` or
`collection` on any call routes to the defaults.

Database name rules: letters, digits, `_`, `-`, `.` only; max 64 characters;
must not be an all-dots name (`.`, `..`); must not be a Windows reserved name
(`NUL`, `CON`, `PRN`, `AUX`, `COM1`–`COM9`, `LPT1`–`LPT9`).

## MCP tools

If the layer0 MCP server is connected to your agent, these 13 tools are
available:

### Memory tools

- `store_memory` — add a document/note to memory. Args: `content`, optional
  `database`, `collection`, `metadata`.
- `search_memory` — hybrid (vector + keyword) search; returns matching chunks.
  Args: `query`, optional `top_k`, `database`, `collection`.
- `rag_query` — retrieve relevant context and return a synthesized answer.
  Args: `query`, optional scope.
- `get_document` — fetch a stored document by its id.
- `delete_memory` — remove a document by id ("forget this").
- `graph_query` — query relationships/links across stored memories.
- `memory_stats` — counts and stats about what is stored.

### Database management tools

- `list_databases` — list all named databases.
- `create_database` — create a new database (and its `.db` file). Args: `name`,
  optional `description`.
- `delete_database` — permanently delete a database and its `.db` file. Args:
  `name`.

### Collection management tools

- `list_collections` — list collections in a database. Args: `database`.
- `create_collection` — create a collection within a database. Args: `database`,
  `name`, optional `description`.
- `delete_collection` — delete a collection. Args: `database`, `name`.

### Example MCP calls

Store into a named database and collection:

```json
{
  "name": "store_memory",
  "arguments": {
    "content": "The user prefers Bun over npm for all JS/TS projects.",
    "database": "work",
    "collection": "preferences"
  }
}
```

Search within a scoped database:

```json
{
  "name": "search_memory",
  "arguments": {
    "query": "which package manager does the user prefer",
    "database": "work",
    "top_k": 5
  }
}
```

RAG answer:

```json
{
  "name": "rag_query",
  "arguments": { "query": "What are the user's tooling preferences?", "database": "work" }
}
```

Delete a memory:

```json
{ "name": "delete_memory", "arguments": { "id": "doc_123" } }
```

Create a database:

```json
{ "name": "create_database", "arguments": { "name": "myproject", "description": "Project notes" } }
```

List databases:

```json
{ "name": "list_databases", "arguments": {} }
```

Delete a database (irreversible — removes the `.db` file):

```json
{ "name": "delete_database", "arguments": { "name": "myproject" } }
```

Create a collection:

```json
{ "name": "create_collection", "arguments": { "database": "work", "name": "meeting-notes" } }
```

## HTTP endpoints

The same operations are available over the OpenAI-compatible HTTP API (default
base `http://127.0.0.1:8080`).

Store a document — `POST /v1/documents`:

```sh
curl -X POST http://127.0.0.1:8080/v1/documents \
  -H "Content-Type: application/json" \
  -d '{"content": "The user prefers Bun over npm.", "database": "work", "collection": "preferences"}'
```

Scoped store — `POST /v1/db/:database/:collection/documents`:

```sh
curl -X POST http://127.0.0.1:8080/v1/db/work/preferences/documents \
  -H "Content-Type: application/json" \
  -d '{"content": "The user prefers Bun over npm."}'
```

Search — `POST /v1/search`:

```sh
curl -X POST http://127.0.0.1:8080/v1/search \
  -H "Content-Type: application/json" \
  -d '{"query": "package manager preference", "database": "work", "top_k": 5}'
```

RAG answer — `POST /v1/rag`:

```sh
curl -X POST http://127.0.0.1:8080/v1/rag \
  -H "Content-Type: application/json" \
  -d '{"query": "What are the user'\''s tooling preferences?", "database": "work"}'
```

Database management:

```sh
# list databases
curl http://127.0.0.1:8080/v1/db

# create database
curl -X POST http://127.0.0.1:8080/v1/db \
  -H "Content-Type: application/json" \
  -d '{"name": "myproject"}'

# delete database (removes .db file)
curl -X DELETE http://127.0.0.1:8080/v1/db/myproject

# list collections in a database
curl http://127.0.0.1:8080/v1/db/myproject/collections

# create collection
curl -X POST http://127.0.0.1:8080/v1/db/myproject/collections \
  -H "Content-Type: application/json" \
  -d '{"name": "notes"}'
```

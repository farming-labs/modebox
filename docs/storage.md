# modebox storage interface

**Status:** draft v0.1.
- **Implemented and tested:** the in-process `Store` trait, the local / memory / `object_store` backends, mounts, staging and the `fs.*` API.
- **Proposed, not implemented:** the out-of-process driver protocol (§9).

## 1. Goal

Agent code should work with files the same way wherever they live: a local repo, scratch memory, S3, R2, MinIO, GCS, Azure, or something custom. It should also be impossible for that code to do anything the host didn't allow.

There are three layers, and only the bottom one knows about any particular storage system:

```
 sandboxed code     fs.list / get / head / put / delete / copy / move / grep / changes / commit / mounts
 ───────────────────────────────────────────────────────────────────────────────────────────────────
 namespace (Vfs)    mounts (prefix → store + mode) · deny rules · staging overlay · commit protocol
                    approvals · state sealed into continuation tokens and sessions
 ───────────────────────────────────────────────────────────────────────────────────────────────────
 Store              head · get · put · delete · list · copy        ← implement this for a new backend
                    LocalStore · MemoryStore · ObjectStoreAdapter (S3/R2/MinIO/GCS/Azure/HTTP) · yours
```

The bottom layer deliberately has **object-store semantics**, not filesystem semantics:
- Object stores are the least common denominator. A directory can emulate an object store cheaply, but the reverse (rename, directories, partial writes) is expensive or impossible.
- Versioned, conditional writes give us safe staging and conflict detection on every backend, without locks.

## 2. Object model

**Key.** A `/`-separated UTF-8 string, such as `src/app/main.ts`.
- Length is 1–1024 bytes.
- No leading `/`, no empty, `.` or `..` segments, and no backslashes or control characters.
- Keys are case-sensitive.
- These rules are the intersection of what S3, GCS, Azure and POSIX accept, so a key that's valid in one backend is valid in all of them.
- There are no directories; a "folder" is just a common key prefix.

**Object.** Bytes plus metadata:

| field | meaning |
|---|---|
| `size` | bytes |
| `modified_ms` | last modified time, if the backend knows it |
| `etag` | opaque **version token** for the current version |
| `content_type` | optional |

**Etag rules:**
- Two etags of the same key are equal exactly when they name the same version.
- Any change of content changes the etag.
- Whether rewriting identical bytes keeps the etag depends on the backend (`content_etags`).
- Code must treat etags as opaque.
- Native S3 etags are MD5s. Local files use weak tokens built from size, mtime and inode (`W/…`). Memory uses content hashes.

## 3. The `Store` operations

The Rust trait is in `crates/modebox-core/src/store/mod.rs`; the summary:

| op | semantics | errors |
|---|---|---|
| `head(key)` | metadata, or `None` if absent | |
| `get(key, {range?, if_match?})` | bytes (or a byte range) and metadata, or `None` | `PreconditionFailed` if `if_match` isn't current |
| `put(key, bytes, {precondition, content_type?})` | **atomic** whole-object write; returns the new metadata | `AlreadyExists` (with `IfNotExists`), `PreconditionFailed` (with `IfMatch`), `PermissionDenied` (read-only), `TooLarge` |
| `delete(key, if_match?)` | removes it; returns whether it existed | `PreconditionFailed` |
| `list({prefix, delimiter, cursor, limit})` | one page of objects sorted by key; with `delimiter`, sub-"folders" come back as `prefixes`; `cursor` when more follow | |
| `copy(from, to, opts)` | defaults to `get` + `put`; backends may copy server-side | as `put`, plus `NotFound` |

**Preconditions:** `None`, `IfMatch(etag)` (compare-and-swap), or `IfNotExists` (create).

**`get` ranges:**
- `end` is clamped to the object size.
- A range starting at or past the end returns an empty body.

**`list` rules:**
- The prefix is a raw string, as in S3: `src/ma` matches `src/main.ts`.
- Pagination visits every key exactly once, in order.

**Errors and their codes** (the code is what sandboxed code sees as `e.code`):

| error | code |
|---|---|
| `NotFound` | `ENOENT` |
| `AlreadyExists` | `EEXIST` |
| `PreconditionFailed` | `PRECONDITION_FAILED` |
| `PermissionDenied` | `EACCES` |
| `InvalidKey` | `EINVAL` |
| `TooLarge` | `EFBIG` |
| `Unsupported` | `ENOTSUP` |
| `QuotaExceeded` | `ENOSPC` |
| `Transient` (worth retrying) | `EAGAIN` |
| `Other` | `EIO` |

**Capabilities.** Backends report what they can do, so the layers above adapt instead of assuming:

```rust
Capabilities { writable, atomic_preconditions, atomic_create, range_reads,
               server_side_copy, consistent_list, content_etags, max_object_size }
```

`atomic_preconditions: false` doesn't mean preconditions are ignored. It means the backend checks and then writes, so a concurrent writer could slip in between. The contract still requires every precondition to be honored when there's no race. `atomic_create` covers just `IfNotExists`: a local directory creates with a hard link, so creates are atomic even though its `IfMatch` isn't. Single-use continuation tracking (`StoreContinuations`) needs it.

## 4. Guarantees, honestly

| backend | atomic puts | atomic preconditions | etags | notes |
|---|---|---|---|---|
| `MemoryStore` | yes | yes | content hash | reference implementation |
| `LocalStore` | yes (temp + rename) | `IfNotExists` yes (hard-link create); `IfMatch` check-then-rename | weak (size, mtime, inode) | path jail; symlinks can't escape; listing honors `.gitignore` and skips dotfiles (configurable) |
| `ObjectStoreAdapter` gs / az / memory | yes | yes | native | |
| `ObjectStoreAdapter` s3 | yes | if the endpoint supports conditional writes (S3, R2, MinIO do; some compatibles don't) | native | otherwise falls back to check-then-write |
| `ObjectStoreAdapter` http | read-mostly | no | server's `ETag` | |

What isn't guaranteed: **multi-object atomicity.** A commit applies objects one at a time. If it's interrupted, some objects are written and the rest stay staged and are reported. Per object it's all or nothing, and nothing ever overwrites a version the code didn't see.

## 5. Namespace: mounts and modes

A `Vfs` maps key prefixes to stores. The longest prefix wins, so `tmp/` shadows the root mount. Each mount has a mode:

| mode | reads | writes |
|---|---|---|
| `read-only` | ✓ | rejected with `EROFS` |
| `staged` | ✓ (staged versions first) | go into an in-memory overlay; written only by `fs.commit()` |
| `direct` | ✓ | go straight to the store (scratch space, output buckets) |

**Deny rules** are globs matched against full keys. Matching keys can't be read, listed or written. The defaults cover `.git/`, `.env*`, `*.pem`, `*.key`, SSH keys, `.npmrc` and `.netrc`.

**Staging and commit:**
- The first time code reads or writes a key on a staged mount, the Vfs records what the store held: whether it existed, and its etag, or a content hash when the store has no etags.
- Staged writes are content-addressed blobs.
- `fs.changes()` diffs them against the store's current content.
- `fs.commit()` writes each change with a precondition built from what was recorded: `IfMatch(etag)` if the key existed, `IfNotExists` if it didn't. A failed precondition becomes a **conflict**, never an overwrite.
- By default, commit requires approval. The approval prompt lists `A`/`M`/`D` per key.
- Uncommitted changes are capped at 64 MiB (`max_staged_bytes`). Past that, `fs.put` fails with `ENOSPC` until the code commits or discards. Overwritten and discarded versions are freed right away. The in-memory `tmp/` scratch mount is capped at 64 MiB too.

**State travels:**
- Staged writes and the recorded versions are sealed into continuation tokens (encrypted) and saved sessions.
- A run that pauses on `fs.commit()` in one process and is approved in another commits exactly what was staged.

## 6. The sandbox API

TypeScript declarations are generated for the model. A summary:

```ts
fs.list({ prefix?, glob?, recursive? = true, limit? }) → { objects: [{ key, size, modified: Date, etag, status? }], prefixes: string[], truncated }
fs.get(key | { key, as?: 'text'|'bytes'|'json', range?: { start, end } }) → string | Uint8Array | any | null
fs.head(key) → { key, size, modified, etag, status? } | null
fs.put({ key, body: string | Uint8Array | any, ifMatch? }) → { key, size, etag }
fs.delete(key) → boolean          fs.copy({ from, to })      fs.move({ from, to })   // across mounts too
fs.grep({ pattern, prefix?, glob?, literal?, ignoreCase?, maxResults? }) → [{ key, line, text }]
fs.changes() → [{ key, status, diff }]      fs.discard({ prefix? }) → number
fs.commit({ message? }) → { applied, conflicts }        fs.mounts() → [{ prefix, store, mode, capabilities }]
```

- **`get` encoding:** text if the bytes are valid UTF-8, otherwise `Uint8Array`.
- **`put` encoding:** a plain value is stored as pretty JSON.
- **`status`:** marks staged objects in listings and `head` results (`added` / `modified`).

## 7. Configuration (`modebox.json`)

```json
{
  "fs": {
    "readOnly": false,
    "mounts": {
      "datasets/": { "url": "s3://acme-data/agent", "readOnly": true,
                     "options": { "region": "auto", "endpoint": "https://<acct>.r2.cloudflarestorage.com",
                                  "access_key_id": "$R2_KEY_ID", "secret_access_key": "$R2_SECRET" } },
      "reports/":  { "url": "gs://acme-reports/out", "mode": "direct" },
      "docs/":     { "dir": "../handbook", "readOnly": true },
      "scratch/":  { "memory": true }
    },
    "deny": ["**/.git/**", "**/.env*", "secrets/**"],
    "commitApproval": true
  }
}
```

- **The workspace root** is always mounted at `""` (staged, unless `readOnly`), and `tmp/` is in-memory scratch space.
- **Option values** like `"$NAME"` are read from the environment.
- **Credentials** live only in the host process. Sandboxed code has no network and no environment, and only sees keys and bytes.

## 8. Writing a backend in Rust

Implement six methods:

```rust
impl Store for MyStore {
    fn describe(&self) -> String { "mydb://…".into() }
    fn capabilities(&self) -> Capabilities { … }
    fn head<'a>(&'a self, key: &'a Key) -> StoreFuture<'a, Option<Meta>> { … }
    fn get<'a>(&'a self, key: &'a Key, opts: GetOptions) -> StoreFuture<'a, Option<Object>> { … }
    fn put<'a>(&'a self, key: &'a Key, body: Bytes, opts: PutOptions) -> StoreFuture<'a, Meta> { … }
    fn delete<'a>(&'a self, key: &'a Key, if_match: Option<Etag>) -> StoreFuture<'a, bool> { … }
    fn list<'a>(&'a self, opts: ListOptions) -> StoreFuture<'a, ListPage> { … }
}
```

Then prove it with the executable contract:

```rust
modebox_core::store::conformance::run(&MyStore::new()).await?;                 // writable stores
modebox_core::store::conformance::run_read_only(&store, &[("k", b"v")]).await?; // read-only stores
```

The suite checks:
- absent keys;
- round-trips of binary, empty and Unicode-keyed objects;
- etag changes;
- ranges;
- all three preconditions on `put`, `get` and `delete`;
- copy;
- raw-prefix listing, delimiters, pagination;
- visibility of deletes.

Today `MemoryStore`, `LocalStore`, and `ObjectStoreAdapter` over `object_store`'s memory and filesystem drivers all pass it.

## 9. Out-of-process drivers (proposed)

Backends in other languages would be separate processes speaking **JSON-RPC 2.0 over stdio**, using the same newline-delimited framing as MCP's stdio transport. Examples: a Python driver for a database, Google Drive, Notion, or a company's internal blob service.

The host spawns the driver, calls `store/initialize`, and treats it as a `Store`. The same conformance suite runs against it.

| method | params | result |
|---|---|---|
| `store/initialize` | `{ protocolVersion: "0.1", options: {…} }` | `{ describe, capabilities }` |
| `store/head` | `{ key }` | `Meta \| null` |
| `store/get` | `{ key, range?: [start, end], ifMatch? }` | `{ meta, body } \| null` |
| `store/put` | `{ key, body, precondition: { ifMatch } \| { ifNotExists: true } \| null, contentType? }` | `Meta` |
| `store/delete` | `{ key, ifMatch? }` | `boolean` |
| `store/list` | `{ prefix, delimiter, cursor?, limit? }` | `{ objects: Meta[], prefixes, cursor? }` |
| `store/copy` (optional) | `{ from, to, precondition }` | `Meta` |

- **Shapes:**
  - `Meta` is `{ key, size, modifiedMs?, etag?, contentType? }`.
  - `body` is base64. A v0.2 would add chunked bodies for large objects (`store/getChunk`, `store/putChunk` with an upload id).
- **Errors:** `{ code: -32000, message, data: { kind: "NotFound" | "AlreadyExists" | "PreconditionFailed" | "PermissionDenied" | "InvalidKey" | "TooLarge" | "Unsupported" | "Transient" | "Other" } }`.
- **Example exchange:**
  ```
  → {"jsonrpc":"2.0","id":7,"method":"store/put","params":{"key":"notes/a.md","body":"aGk=","precondition":{"ifNotExists":true}}}
  ← {"jsonrpc":"2.0","id":7,"error":{"code":-32000,"message":"notes/a.md: already exists","data":{"kind":"AlreadyExists"}}}
  ```

## 10. Not in v0.1

- **Streaming / multipart** for objects larger than memory. `get` takes byte ranges today, but bodies are buffered.
- **Watching for changes**, i.e. notifications.
- **Multi-object atomic commits.** This would need a manifest-based or transactional backend.
- **Object versions and history**, such as S3 versioning or git trees as a read-only backend.
- **Presigned URLs** for handing large data to tools without passing it through the sandbox.
- **A search index:** `fs.grep` scans.
- **`sh()` seeing staged writes.** The OS-sandboxed shell runs on the real folder.
- **Testing against live S3, R2, MinIO, GCS and Azure.** The adapter is exercised through `object_store`'s in-memory and filesystem drivers only, and conditional-write support varies by S3-compatible provider.

# TSG requirements and the Cx roadmaps — what can be reconciled, and what is missing

- Date: 2026-11-18
- Asked for by: owner decision 7 of the second review ("inspect
  `COMMENTERTHE9/TSG-TeamComms` read-only; reconcile the historical Cx master
  roadmap with the current repository; identify what is missing")
- Nothing in any TSG repository was modified. No graphics API, engine
  technology or language feature is selected here.

## 1. Summary

1. **`TSG-TeamComms` is a team status board, not an engine specification.** It
   records, as of early 2025, three teams integrating a Go server, a C++ client
   and a Rust asset system. It does not mention Cx.
2. **The "older Cx master roadmap" is not in the Cx repository.** Three
   historical roadmaps are. None of them contains Vulkan, DirectX 12, math
   types, a renderer, physics or the NOD Protocol. Those appear for the first
   time in the living roadmap, in one commit on 2026-07-05.
3. **So the reconciliation asked for can be done only in part.** What the
   repository's own roadmaps committed to is compared with the code, item by
   item, in the companion table (190 rows). What TSG needs from Cx cannot be
   derived from either source; §5 lists the documents that would be needed.

## 2. What was read

| Source | What it is | Read |
|---|---|---|
| `COMMENTERTHE9/TSG-TeamComms` @ `085a90b` (2025-06-02) | 13 files: a README and status/update/request notes for three teams | all 13 |
| `docs/frontend/ROADMAP.md` (v5.0, 2026-04-25) | historical front-end roadmap | all |
| `docs/ROADMAP_v3.1.md` (2026-03-16) | historical roadmap | all |
| `docs/backend/cx_backend_roadmap_v3_1.md` (v4.4, 2026-05-12) | historical back-end roadmap | all |
| `docment/ROADMAP.md` | the living roadmap | all |
| `docs/post_0_1/gene_phen_design.md` | design document that cites a "merged master roadmap" | the citation |

**Not available**, and not claimed: the TSG engine repository, the Go
orchestrator repository, the C++ client repository, any document describing how
TSG would use Cx, and any earlier AI conversation about either project.

## 3. `TSG-TeamComms`

### 3.1 What it describes

- **A Go "Universe Orchestrator".** An HTTP server with 37 endpoints, JWT
  authentication, a basic WebSocket channel, and PostgreSQL persistence for
  players, ships, inventory, factions, market, contracts and combat logs.
- **A C++ game client** acting as a "physics shard": it talks to the
  orchestrator over WinHTTP and sends heartbeats.
- **A Rust virtual file system (VFS)**, at `C:/Projects/TSG-Engine/vfs` on the
  team's machines, exposing 26 C-ABI functions. Among them: `vfs_init_gpu` taking
  allocation and free callbacks (described as Vulkan callbacks), `vfs_request`,
  `vfs_is_resident`, `vfs_get_ptr`, priority loading, and `vfs_get_stats()`
  returning a struct. It does LRU paging of GPU memory, keeps a ghost cache, and
  uses worker threads. It builds with both MSVC and MinGW.

### 3.2 What it does not contain

- Any mention of Cx.
- An engine architecture, a renderer design, a physics design, or a protocol
  definition.
- The documents it refers to: `shared-docs/INTEGRATION_GUIDE.md` and
  `shared-docs/architecture/vfs-design.md` are named and absent.

### 3.3 A note on its contents

One file contains a database connection string with a user name and password.
It is not reproduced here or anywhere else in this repository. The owner may
want to rotate that credential, since the repository's history holds it.

### 3.4 What it implies for Cx — as questions, not requirements

The repository shows an engine built today from three languages, joined by a
C ABI and HTTP. If Cx is to replace or join any of those parts, each of these
has to be answered by the owner; none can be inferred:

1. Which part is Cx for — the client ("physics shard"), the VFS, new engine code
   beside them, or all of it eventually?
2. Must Cx call the existing 26 VFS functions (Cx calling C), be callable from
   the C++ client (C calling Cx), or both? This decides the first FFI direction.
3. `vfs_init_gpu` takes function pointers and `vfs_get_stats()` returns a struct
   by value. Cx has neither function-pointer values nor a C-compatible
   struct-return convention today.
4. The VFS uses worker threads. Cx has no threads and no stated memory model for
   sharing.
5. The client is Windows-first (WinHTTP, MSVC, MinGW). The Cx LLVM plan already
   names Windows and Linux; nothing here contradicts that.

## 4. The Cx roadmaps

### 4.1 Where the engine items come from

Checked by searching each file:

| Document | Vulkan | DirectX | NOD | physics | renderer | vector/matrix types | TSG |
|---|---|---|---|---|---|---|---|
| `docs/frontend/ROADMAP.md` | – | – | – | – | – | – | – |
| `docs/ROADMAP_v3.1.md` | – | – | – | – | – | – | – |
| `docs/backend/cx_backend_roadmap_v3_1.md` | – | – | – | – | – | – | – |
| `docment/ROADMAP.md` (living) | yes | yes | yes | yes | yes | – | yes |

In the living roadmap they are five lines — "1.0+ Graphics begins: Vulkan/DX12
bindings", "1.1+ Renderer", "1.2+ Physics", "1.3+ Audio", "1.4+ Networking / NOD
Protocol", and "2029+ TSG playable" — added by commit `10ac2ed` on 2026-07-05
("docs: reconcile roadmap — record actual 0.3 scope + current forward plan").
The file itself dates from `7129e58` (2026-05-05).

`docs/post_0_1/gene_phen_design.md` line 326 is headed "Roadmap Placement
(current, per merged master roadmap)". No file of that description is in the
repository.

### 4.2 What the historical roadmaps did commit to on the engine side

From `docs/frontend/ROADMAP.md`, "Post-0.1 — Engine Systems", "Runtime and
Stdlib", "Filesystem I/O", "Tooling" and "Design Backlog". Every one is absent
from the code today unless marked.

| Commitment | State |
|---|---|
| Window and screen system — images, `Handle<Window>`, blit/present, events, headless mode, Win32 / Cocoa / X11 / Wayland | absent |
| GPU system — VRAM registry, GS types, `.drop(fence)`, a GPU memory lifetime model, a GPU rendering path | absent |
| Audio, "deferred until window system lands" | absent |
| Networking, TCP/UDP, "deferred until filesystem I/O is proven" | absent |
| Filesystem I/O through `Handle` | absent |
| Collections: hashset, hashmap, hashweb, `Vec<T>`, ring buffer, queue, stack, linked list, tree map | absent |
| `rc<T>`, `shared<T>`, reference-cycle handling | absent, no design |
| Region invalidation of handles (`region_id`) | field exists, unused |
| Handle-backed containers | absent |
| String utilities, sorting and searching | absent — no standard library is bundled |
| C FFI surface | absent, no design document |
| async / continuations / lambdas / closures, reflection | absent, no design document |
| Ricey registry and package integration | absent; the resolver refuses registry imports |
| LSP; `cx build` / `run` / `test` / `check`; CLI visualizer | absent |
| Cranelift JIT | done, for a subset of the language |
| LLVM AOT | absent — a stub, no LLVM dependency |
| gene + phen | done |
| 2D–4D arrays | done on both backends |

### 4.3 The full comparison

`report/roadmap_item_table_2026_11_18.md` compares every item of the three
historical roadmaps with the repository, 190 rows, each with the evidence: a
program that was run on both backends, or the file and line that was read.
Its counts:

| Status | Rows |
|---|---|
| Absent | 52 |
| Partial | 26 |
| Contradicted by current behaviour | 13 |
| Superseded by a later document or count | 8 |
| Done on the interpreter only | 6 |

(The remaining rows are done on both backends, or are statements rather than
deliverables.) The thirteen contradictions are places where a document marked
frozen or locked says one thing and the language does another — for example
"expression statements require semicolons" (they do not), "`const` initializers
are literal-only" (a call is accepted), "`*arr` shorthand removed" (fixtures use
it), "lazy loading — only referenced symbols loaded" (an unreferenced function
with an error in it fails the importing program).

Two rows have moved since the table was produced: the unknown-`while` ruling
(L2) was confirmed by the owner, and IR reproducibility (T4) is fixed on
`backend/m1.1-static-data`.

### 4.4 Items the living roadmap dropped without saying so

Present in the historical roadmaps and not in the living one's forward plan:
the window and screen system, the GPU system's four named parts, filesystem
I/O through `Handle`, hashweb, `rc<T>` / `shared<T>`, region invalidation,
handle-backed containers, the LSP, and async / lambdas. They may have been
dropped on purpose; the living roadmap does not record a decision either way.

## 5. What is missing

To reconcile TSG's requirements with a Cx roadmap, these would be needed. None
is in either repository.

1. **The "older Cx master roadmap"** the owner referred to — the document that
   listed Vulkan, DirectX 12, math types, the renderer, physics and the NOD
   Protocol before 2026-07-05, if it is a different document from the living
   roadmap.
2. **`shared-docs/INTEGRATION_GUIDE.md`** and
   **`shared-docs/architecture/vfs-design.md`** from the TSG side.
3. **The TSG engine repository** (`TSG-Engine`), or at least the VFS's C header
   — the 26 function signatures are the first concrete FFI target Cx could have.
4. **A definition of the NOD Protocol.** It is a name in one roadmap line.
5. **A statement of which parts of the engine Cx is meant to implement**, and in
   what order.

## 6. What this does not establish

- That TSG needs any particular Cx feature. The engine gaps listed in
  `report/tsg_llvm_readiness_2026_11_18.md` §6 were derived from what any engine
  of this kind needs and from the owner's description; this report adds the
  five questions of §3.4 and removes none.
- That any graphics API has been chosen. "Vulkan/DX12 bindings" is one line in
  the living roadmap; the VFS notes mention Vulkan callbacks. Neither is a
  design decision, and none is made here.

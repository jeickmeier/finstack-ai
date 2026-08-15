# PR-058 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Intended branch (when admitted): `codex/pr-058-remote-session-server`
Intended baseline: local `main` at `834a071147ed415b794fb6e7ead8472f18094f84`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-058. The closed PR-054
envelope is not reused. The PR-055–PR-057 envelopes are not reused.
PR-058 is the only active logical PR once admitted. This planning
file does not admit the PR, start Phase 8, or record Phase 8
entrance.

## Execution envelope

Authorized by the owner sentence `Proceed to PR-056 through PR-066`
on 2026-08-15, recorded as:

```
Run PR-056 through PR-066 sequentially; mode=integrated; target=main;
local branch/commit/merge authorized; external actions=none;
stop before any gate crossing unless a separate passing gate decision exists.
```

PR-058 is the only active logical PR. Do not start PR-059+ until
this PR is `Done`. Phase 9 (PR-062–PR-066) stays blocked on G7 and
Phase 9 entrance.

Not authorized as a standalone sentence. Suggested text when the owner is ready:

```
Run PR-058; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

`implement the plan` is enough only if it names that same local-only
integrated envelope **and** the admission checks below are already
true. Do not infer authorization from this planning file, from
`continue`, from Phase 7 `Done`, from G6 `Passed`, or from the
PR-055–PR-057 plans existing.

When authorized, reuse:

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden: push, hosted PR/merge, npm/pypi publish, crates.io
publish, tag, G5 inference, G7 inference. Do not write `G5-D-*` or
`G7-D-*`. Do not start PR-055–PR-057 or PR-059+. Do not cut or
publish `0.1.0`. Do not bump the lockstep workspace version off
`0.0.4`.

## Admission (when authorized)

Do not admit coding until all of the following are true. Planning
this file does not record them.

### Phase 8 entrance is `Passed` (2/2)

Recorded by PR-055. Current state is `Passed` (2/2).

| Entrance bullet | Current state |
| --- | --- |
| Native preview, Python alpha, WASM alpha, durability beta, plugin alpha | **Satisfiable.** G3/G4/G5/G6 are named (`G5-D-durable-beta-a9568bd869b5`). Do not record `PH8-E-entrance-gates-*` until PR-055 is admitted. |
| Public API change backlog triaged | **Blocked.** `docs/implementation/public-api-change-backlog.md` does not exist. |

Do not infer G5 from Phase 6 `Done`. Do not write the backlog or
`G5-D-*` from this PR. If the owner says `implement the plan` while
entrance is still `0/2`, **stop**.

Phase 8 entrance, once Passed, is recorded by the first admitted
Phase 8 PR. Do not re-record `PH8-E-entrance-*` here.

### PR-055, PR-056, and PR-057 are `Done`

All three are planned only and still `Todo`. Implementation Plan
lists them before PR-058. Keep one active logical PR. Do not admit
PR-058 while any predecessor is `Todo` unless the owner explicitly
authorizes parallel Phase 8 work in the same sentence.

PR-058's *code* dependencies are PR-039 (canonical-CBOR),
PR-046/PR-047 (sessions/lanes), and stable public events. Those are
already on `main`. `SecurityAuditSink` / `SecurityAuditGate` already
exist in runtime (`crates/finstack-ai-runtime/src/audit.rs`).
`schemas/remote/` and `schemas/process/` are reserved. WIT worlds
are not remote DTOs (Phase 7); this PR must keep them that way
(ADR-014).

### Other admission checks

- Changing journal, remote-protocol, or WIT compatibility **policy**
  is an Implementation Plan §6.3 ADR trigger. Introducing the
  reserved remote/process families and filling their fixtures is
  **not** a policy change if TDD §28.4 rows are copied verbatim.
  Do not silently relax inbound reject-unknown.
- Threat Model section 18 is triggered (network listener, protocol
  parser). Primary **TM-09**. Also **TM-10** / **TM-11** (command
  auth, idempotency), **TM-16** (pre-allocation limits), **TM-19**
  (locator/tenant), **TM-20** (no store-private leak). Complete the
  review before merge.
- ADR-014 is `Not started` / `Missing`. This PR plus Phase 7 WIT
  evidence should make it `Implemented` / `Verified` only if remote
  DTOs, process family fixtures, and WIT worlds remain pairwise
  distinct. Do not unify vocabularies.
- ADR-021 is `Not started` / `Missing`. This is its mapped delivery.
  Framing share + distinct enums can become `Implemented` /
  `Verified`. Process *session* vocabulary stays later (PRD §18).
- Runtime and default SDK stay protocol-free (current
  `finstack-ai-protocol` rustdoc). Do not add `rustls` / remote
  DTOs to kernel or runtime.

## Traceability

Implementation Plan PR-058; PRD UC-06; FR-RT-008; ADR-014; ADR-021;
ADR-015 (envelope bytes); TDD §28.2–28.4.
TM-09 / TM-10 / TM-11 / TM-16 / TM-19.
G7 is out of scope.

## Acceptance mapping

Six Implementation Plan bullets map 1:1 to A01–A06.

- PR-058-A01: Reconnect obtains a fresh authoritative snapshot
  before live events. Order is authenticate →
  `open-session(last-known-durable-sequence)` → snapshot at `S`
  (or explicit no-snapshot) → durable tail `S+1..B` → sync
  barrier `B` → live batches after `B`. A test that injects a
  live event before the barrier fails the server. Transient
  progress from the previous connection is not replayed.
- PR-058-A02: Unknown protocol versions fail before session
  acquisition. Hello selects the highest mutually supported
  version at or above both peers' downgrade floor. Unknown
  version, unknown payload family, or missing mandatory security
  features close the connection with no session open and no
  fallback.
- PR-058-A03: Only bounded `ClientHello`, `ServerHello`,
  `Authenticate`, `AuthResult`, and `Close` are parsed before
  authentication. Pre-auth frame ceiling is **16 KiB**; the
  4-byte length is rejected **before allocation** when over the
  ceiling. No session/target lookup pre-auth. Oversized/malformed
  input fails closed. Post-auth commands carry command ID, digest,
  tenant scope, and locator; unauthorized/scope-mismatch is
  audited without revealing target existence.
- PR-058-A04: The server refuses non-loopback plaintext and
  downgrade below policy. Default listen is Unix socket or
  loopback TCP. Non-loopback TCP requires explicit enablement,
  TLS 1.3+, and configured authentication. Bearer credentials are
  never accepted over plaintext. Reconnect never interleaves live
  events before the snapshot/tail barrier (A01). A slow client
  pauses within the credit window, then disconnects and resumes
  from its durable cursor; terminal completion is not dropped.
- PR-058-A05: Protocol adapters do not expose store-private or
  kernel-private structures. Public remote DTOs are commands,
  results, snapshots, and `RunEvent` batches already on the
  public event surface. No `rusqlite` types, journal page
  layouts, or kernel-private reducer state cross the socket.
- PR-058-A06: Frame/handshake fuzz and size-limit tests are
  **shared** by remote and process payload fixtures. Vocabulary
  compatibility tests remain **separate**
  (`fixtures/compatibility/remote/` vs
  `fixtures/compatibility/process/`). A process-family hello must
  not decode as a remote session command, and the reverse.

Principal changes that are not extra acceptance IDs, but are
required to prove the six bullets:

- Generic 4-byte length prefix + family-tagged CBOR envelope +
  hello/version negotiation in `finstack-ai-protocol`.
- Remote session vocabulary (distinct enums).
- Reserved process family that reuses framing only.
- Reference Unix-socket / loopback TCP server with auth hooks and
  single-writer session routing.
- Authenticated command IDs/digests, idempotency receipts,
  credit-window flow control.
- Rust and TypeScript client helpers.
- `SecurityAuditSink` required in server readiness.

## Locked design

### Layout

Technical Design §2 does not name a server crate; §3.1 does
("server/client adapters depend on protocol plus SDK/runtime").
Put the reference server in `crates/`, not `extensions/` or
`plugins/`.

```text
crates/finstack-ai-protocol/
  src/frame.rs          # NEW: 4-byte BE length + pre-alloc check
  src/handshake.rs      # NEW: family-tagged hello/auth/close
  src/remote.rs         # NEW: remote session DTOs only
  src/process.rs        # NEW: process family id + handshake-only
                        #       fixtures; no session command enum
crates/finstack-ai-server/          # NEW reference server + Rust client
  src/lib.rs, listen.rs, session.rs, auth.rs, credit.rs
schemas/remote/                     # un-reserve; candidate-v1
schemas/process/                    # un-reserve family id only
fixtures/compatibility/remote/v1/
fixtures/compatibility/process/v1/  # frame/handshake twins + distinct vocab
bindings/finstack-ai-wasm/js/src/remote.ts   # TS helpers; no second CBOR codec
```

Do not add remote DTOs to `finstack-ai-kernel` or
`finstack-ai-runtime`. Do not put the server under `plugins/`.
Do not invent `server/` at the repo root.

Workspace member + `[workspace.dependencies]` at `0.0.4`.
`finstack-ai-server` may depend on `finstack-ai` (native-tokio),
`finstack-ai-protocol`, and `finstack-ai-runtime` (audit gate +
public Session/Lane). Default `finstack-ai` features must not
depend on `finstack-ai-server` or `rustls`.

### Shared framing (ADR-021)

Copy TDD §28.2:

```text
4-byte unsigned big-endian payload length
CBOR envelope payload
```

- Treat the length as untrusted. If `length > ceiling`, fail
  **before** allocating the payload buffer.
- Pre-auth ceiling = `16 * 1024`. Post-auth ceiling is configured
  and still checked before allocation (use existing protocol
  string/map/depth ceilings; do not use the 8 MiB journal envelope
  as the pre-auth cap).
- Compression disabled on the reference server.
- Envelope carries `payload_family: remote | process` (stable
  strings), `protocol_version`, and a CBOR body. Unknown family
  closes the connection.
- Handshake code is generic over family. Remote and process do
  **not** share message enums.

Reuse `finstack-ai-protocol::{encode,decode}` (ADR-015 profile).
Do not add a second CBOR writer.

### Remote vocabulary (ADR-014)

Pre-auth only:

- `ClientHello` / `ServerHello` (versions, downgrade floor,
  mandatory security features)
- `Authenticate` / `AuthResult`
- `Close`

Post-auth (minimum set; reject unknown variants):

- `OpenSession` (optional last-known durable sequence)
- `Snapshot` / `NoSnapshot`
- `DurableTail` / `SyncBarrier`
- `EventBatch` (public `RunEvent` projection, not kernel-private)
- `Command` / `CommandResult` (start/cancel/resolve/complete as
  already public SDK operations; include UUIDv7 command ID,
  locator, digest)
- credit `Grant` / `Ack`
- `Close`

Inbound: `deny_unknown_fields`. No silent discard.

Process family in this PR: family tag + the same five handshake
kinds **under process type names**, plus fixtures. No process
plugin session commands (PRD: "process vocabulary later").

WIT worlds stay `@0.0.4` experimental and are not these enums.

### Reference server

- Default bind: Unix socket **or** `127.0.0.1`. Binding
  `0.0.0.0` / non-loopback without TLS 1.3+ and auth is
  `server_listen_invalid`.
- Unix socket: filesystem mode/owner documented; no bearer over
  a second plaintext TCP hop.
- Auth hook: application-supplied verifier. Missing auth config
  on TCP = fail closed. `SecurityAuditGate::enable` is required
  before accept; unhealthy sink = not ready.
- Single-writer: at most one writer connection per session.
  A second open fails closed and is audited (`ScopeMismatch` /
  busy), without confirming another tenant's session exists.
- Parse/auth deadlines and attempt caps on the hello/auth
  window.
- Idempotency: equal command ID + digest within the horizon
  returns the original receipt; conflicting reuse fails closed
  and is audited.
- Credit window: bounded byte/item credits; slow client
  disconnects; resume from durable cursor.

TLS: `rustls` / `tokio-rustls` only in `finstack-ai-server`.
Confirm versions at implementation time. No `native-tls` unless
already forced elsewhere. No `libloading`.

### Clients

- Rust: encode/decode + async connect/reconnect helper in
  `finstack-ai-server` (or `protocol` sync + server async).
  Reconnect must implement A01 ordering.
- TypeScript: length-prefix + call existing WASM
  `journal_known_answer` / canonical-CBOR helpers for body
  bytes. **Do not** add a second JS CBOR library
  (`bindings/finstack-ai-wasm/js/src/index.ts` already forbids
  that). Tree-shakeable module; no provider keys.

### SecurityAuditSink

Server construction calls `SecurityAuditGate::enable`. Tests:

- malformed pre-auth frame → audit, no target lookup
- unknown locator → `UnknownLocator`, response does not
  distinguish missing vs unauthorized
- scope mismatch → `ScopeMismatch`
- no raw tokens or session ids in audit events (digests only)

`SecurityAuditSink` is not an `Observer` (TDD §19).

### Compatibility families

Un-reserve `remote` and `process` in
`schemas/schema-families.toml` to `candidate-v1` / in-use.
Keep TDD §28.4 profiles. Shared fuzz/size tests live in
`finstack-ai-protocol` and take a family parameter. Vocabulary
golden files stay in separate directories.

### Graph

- `finstack-ai-kernel` / `finstack-ai-runtime` / default
  `finstack-ai` / `finstack-ai-wit` / `check-wasm`: no
  `finstack-ai-server`, no `rustls`, no remote DTO dependency
  on the default SDK path. Protocol crate may gain frame/remote
  modules; runtime must still not depend on protocol for
  in-process work (existing rule).
- `finstack-ai-server` may use tokio + rustls + protocol + SDK.
- Add `finstack-ai-server` and `rustls` to wasm/kernel forbidden
  sets in `tools/wasm_package/check.py`.

Do not restore `tools/architecture/`.
Do not invent `mise run schema-governance`.

## Tasks (when admitted)

Task IDs are minted at admit, not now. Do not start these until
admission checks pass.

1. Tracking: confirm Phase 8 entrance `Passed` (2/2) and
   PR-055–PR-057 `Done`; open
   `codex/pr-058-remote-session-server` from the then-current
   `main` tip. Mark PR-058 `In progress`. Do not re-record
   Phase 8 entrance.
2. Generic frame + handshake + pre-auth 16 KiB / unknown-version
   tests (A02, A03, A06 shared half).
3. Remote vs process family split + separate vocab fixtures
   (A06, ADR-014/021).
4. Reference server: listen policy, TLS floor, audit gate,
   single-writer, command idempotency (A03, A04).
5. Reconnect snapshot-tail-barrier-live + credit window + slow
   client (A01, A04).
6. Rust/TS clients, public-surface-only adapters (A05), TM-09
   review, candidate evidence. Stop before `G7-D-*`.

## Explicit exclusions

No public cloud control plane, gateway dashboard, or multi-region
routing. No process-plugin session vocabulary (framing only). No
WIT world change. No `@1.0.0` worlds. No default SDK feature that
pulls the server or rustls. No hosted listen on public interfaces
in tests (loopback/Unix only). No observer/OTel work (PR-057).
No workflow adapters (PR-059). No G5 or G7 decision. No `0.1.0`
bump, publish, or tag.

## Validation

- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai --locked`
  — no `finstack-ai-server`, no `rustls`
- `cargo tree -p finstack-ai --locked --no-default-features` — same
- `cargo tree -p finstack-ai-protocol --locked` — no tokio, no
  rustls, no kernel
- `cargo tree -p finstack-ai-server --locked` — has protocol +
  rustls/tokio; no wasmtime, no `libloading`
- `cargo test -p finstack-ai-protocol --offline --locked`
- `cargo test -p finstack-ai-server --offline --locked`
- focused JS tests for the remote helper (no Playwright matrix
  required)
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `uv run --no-project python tools/wasm_package/check.py graph`
- `mise run check` after the candidate is otherwise green

Do not require `mise run ci`, a public TLS certificate, or a
hosted collector.

## Suggested authorization sentence

When Phase 8 entrance is `Passed` (2/2), PR-055–PR-057 are
`Done`, and the owner is ready:

```
Run PR-058; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

A later, separate sentence is required to record `G5-D-*`, triage
the public API change backlog, admit PR-055–PR-057, or record
`G7-D-*`. Do not infer those from `implement the plan`.

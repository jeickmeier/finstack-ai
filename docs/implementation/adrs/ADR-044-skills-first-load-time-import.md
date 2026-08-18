# ADR-044: Skills-first load-time import

## Status

Accepted

## Date

2026-08-17

## Accountable role

Ecosystem lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

ADR-041 implements mid-run `capability_list` / `capability_activate` and
explicitly leaves FR-10 load-time `SKILL.md` / plugin import unauthorized.
FR-06 activation already exists. Adopters still need a composition-time
way to turn checked-in skill documents into `CapabilitySpec` values
without a seventh port, a new `RecordBody`, or a per-run filesystem scan.

Full Agent Plugins and `plugins/` WIT remain a different isolation
surface. This record authorizes only a skills-first importer.

## Decision

Load-time skill import is a **leaf crate**
`extensions/toolsets/finstack-ai-tools-skill-import`
(`finstack-ai-tools-skill-import`). It is not under `plugins/`, not a
WIT guest, and not added to the default SDK graph.

- Skills-only first cut. No Agent Plugin manifest, no `plugins/` host,
  no `scripts/` or `references/` materialization.
- Parse happens at process-start / composition time. The per-run path
  does no filesystem I/O and no catalog scan.
- Catalog default-off: the host must opt in. A disabled catalog imports
  nothing even if documents are supplied.
- `SKILL.md` YAML-ish frontmatter maps onto `CapabilitySpec`
  (`name`/`id` → capability id, `description` → description). The
  markdown body maps onto one untrusted `InstructionSpec`.
- Imported capabilities cannot set `trusted_application_instructions`.
  They contribute no context-provider, toolset, or middleware handles in
  this cut. Activation is `Application` (explicit), never `Always`.
- The lock digest covers imported **content** (id, description, body),
  not the source path.
- Frontmatter keys `scripts` and `references` fail closed.
- `mcp.json` is the second half of this wave: a construction-time
  snapshot into `finstack-ai-tools-mcp` (`McpConfig::try_from_mcp_json`).
  It is not a run-time scan. Existing `finstack-ai-tools-skills`
  activation tools are unchanged.

## Consequences

- Hosts that opt in receive `CapabilitySpec` values they can register
  before resolve. Mid-run activation stays ADR-041.
- Path-identical copies with different content produce different
  digests. Content-identical copies from different paths produce the
  same digest.
- WASM graphs must not grow this crate; it is `FORBIDDEN_WASM`.

## Rejected alternatives

**Put the importer under `plugins/`.** Rejected: WIT isolation is not
this first cut.

**Scan `SKILL.md` on every run.** Rejected: composition-time only.

**Default-on catalog.** Rejected: surprise activation and lock churn.

**Treat imported instructions as trusted application instructions.**
Rejected: an MCP/skill document is not an application-instruction
authority.

**Include `scripts/` and `references/` now.** Rejected: first-cut
scope; a later ADR may add them.

**Seventh port or new `RecordBody`.** Rejected: G-01 holds.

## Compatibility and schema-change classification

No journal kind, kernel input, or schema change. Additive leaf crate
and MCP construction helper.

## Security classification

- References: SEC-INV-001; TM-01
- Threat Model review trigger: none beyond existing capability-catalog
  notes. This record does **not** invent a review id.
- Residual: hosts must not mark imported instructions trusted; `mcp.json`
  snapshots still require the MCP allowlist.

## Affected requirements, design, and delivery

- Affected requirements: FR-10
- Affected Technical Design: capability catalog / FR-06 follow-on.
  Planning files are not edited.
- Implementation: gap-fix G8
- Unchanged: `finstack-ai-tools-skills` activation Toolset (FR-06)

## Supersession metadata

None. This ADR supersedes no prior ADR. It does not change ADR-041
except to authorize the importer ADR-041 deferred.

## Reconsideration conditions

May change only through a new superseding ADR. Adding `scripts/` /
`references/`, Agent Plugins, per-run scans, default-on import, or
trusted-application-instruction authority requires that path.

## Approval and implementation-evidence links

- Approval: accepted by the decision owner to execute FR-10 first cut
  locally only, without publication
- Implementation evidence: Missing (no published evidence id)

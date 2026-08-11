# PR-025 execution plan

Date: 2026-08-11

## Execution envelope

- mode: integrated
- branch: `codex/pr-025-filesystem-tools`
- baseline: `0ae6cd9a2244287b844d1c8720fe4cf1f765fc34`
- integration target: `main`
- authorized local actions: branch, edit, test, commit, and merge
- authorized external actions: none

## Contract and scope

PR-025 adds trusted native calculator and filesystem leaf toolsets under
`extensions/toolsets/`. The filesystem package implements the existing public
`Toolset` contract over an explicitly opened root directory; it does not add a
kernel port or make path strings authoritative.

The implementation sequence is:

1. Add a deterministic bounded calculator with one cached public tool spec and
   stable decimal-number semantics suitable for the benchmark path.
2. Open and retain an authorized filesystem root capability, validate relative
   paths/patterns, and walk every path component through no-follow directory
   handles.
3. Implement bounded read, write, edit, list, glob, and content-search calls,
   authorizing and operating on the same opened handle/object.
4. Route oversized successful results through the optional scoped
   digest-verifying `ArtifactStore`; fail closed when staging is required but no
   store or tenant scope is available.
5. Add traversal, protected-pattern, symlink, rename/symlink-swap, output-bound,
   context-propagation, and spec-caching tests plus a high-volume benchmark.
6. Run focused, target, dependency, architecture, security, benchmark, and
   aggregate validation; bind immutable candidate and local integration proof.

## Acceptance map

- A01: traversal, symlink escape, protected-file, and oversized-output fixtures
  pass with no untracked spill files.
- A02: rename and symlink-swap race fixtures cannot escape an opened root;
  platforms without the required safe primitives fail construction closed.
- A03: each toolset builds its immutable tool specifications once and returns
  the same shared specification allocation across turns.
- A04: every dispatch consumes the committed effect identity and authenticated
  principal context; artifact scope derives from that exact call context.
- A05: the deterministic calculator high-volume path is benchmarked.

## Security and decision review

- TM-03 and SEC-INV-001/007/008/012 govern path authority, traversal,
  symlinks/races, bounds, principal/effect binding, and optional batteries.
- Rustix official documentation confirms directory-fd-relative `openat`,
  `OFlags::NOFOLLOW | DIRECTORY | CLOEXEC`, descriptor metadata, and relative
  rename primitives. The Unix implementation walks one validated component at
  a time; unsupported targets fail construction closed.
- Protected patterns are application policy, content never grants authority,
  and artifact attributes remain non-authoritative.
- No ADR trigger is identified at admission; no primary-port, effect-order,
  compatibility-policy, or trust-boundary change is introduced.

## Exclusions

No shell execution, Git integration, browser automation, OS sandbox, recursive
write/delete, ambient root discovery, secret access, untracked spill file,
hosted pull request, publication, push, or later PR-056 hardening is included.

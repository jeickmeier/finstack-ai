# PR-067 TM-04 review — secret-free config needles

Date: 2026-08-15
Reviewer: me@jeickmeier.com
Candidate: local `main` working tree (uncommitted; no candidate SHA)
Decision: Pass for the key-token expansion. No threat-model rewrite.
No Implementation Plan §6.3 ADR trigger. Do not infer merge or G8
re-opening.

## Scope

- `crates/finstack-ai/src/bundle.rs` `contains_secret` / `key_looks_secret`
- `CHANGELOG.md` Unreleased Fixed
- Existing canary plus `apikey`, `auth`, `authorization`, `secretkey`,
  `client_secret`, `auth_token`, and allow-list cases
  `oauth_client_id` / `author` / `authority`

Triggered control: Threat Model TM-04 / SEC-INV-005 (secret material
must not enter bundle configuration objects).

## Disposition

PASS  Key scan only. Values are not inspected, which avoids false
      positives on prose and remains the 1.0.x contract. Keys ending
      in `_ref` stay allowed.

PASS  Needles are whole tokens after split on non-alphanumeric
      characters, plus the compounds `api_key`, `access_key`, and
      `private_key`. Raw substring `auth` is not used, so `oauth`,
      `author`, and `authority` do not fail closed.

RESIDUAL  An innocuous key that holds a credential string is still a
      host problem. A config-schema allowlist is AgentSpec evolution
      and is out of PR-067.

No journal, WIT, or protocol meaning change.

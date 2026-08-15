# Conformance suites and badge process

Published first-party suites for the six primary ports and the
experimental plugin host. Workspace suite version is **0.1.0**
(`PORT_CONFORMANCE_SUITE_VERSION` / plugin-host crate version).
This page is not a marketplace and does not issue hosted badges.

## Suites

| Suite | Surface | How to run |
| --- | --- | --- |
| Model / provider | `check_model_conformance` | `mise run conformance` |
| Toolset | `check_toolset_conformance` | same |
| Context | `check_context_conformance` | same |
| Middleware / compaction | `check_middleware_conformance`, `check_compaction_conformance` | same |
| Observer | `check_observer_conformance` | same |
| Journal store | `check_journal_store_conformance` | same |
| Plugin host | `plugins/finstack-ai-plugin-host` G6 hostile + lockfile tests | same |
| Binding traces | `finstack-ai-test` goldens | `mise run test` |

A failed port check prints:

```text
{port} contract `{contract}` suite {version} violated: …
```

A failed plugin check prints:

```text
plugin world `{world}` contract `{contract}` suite {version} violated: …
```

A message that says only `assert failed` is not a published failure.

## Badge process

A first- or third-party package may claim
`finstack-ai <port> conformance <suite_version>` only after the
published suite passes on a named commit. The claim must list:

- the suite version (today `0.1.0`);
- the contract IDs that passed;
- the commit SHA.

There is no hosted badge issuer, commercial listing, or registry
fetch of suites. Copy the public helpers; do not depend on kernel
or runtime internals.

Experimental IndexedDB and WIT `@0.x` are not 1.0 badge surfaces.
See [support](support.md) and
[1.0 compatibility policy](../implementation/1.0-compatibility-policy.md).

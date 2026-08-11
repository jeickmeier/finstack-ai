# PR-021 local delivery evidence

PR-021 implements typed registration and one-time resolved-agent construction
for the six primary ports. The immutable implementation candidate is commit
`d12abc5ec917be1d725cafdd7e9250cb23f9799f` with tree
`5e4b2c4b9476d66f5c0071995ef58a8d5549a3f2` on
`codex/pr-021-rust-sdk`.

The exact local command inventory is in
[`candidate-validation.txt`](candidate-validation.txt), and the security and
threat-model disposition is in [`security-review.txt`](security-review.txt).

## Local acceptance map

| Acceptance | Immutable candidate proof | Disposition |
| --- | --- | --- |
| A01 | Duplicate registration, explicit source-checked replacement, missing selection, kind/version mismatch, alias conflict, construction failure, and descriptor mismatch carry stable codes and request/registration sources where those sources exist | Passed at candidate |
| A02 | Registry identities and aliases use ordered maps; resolution follows fixed model/toolset/context/middleware/store/observer order; the opposite-registration-order fixture produces an identical report and alias expansion | Passed at candidate |
| A03 | `ResolvedComponent` retains direct `Arc<dyn Port>` handles, and the lookup-count fixture proves repeated model/store/run-plan access does not return to the registry | Passed at candidate |
| A04 | Ready and factory registrations produce the same resolved shape; a selected factory and `Model::warmup` each execute once across repeated resolutions, while lifecycle shutdown is at-most-once | Passed at candidate |
| A05 | Test-only extension structs register components solely through the public `Extension`/`Registrar` protocol, with no SDK-owned extension-name list or package discovery | Passed at candidate |

The implementation remains within trusted in-process SDK composition. It adds
no package discovery, dynamic loading, plugin sandbox, provider, network client,
`AgentSpec`, capability/bundle surface, binding API, or publication. Local
integration, hosted validation, publication, and independent review are not
claimed by this candidate evidence.

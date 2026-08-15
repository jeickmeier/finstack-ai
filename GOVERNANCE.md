# Governance

## License

`finstack-ai` is dual-licensed under **MIT OR Apache-2.0**. See [`licenses/LICENSE-MIT`](licenses/LICENSE-MIT) and [`licenses/LICENSE-APACHE`](licenses/LICENSE-APACHE). Contributors license their contributions under the same terms.

## Contribution license

Contributions use the [Developer Certificate of Origin](https://developercertificate.org/) (DCO) sign-off. The project does **not** require a Contributor License Agreement (CLA). See [`CONTRIBUTING.md`](CONTRIBUTING.md) for sign-off instructions.

## Maintainer group

The **finstack-ai maintainers** group owns releases, security response, contribution acceptance, and merge authority for the repository.

| Role | Contact |
| --- | --- |
| Maintainer group | finstack-ai maintainers |
| Current members | `me@jeickmeier.com` |
| Release owner | `me@jeickmeier.com` |
| Contribution-acceptance owner | `me@jeickmeier.com` |
| Security response owner | `me@jeickmeier.com` |

Additional maintainers may be added by the release owner and recorded in this table.

## Architecture and process decisions

- Architecture decisions are recorded as ADRs under [`docs/implementation/adrs/`](docs/implementation/adrs/) and indexed in [`docs/implementation/adr-register.md`](docs/implementation/adr-register.md).
- The planning baseline in [`docs/planning/`](docs/planning/) is the implementation contract during normal coding.
- Delivery status, evidence, and exceptions are tracked in [`docs/implementation/`](docs/implementation/).
- Ecosystem-facing contract changes (journal schemas, event order, WIT worlds, remote protocols) require a public RFC process in addition to an ADR. Start from [`docs/rfcs/README.md`](docs/rfcs/README.md) and [`docs/rfcs/0000-template.md`](docs/rfcs/0000-template.md).

## Exceptions and waivers

Deviations from Engineering Standards **must** / **must not** rules require an accepted ADR or a documented, time-bounded waiver per Engineering Standards section 14 and the [`exceptions-register`](docs/implementation/exceptions-register.md).

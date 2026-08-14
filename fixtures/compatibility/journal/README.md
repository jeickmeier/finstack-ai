# journal fixtures

Historical JournalStore v1 and ADR-015 canonical-CBOR corpus. The first
versioned tree is `v1/`. JSON sidecars are diagnostic; sibling `.cbor`
files are the byte-identical journal encoding.

```text
fixtures/compatibility/journal/v1/cbor-profile/
fixtures/compatibility/journal/v1/record-payload/
fixtures/compatibility/journal/v1/envelope/
fixtures/compatibility/journal/v1/limits/
fixtures/compatibility/journal/v1/tamper/
```

Limit cases use recipes rather than checked-in 8 MiB files. Tamper cases
must fail checksum or sequence verification before `apply`.

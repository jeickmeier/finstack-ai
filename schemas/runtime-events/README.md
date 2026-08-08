# runtime-events (reserved)

Owner: `me@jeickmeier.com`  
Compatibility profile: durable/transient classified  
Fixtures: `fixtures/compatibility/runtime-events/`

Future runtime event schemas land here.

Unknown-field / evolution profile (Technical Design §20.2, §28.4):

- Durable-derived event payloads follow the durable-record rule: schema-declared
  ignorable optionals may be preserved; unknown state-bearing fields/kinds are
  fatal before apply. Durable-derived event IDs are replay-stable.
- Transient progress/diagnostic events follow diagnostic-metadata rules:
  retain-or-ignore; must not influence replay. Transient event IDs are
  explicitly non-replay-stable.

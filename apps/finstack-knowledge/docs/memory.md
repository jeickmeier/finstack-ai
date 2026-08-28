# Memory

The memory extension (`extensions/context/finstack-ai-memory`) gives the
agent durable recall across sessions. It is one composition with four
faces:

- **Store** — where memory records live, scoped by tenant.
- **Toolset** — model-callable tools to remember and search facts
  explicitly (`remember` and friends).
- **Context provider** — recall: before each turn, relevant memory
  records are retrieved (keyword/FTS ranking) and contributed to the
  model-visible context within an explicit budget. Contributions are
  data, never instructions.
- **Observer** — capture: watches committed runs and records durable
  facts automatically.

In the knowledge agent all four faces are wired, so facts remembered in
one session (from either the CLI or a notebook) are recalled in later
sessions against the same data directory. Retrieval is keyword-based in
this version; the store trait reserves the extension point for vector
retrieval without changing the composition.

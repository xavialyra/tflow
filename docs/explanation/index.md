# Architecture & Concepts (Explanation)

This section provides understanding-oriented explanations of `tui-launcher`'s design philosophy, internal architectural boundaries, and runtime invariants.

## Articles

- **[Architecture Overview](architecture-overview.md)**
  Domain boundaries across Workflow, Session, Engine, UI Chrome, and external execution, along with the 14 core dependency rules.

- **[Input & Navigation Model](input-and-navigation-model.md)**
  Deep dive into route catalogs, lossless input transport, key binding precedence, and view lifecycle invariants.

- **[Runtime Guarantees & Safety](runtime-guarantees.md)**
  Resource bounds, process group cleanup, execution sandboxing, and terminal state restoration.

---

For chronological design rationale and formal records, see **[Architecture Decision Records (ADRs)](../adr/index.md)**.

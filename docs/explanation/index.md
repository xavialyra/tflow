# Architecture & Concepts (Explanation)

This section provides understanding-oriented explanations of `tlaunch`'s design philosophy, internal architectural boundaries, and runtime invariants.

## Articles

- **[Architecture Overview](architecture-overview.md)**
  Domain boundaries across Workflow, Session, Engine, UI Chrome, and external execution, along with the 14 core dependency rules.

- **[Architecture Convergence](architecture-convergence.md)**
  Implementable migration plan for protocol ownership, execution lifecycle, task correlation, scheduling decisions, and contract gates.

- **[Input & Navigation Model](input-and-navigation-model.md)**
  Deep dive into route catalogs, lossless input transport, key binding precedence, and view lifecycle invariants.

- **[Runtime Guarantees & Safety](runtime-guarantees.md)**
  Workflow trust boundary, resource bounds, process group cleanup, and terminal state restoration.

---

For chronological design rationale and formal records, see **[Architecture Decision Records (ADRs)](../adr/index.md)**.

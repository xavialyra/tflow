# Technical Reference

The Reference section contains formal specifications, configuration schemas, command-line arguments, and protocol boundary definitions for `tflow`.

## References

- **[CLI Reference](cli.md)**
  Command-line flags, environment variables, validation mode (`--check`), and parameter syntax for direct view invocation.

- **[settings.toml Specification](settings-toml.md)**
  Passive host settings, global engine defaults, image protocols, and host-level semantic style overrides.

- **[Suite Manifest Specification](suite-toml.md)**
  Orchestration suite manifests, workflow mounting, entrypoints, alias routing, and suite-level semantic style overrides.

- **[workflow.toml Specification](workflow-toml.md)**
  Workflow manifest format, single-file workflows, static View declarations, query schemas, Engine configurations, producer handlers, and the version-1 JSON protocol.

- **[Producer Protocol](producer-protocol.md)**
  Literal configuration values, public producer context, the version-1 request/response boundary, and successful command feedback with a 3-second display timeout.

- **[Picker Details via Companion Views](picker-preview.md)**
  Replacement reference for the removed Picker preview API, named Capture companions, and ordinary View bindings.

- **[Form Content and State](form.md)**
  Native editable fields, declared/script content producers, validation, command state, and keyboard behavior.

- **[Theme TOML Specification](theme-toml.md)**
  Flat scheme color syntax and whitespace handling, built-in defaults, component fields, and workflow selected-style merging.

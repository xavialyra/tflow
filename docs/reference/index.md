# Technical Reference

The Reference section contains formal specifications, configuration schemas, command-line arguments, and expression syntax definitions for `tui-launcher`.

## References

- **[CLI Reference](cli.md)**
  Command-line flags, environment variables, validation mode (`--check`), and parameter syntax for direct view invocation.

- **[config.toml Specification](config-toml.md)**
  Root configuration specification, including theme selection, default view, global keymap defaults, and session commands.

- **[plugin.toml Specification](plugin-toml.md)**
  Plugin manifest format, view declarations, query schema types, engine configurations (`picker`, `capture`, `embedded`), and command actions.

- **[Expression Syntax](expressions.md)**
  Dynamic `{{ namespace.path }}` syntax, evaluation stages, available namespaces, and execution limits.

---
title: "Expression Syntax and Evaluation"
type: "reference"
tags:
  - expressions
  - templates
  - evaluation
  - security
description: "Reference for {{ namespace.path }} dynamic expressions, scopes, lifecycle stages, and safety limits."
---

# Expression Syntax and Evaluation

`tui-launcher` configurations and command arguments can embed dynamic values using `{{ namespace.path }}` expressions.

## Syntax Rules

- **Whole Expression**: When an entire field is a single expression (e.g. `query = "{{ selection.value }}"`), the resolved value preserves its underlying JSON type (e.g., boolean, integer, or array).
- **Mixed Expression**: When an expression is embedded alongside string literals (e.g. `args = ["--file={{ selection.value }}"]`), the expression is interpolated into a string.
- **Safety**: Expressions are purely data-retrieval operations over in-memory namespaces. Expressions **never execute commands or access filesystem paths**.

## Standard Namespaces

| Namespace | Availability | Description | Example Path |
| :--- | :--- | :--- | :--- |
| `selection` | Item-activated commands in `picker` | The currently selected item object. | `{{ selection.value }}`, `{{ selection.label }}` |
| `page` | Active view scope | View-level query and state. | `{{ page.input }}` |
| `query` | Route query scope | Parsed query schema parameters. | `{{ query.target }}` |
| `env` | Global runtime scope | Environment variables. | `{{ env.HOME }}` |

## Evaluation Lifecycles

Values are resolved strictly at the lifecycle stage where their required namespaces exist:
1. **Compilation Time**: Static configuration paths are verified.
2. **Mount Time**: View and route queries are resolved.
3. **Execution Time**: `selection` and dynamic command payloads are resolved upon keypress.

## Resource Limits and Safety Budgets

To prevent memory leaks or denial-of-service from cyclic or maliciously crafted templates, evaluation is strictly bounded:
- **Maximum Output Budget**: 16 MiB shared memory budget per evaluation operation.
- **Traversal Limits**: Depth, path segment count, and visited node counts are bounded.
- **Cancellation**: Traversal operations respect cooperative cancellation signals.

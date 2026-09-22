---
title: "How to Add Route Completion to a Launcher"
type: "how-to"
tags:
  - picker
  - navigation
  - completion
  - aggregate
  - commands
description: "Give an aggregate Picker a Tab and Space route completion flow using workflow commands, a popup View, and a shared route lookup."
---

# How to Add Route Completion to a Launcher

## Problem

You want a launcher View where typing a route name (`calc`, `sys`) and pressing
`Tab` jumps straight to that View, while ambiguous input opens a popup list of
routes. Route entry belongs to the workflow, not to the Picker engine, so this is
ordinary View and command configuration.

## Solution

The development fixture implements this end to end in
[`core/workflow.toml`](../../tests/fixtures/config/workflows/core/workflow.toml)
and its `scripts/` directory; this guide explains the four moving parts.

### 1. Let the aggregate View own Tab and Space

```toml
[views.main.keymap]
mode = "item"
tab = "complete"
space = "separate_route"
```

`mode = "item"` keeps the per-item bindings that
[aggregation](picker-views.md#3-compose-multiple-sources-in-an-aggregation-view)
attaches to each item, and the table adds a base layer that stays available
while the list is empty, unfiltered, or still loading.

### 2. Resolve the typed prefix

`Tab` has to answer three ways with one operation type, because a command's
declared type is static: jump on an exact alias or reference, jump on a single
candidate, and otherwise open the popup. Every branch returns `navigate`.

```python
# scripts/complete.py (abridged)
request = json.load(sys.stdin)
context = request.get("context") or {}
state = (context.get("engine") or {}).get("state") or {}
prefix = state.get("input", "") if isinstance(state, dict) else ""

matches = completion_routes.candidates(prefix)
route = completion_routes.find_route(prefix.strip())
if route is None and len(matches) == 1:
    route = matches[0]

if route is not None:
    operation = {"type": "navigate", "target": route["value"], "clear_input": True}
else:
    operation = {
        "type": "navigate",
        "target": completion_routes.view_ref("completion"),
        "query": {"filter": prefix},
        "presentation": {"mode": "popup", "width": 72, "height": 16},
        "clear_input": True,
    }
```

`find_route` is checked before the fuzzy list so that a typed `pass` jumps to
`pass:main` instead of offering a popup because `pass:unlock` also contains the
substring. `clear_input` consumes the typed prefix on the way out, so returning
to the launcher later starts from an empty line instead of the half-typed route.

The popup is another View of the same workflow, so its reference is resolved from
the mount instead of being written as `core:completion`: the manifest decides the
member id, and one workflow package can be mounted under any name.

### 3. Add the popup View and its accept command

The popup is an ordinary Picker whose query carries the typed prefix:

```toml
[views.completion.query]
type = "object"
input = "filter"
filter = { type = "string", default = "" }

[views.completion.engine]
type = "picker"
[views.completion.engine.config]
show_left_prefix = false
[views.completion.engine.config.items]
producer = "script"
[views.completion.engine.config.items.handler]
file = "scripts/completion.py"

[views.completion.keymap]
enter = "accept_route"
```

`show_left_prefix = false` keeps the popup free of the route marker that
[`left_prefix`](../reference/settings-toml.md) draws for other non-root Views.
The items script lists the same candidates the `Tab` command used. Accept
replaces the popup with the chosen route, so the route does not stack on top of
the launcher:

```python
# scripts/completion_accept.py (abridged)
operation = {"type": "navigate", "target": item["value"], "replace": True}
```

### 4. Share one candidate set

Both scripts must agree on what a "route" is, so they import one module. The
authoritative list is the suite's aliased Views, which
[`--inspect --all`](../reference/cli.md) reports:

```python
# scripts/completion_routes.py (abridged)
suite = os.environ["TFLOW_SUITE"]
owner = os.path.basename(os.environ.get("WORKFLOW_DIR") or "") or "core"  # see below
binary = os.environ.get("TFLOW_BIN") or "tflow"
result = subprocess.run(
    [binary, "-s", suite, "--inspect", "--all"],
    capture_output=True, text=True, timeout=10,
)
routes = []
for view in json.loads(result.stdout)["views"]:
    reference, alias = view["view"], view["alias"]
    # The launcher workflow owns the completion UI; alias-less Views are derived
    # Views such as sys:output, which are reachable from a command instead.
    if reference.startswith(f"{owner}:") or not alias:
        continue
    routes.append({"value": reference, "display": f"{alias} ({reference})",
                   "metadata": {"alias": alias}})
```

`TFLOW_SUITE` is set by the host for suite runs, and `WORKFLOW_DIR` for
directory workflows, so no path needs to be hardcoded. `owner` is the member id
this workflow is mounted as: read the suite manifest and match the `[workflows]`
entry whose `dir` resolves to `WORKFLOW_DIR`, because the manifest key is not
necessarily the directory name. Falling back to the directory name keeps the
script useful when no manifest can be read. Fuzzy matching then only has to test
the typed tokens against the alias and the reference, and the exact match is a
second lookup over the same list.

### 5. Make Space jump, or append

Binding `space` removes it from the editor, so a space that does not name a
route must be re-entered by re-mounting the View:

```python
# scripts/route_separator.py (abridged)
state = (request["context"].get("engine") or {}).get("state") or {}
input_text = state.get("input") or ""
parameters = request["context"].get("parameters") or {}
selected_value = (state.get("item") or {}).get("value")

match = completion_routes.find_route(input_text.strip())
if match is not None:
    operation = {"type": "navigate", "target": match["value"], "clear_input": True}
else:
    query = dict(parameters)
    query["search"] = input_text + " "
    if selected_value:
        query["__focus"] = selected_value
    operation = {"type": "navigate", "target": completion_routes.own_view_ref(),
                 "query": query, "replace": True}
```

`replace = true` re-mounts the same View, `__focus` restores the previous
selection, and carrying the current parameters keeps the aggregate's `sources`
intact. See [View Navigation & Popups](view-navigation-and-popups.md) for the
reserved query keys.

### 6. Show the route marker (optional)

A shell-like prompt marker is purely presentational and configured once for
every Picker:

```toml
[defaults.picker]
left_prefix = "$route"
left_prefix_backspace = "root"
```

`$route` renders the target View's alias or reference; any other value is a
literal marker, and omitting `left_prefix` renders nothing. `left_prefix_backspace`
decides what `Backspace` does on an empty, prefixed input line: `"parent"` returns
one level, `"root"` returns to the launcher, and unset leaves `Backspace` inert.
Both settings live in the host settings file, not in a suite manifest.

## Verification

```sh
tflow -s <suite.toml> --check
tflow -s <suite.toml> --items <launcher>:completion ''
```

The first command validates the commands, popup query, and keymaps. The second
prints the candidate list, which is what the popup will show for an empty
prefix.

## Related

- [Picker Views](picker-views.md) — item producers, aggregation, and previews.
- [Commands and Producer Scripts](commands-and-producers.md) — operation types and item bindings.
- [settings.toml and Suite Specification](../reference/settings-toml.md) — the left-prefix settings.

# Input And Navigation Model

This document defines the target input, navigation, View, and Engine boundary
for the launcher. It is a normative design document. It does not describe a
compatibility implementation and it does not introduce an extra ownership
layer between Route, View, and the host.

The central rule is:

> A Route selects one opaque View implementation. Picker, Embedded, Capture,
> and future implementations are peers. No Route, workflow module, or Engine
> implementation depends on another concrete Engine.

## Core Model

The launcher has one terminal input transport, one navigation host, and one
host-facing View protocol. It does not have one generic editor or one generic
interaction model shared by every View.

```text
Terminal
  -> InputPipeline
  -> Session / Router
  -> active View
       -> Picker implementation
       -> Embedded implementation
       -> Capture implementation
       -> another implementation
```

The main components are:

- `InputPipeline` converts terminal bytes into lossless `InputEvent` values.
- `RouteCatalog` contains immutable route identity, aliases, query schemas, and
  route-completion metadata.
- `Router` owns the navigation stack, View instances, lifecycle, and
  call/return boundaries.
- `View` is the only runtime contract visible to the host.
- Picker, Embedded, and Capture are concrete View implementations. The host
  does not inspect their concrete types after creation.
- `Session` owns the terminal adapter, outer loop, task and effect services,
  command execution, and invocation result handling.
- `Chrome` owns shared frame composition and the shared footer. It does not own
  a View's input, editor, cursor, candidates, or selection.

A View may use an editor, a PTY, a captured stream, a list, a form, or none of
these. Those are private implementation choices. The common contract covers
lifecycle, input delivery, decisions, rendering, metadata, and results only.

## Ownership And Dependencies

### Route and workflow configuration

The compiled Route definition is the stable configuration contract for one
View location. It contains:

```text
ViewRef
aliases
query schema
commands
presentation
opaque implementation payload
```

The opaque implementation payload is consumed only by `ViewFactory`. Router
passes it through but never inspects its type or fields. A Route definition
does not contain a live View, editor state, selection state,
PTY state, task handles, or a navigation stack.

The Route query contract owns query schemas, field types, defaults, input
order, parsing, and validation. `workflow::command` owns command definitions
and action preparation. These modules do not depend on Picker, Embedded,
Capture, ratatui, or terminal I/O.

### RouteCatalog

`RouteCatalog` is an immutable read-only index over compiled route metadata.
It may expose operations such as:

```rust
trait RouteCatalog {
    fn resolve(&self, selector: &str) -> Option<RouteTarget>;
    fn complete(&self, prefix: &str) -> Vec<RouteMatch>;
    fn query_schema(&self, target: &ViewRef) -> &QuerySchema;
}
```

The catalog owns route identity and route metadata. It does not own:

- live View instances;
- a navigation stack;
- completion selection;
- an editor buffer;
- Picker items; or
- terminal state.

`complete` refers to route matches, not to the item rows displayed by a
Picker implementation. A Picker may use route matches for its own route-entry
UI, but the catalog does not own the Picker's selected match or replacement
range.

The catalog cannot perform a transition. It only resolves and validates route
metadata. Router remains the only component allowed to commit navigation.

### Router

Router is the runtime navigation authority:

```text
Router
  routes: RouteCatalog
  stack: Vec<ViewInstance>

ViewInstance
  instance identity
  route location
  presentation
  lifecycle state
  call/return boundary
  Box<dyn View>
```

Router responsibilities are:

- resolve and validate structured navigation requests;
- create View instances through `ViewFactory`;
- maintain the View stack and instance identities;
- apply push, replace, call, return, and popup transitions;
- mark instances active, covered, closing, or closed;
- deliver lifecycle, input, task, tick, resize, and EOF events; and
- discard task results for closed or replaced instances.

Router does not:

- decode raw terminal bytes;
- edit a View's editor;
- perform Picker item filtering;
- own route-completion selection;
- render a View's private surface; or
- execute external effects.

Router may use generic host services while creating or driving a View. Those
services must not expose `Session`, another View's private state, or concrete
implementation types.

A transition is structurally transactional:

1. Resolve and validate the target Route.
2. Create and initialize the target View.
3. Prepare the target lifecycle and resources.
4. Commit the new stack entry and presentation.
5. Cover the previous active instance when applicable.
6. Activate the target instance.
7. Notify the source View that the transition committed.

If creation or initialization fails, the previous active stack remains in
place and the source View receives a rejection. External operations such as
starting a child process are not rollbackable; the host must explicitly clean
up a staged resource when a structural transition is rejected.

The lifecycle states mean:

```text
mounted = the instance owns its resources
active  = the instance receives user input
covered = the instance remains mounted but does not receive user input
closing = the instance is releasing resources
closed  = the instance is no longer part of the stack
```

A covered View may continue background work only when its own lifecycle policy
allows it. It must not receive active user input while covered.

### View implementation boundary

All concrete Views implement one external protocol. The protocol contains no
Picker, Embedded, or Capture type.

The composition root may know concrete implementation types:

```text
ViewFactory
  RouteDefinition + opaque implementation payload
  + narrow RouteLookup + HostServices
    -> Box<dyn View>
```

After creation, Router, Session, and Chrome see only `View`. An implementation
may internally contain a runtime, renderer, editor, PTY session, item loader,
or child process. Those internal parts are not additional host protocols.

The following dependency rule is mandatory:

```text
composition root -> concrete implementations
host             -> View contract
View             -> route/query contracts and narrow host services
implementation   -X-> another concrete implementation
workflow         -X-> Picker / Embedded / Capture
Chrome           -X-> implementation-private state
```

Picker, Embedded, and Capture are therefore peer View implementations. Picker
is not a base class or a shared feature contract for other Views.

### Session

Session owns:

- terminal mode, shutdown, and final output handling;
- reading terminal input through the single `InputPipeline`;
- global command orchestration and command execution;
- the outer event loop;
- ownership of the single `TaskRuntime` and task scheduling;
- external effect execution;
- terminal dimensions and drawing invocation; and
- invocation result adaptation.

Session does not own:

- a View's editor or cursor;
- a View's Picker items or selection;
- route-completion selection or replacement ranges;
- a generic editor for all Views;
- query parsing;
- PTY state; or
- implementation-specific input branches.

Session may host Router, but hosting Router is not Router-state ownership.
Router owns the stack and View locations.

### Chrome

Chrome is split into two host-owned presentation components:

```text
Chrome
  ContentHost
  Footer
```

`ContentHost` owns:

- application framing;
- inline and popup placement;
- content-area calculation;
- popup clearing and borders; and
- invocation of visible View rendering.

`ContentHost` does not know whether the active View is Picker, Embedded,
Capture, or another implementation.

`Footer` consumes generic data from the committed Router location and active
View metadata. It renders:

- current route location;
- View status;
- View errors; and
- effective command hints.

Neither component renders:

- a query line;
- an editor cursor;
- route-completion rows;
- Picker item rows;
- a popup-specific input row; or
- a copy of a View's input buffer.

The active View owns every cell inside the content area supplied by
`ContentHost`. A Picker therefore renders its own query line if it has one. An
Embedded View renders its own terminal surface. Capture renders its own
output.

## View Contract

The contract between the host and a created View is expressed through one
event and decision protocol. Route query data, narrow construction services,
and generic metadata are inputs or outputs of this protocol; they do not form a
second protocol for identifying or driving concrete View types. The exact Rust
representation may evolve, but the ownership and data flow are normative.

```rust
enum ViewEvent {
    Lifecycle(LifecycleEvent),
    Input(InputEvent),
    Task(TaskEvent),
    Tick,
    Resize(TerminalSize),
    Eof,
}

enum ViewDecision {
    Stay,
    Invalidate,
    Command(CommandRequest),
    Transition(TransitionRequest),
    Return(ViewResult),
    Effect(EffectRequest),
    Batch(Vec<ViewDecision>),
    Exit,
}
```

A View mutates only its private state while handling an event. Router applies
structural decisions, Session-owned services execute commands and effects, and
the host controls the outer loop. `Batch` is ordered: the host processes its
decisions in order and stops at the first failed decision.

A View must not retain `Session`, `Router`, a complete host service object, or
another View's state. It may retain narrow handles such as a task handle or an
owned process resource.

### View context

`ViewContext` is a host-provided snapshot for one lifecycle, event, or render
operation. It may contain instance identity, route location, the validated
query, presentation, terminal size, cancellation observation, and committed
host metadata.

It is not a general mutable state bag. A View must not store the context or use
it as a second owner for editor, item, selection, PTY, or task state. Any
mutable Rust reference used by the current implementation is event-local
plumbing; its host-owned fields remain subject to the same rule.

### View metadata

A View may publish generic metadata for Chrome and command presentation:

```rust
struct ViewMetadata {
    title: Option<String>,
    status: Option<String>,
    error: Option<String>,
    bindings: BindingSet,
}

struct RenderResult {
    cursor: Option<RelativeCursor>,
    metadata: ViewMetadata,
}
```

The cursor is relative to the View content area. A View with no cursor returns
`None`. ContentHost and Footer never calculate a View cursor.

View metadata must not contain implementation-specific types. If an
implementation needs to publish a current result for command evaluation, it
publishes a generic `ViewPublication` or includes the relevant value in a
`ViewResult`; it does not expose its internal runtime state.

### Host services

Host services provide external resources owned by the host:

```rust
trait HostServices {
    fn submit_task(&self, request: TaskRequest) -> TaskHandle;
    fn create_process(&self, request: ProcessRequest) -> Result<Box<dyn Process>>;
    fn terminal_size(&self) -> TerminalSize;
    fn cancellation(&self) -> CancellationObserver;
}

trait RouteLookup {
    fn resolve(&self, selector: &str) -> Option<RouteTarget>;
    fn complete(&self, prefix: &str) -> Vec<RouteMatch>;
}
```

`RouteLookup` is a read-only construction dependency. A Picker may receive a
narrow handle to it from `ViewFactory`; it cannot mutate the catalog or commit
a transition.

A service creates or manages a resource that a View may need to retain. One-shot
application operations are effects:

```rust
enum EffectRequest {
    CopyToClipboard(String),
    RunPrepared(PreparedCommand),
}
```

The distinction is:

```text
Host service = long-lived resource or host-owned facility
Effect       = ordered one-shot external operation
```

`Session` owns one `TaskRuntime` and one `EffectExecutor`. Picker, Embedded,
and Capture do not create private parallel host runtimes.

### Commands

`workflow::command` owns static command definitions, validation, and action
preparation. `Session` owns the runtime `CommandService`, which resolves a
request against the current context and coordinates the resulting Router
transition or effect. Global commands are handled by this service before local
View input. A local View binding may identify a configured command, but the
View does not need to own the command evaluator or a complete Config object.

The generic flow is:

```text
InputEvent
  -> Router reserves a global binding, or delivers it to the View
  -> View handles a local binding and returns CommandRequest
  -> Session-owned CommandService resolves and evaluates the command
  -> Router applies navigation, or Session-owned EffectExecutor applies the effect
```

A `CommandRequest` contains a command reference and the generic context needed
for evaluation:

```text
CommandRequest
  command reference
  source View location
  current View publication, if any
  current query/parameter values
  raw binding input, if required by the command contract
```

`ViewPublication` and `ViewResult` are one-way host/workflow data. They are
consumed only by the command or return contract that requested them. A View
must not use another View's publication as an implicit cross-View interface,
and publication values must not expose concrete implementation state.

Static command preparation remains in `workflow::command`; runtime command
orchestration remains in Session-owned `CommandService`. Neither is
reimplemented inside a Picker, Embedded, or another concrete implementation.

A local binding can mean any implementation-specific action. Router does not
interpret it as Picker, Capture, or Embedded behavior. It only arbitrates the
physical key and delivers the event to the active View.

Passthrough is also a View-local behavior. An Embedded View may consume a
binding and forward its original `InputEvent.raw` bytes to its PTY. No other
View needs to know what passthrough means.

### Task correlation

A task result is correlated independently at two levels:

```text
Router correlation:
  ViewInstanceId

View correlation:
  TaskId or request generation owned by that View
```

Router discards results for an instance that has been closed or replaced. The
View rejects results that no longer match its own request generation. A shared
mutable `ViewContext.revision` must not be used as the only identity for every
kind of work, because a publication, status update, or resize may not mean
that an item request is stale.

A task request binds ownership when it is submitted. The View obtains its
instance identity from `ViewContext`; `TaskRuntime` carries that identity into
the result:

```rust
struct TaskRequest {
    owner: ViewInstanceId,
    task: TaskId,
    generation: u64,
    operation: TaskOperation,
}

struct TaskEvent {
    instance: ViewInstanceId,
    task: TaskId,
    generation: u64,
    value: Value,
}
```

### Navigation decisions

Views never mutate the Router stack directly. They return structured requests:

```rust
struct NavigationRequest {
    target: ViewRef,
    query: ParsedQuery,
    presentation: ViewPresentation,
}

enum TransitionRequest {
    Push(NavigationRequest),
    Replace(NavigationRequest),
    Call {
        request: NavigationRequest,
        continuation: Continuation,
    },
    Return(ViewResult),
}
```

Router validates the target and query before committing a transition. It does
not receive an arbitrary editor buffer, a Picker candidate index, or a route
completion replacement range.

## Input Contract

### Lossless transport

`InputPipeline` is the only terminal-byte decoder. It preserves raw bytes even
when a key is decoded:

```rust
enum InputEvent {
    Key { key: Key, raw: Vec<u8> },
    Paste { text: Option<String>, raw: Vec<u8> },
    Bytes(Vec<u8>),
    Eof,
}
```

The interpretation belongs to the active View:

```text
Picker:
  decoded keys and valid paste text -> editor or Picker action

Embedded:
  raw bytes -> child PTY, after any Embedded-local binding

Capture:
  only the inputs required by its own actions
```

A decoded key retains its original bytes so that an Embedded implementation
can forward them without reconstructing terminal sequences. A key or sequence
that cannot be decoded remains available as `Bytes`.

The common input protocol does not contain `InputFocus`,
a legacy engine-specific buffer target, or a generic editor target. Those were
legacy host concepts. A View that needs editing owns its editor; a View that needs raw
input consumes raw bytes.

### Binding ownership and precedence

`workflow::command` owns global command definitions. Session-owned
`CommandService` executes them. Router is the single physical-key arbiter.
Each View owns its local binding table and local input policy.

```text
terminal bytes
  -> InputPipeline
  -> Router global binding arbitration
  -> active View::event(Input(InputEvent))
  -> ViewDecision
  -> Router transition processing / Session effect processing
```

The precedence order is:

```text
Router global binding
  > active View local binding
  > active View input fallback
```

Router does not decode or interpret the local binding's semantic target. A
local binding may mean editor deletion, route completion, Picker selection,
Capture action, Embedded cancellation, or Embedded passthrough.

There is no second Session input layer created to expose Picker behavior to
another implementation.

## Route And Query Contract

### Route query

Every View may define a query schema. Query is a Route contract, not a Picker
editor contract.

```text
QuerySchema
  fields, types, defaults, input_order, validation

ParsedQuery
  target-bound, schema-validated parameter values
```

A `ParsedQuery` is immutable and contains only structured route data:

```rust
struct ParsedQuery {
    target: ViewRef,
    schema: QuerySchemaId,
    values: ParameterValues,
}
```

Its invariants are:

- `target` is the canonical target named by the navigation request;
- `schema` is the schema for that target; and
- `values` have passed the target schema and validation rules.

`ParsedQuery` does not contain:

- an editor buffer;
- a cursor;
- a Picker completion selection;
- a source route selector; or
- a presentation-specific input row.

A View may render a canonical editable representation of its initial values,
but that is an implementation choice.

If an invocation needs to represent incomplete user text, it must use an
explicit draft/input type at the interaction boundary. It must not make
`ParsedQuery` ambiguously represent both invalid draft text and valid typed
values.

### Picker editor and route entry

Picker owns its own private input state when it provides an editor:

```text
PickerEditorState
  raw text
  cursor
  editor revision
  parse diagnostic
  optional route-completion state
```

The editor is one way to construct a query. It does not redefine the Route
query contract.

A Picker with route-entry enabled may interpret its editor as:

```text
route selector + target query text
```

For example:

```text
Picker editor: "sys de"
  -> selector: "sys"
  -> RouteCatalog resolution: "sys:main"
  -> target query text: "de"
  -> target QuerySchema parsing
  -> ParsedQuery
  -> NavigationRequest
  -> Router transition
```

Picker owns selector/query splitting and route-completion UI. Router owns
resolution validation and transition commit. The Router receives only the
structured target and ParsedQuery.

Route entry is a construction policy, not a feature required by other Views:

```text
default Picker: route entry may be enabled
ordinary Picker: route entry may be disabled
Embedded:        no route completion unless its own implementation provides it
Capture:         no route completion unless its own implementation provides it
```

An Embedded implementation may implement its own route-entry UI. It does not
need to use Picker's editor or completion state.

### Picker route completion

Route completion is Picker-private state. It is not Router state or a public
selection model.

Picker may:

1. inspect the cursor position in its own editor;
2. query immutable RouteCatalog route matches;
3. filter and rank those matches according to its own UI policy;
4. store a selected match and replacement range privately; and
5. handle Tab, Up, Down, Enter, and Escape.

Replacing `sy` with `sys ` changes only Picker state. It does not mutate the
Router stack. Navigation happens only after Picker has constructed a valid
`NavigationRequest`.

Completion state is tied to:

```text
ViewInstanceId
Picker editor revision
route match list
selected index
replacement range
```

An accepted completion edit is applied only when the source instance and
editor revision still match. Other Views do not receive this state.

### Picker items and selection

Picker item loading, item filtering, item selection, and preview are also
Picker-private implementation state:

```text
PickerRuntimeState
  item request identity
  current item list
  selected item
  preview state
  loading state
```

These are not shared item or selection semantics and are not part of the
generic View contract. A different View implementation may display no items, a
tree, a form, a PTY, or an external UI process.

A Picker may return a generic `ViewResult` containing its selected output. The
result contract is shared; the item model and selection mechanics are not.

## Concrete View Implementations

### Picker

A Picker implementation may own:

- an editor and cursor;
- query parsing through the Route's QuerySchema;
- route selector/query mapping;
- route completion state;
- item loading and stale-result handling;
- item selection and preview; and
- rendering of its complete query/list/preview surface.

Picker must not duplicate query schema semantics. It uses the workflow
parameter contract to parse and validate its editor text.

An invalid editor draft remains in Picker state. Picker exposes a diagnostic
through generic View metadata and does not send an invalid ParsedQuery to
Router.

### Embedded

An Embedded implementation may own:

- a child process;
- a PTY;
- a terminal screen;
- raw input forwarding;
- process completion; and
- its own result or command protocol.

It receives the same lossless `InputEvent` as every other View. It may forward
raw bytes after handling an Embedded-local binding. It never receives a
generic editor solely because Picker has one.

An Embedded implementation may be a complete opaque terminal View. If it must
cooperate with launcher-owned route/query/command semantics, it needs an
explicit structured child protocol. Raw PTY forwarding alone does not imply
Picker semantics.

### Capture

A Capture implementation is output-oriented. It may consume initial
structured query values, capture output, expose its own actions, and return a
result. It does not receive a generic editor or Picker item model.

### Peer implementation rule

Picker, Embedded, and Capture are peers:

```text
Picker    -X-> Embedded internals
Embedded  -X-> Picker internals
Capture   -X-> Picker internals
workflow  -X-> all concrete implementations
Chrome    -X-> all concrete implementations
```

Only their shared View contract, Route contract, and generic host services are
common.

## Rendering And Footer

ContentHost calculates the area in which a View renders:

```text
inline:
  Chrome frame
  -> active View content area
  -> active View renders its complete surface

popup:
  parent View surface
  -> popup frame
  -> child View content area
  -> child View renders its complete surface
```

Presentation changes geometry only. It does not change View input, lifecycle,
task, command, or result semantics.

Picker owns its query line and cursor in both inline and popup presentations.
Embedded owns its PTY screen in both presentations. Capture owns its output
surface.

The footer is assembled from committed Router location and generic View
metadata:

```rust
struct FooterModel {
    location: ViewLocation,
    title: Option<String>,
    status: Option<String>,
    error: Option<String>,
    bindings: BindingSet,
}
```

The footer must not receive a View's editor text, query draft, completion
state, Picker items, or cursor coordinates.

The footer displays committed location, not uncommitted Picker input. While a
Picker editor contains `sys de`, the footer continues to show the current
committed View until Router commits the target transition.

Footer layout owns clipping and command-hint overflow. It must not reserve a
fake editor row when the active View has no editor, and it must not allow
status, errors, or command hints to overlap.

## Event And Lifecycle Rules

The host loop is:

```text
Session reads terminal bytes
  -> InputPipeline emits InputEvent
  -> Router arbitrates global bindings
  -> active View receives ViewEvent::Input
  -> View returns ViewDecision
  -> Router applies transitions
  -> Session executes effects and commands
  -> task results are correlated and delivered
  -> ContentHost renders visible Views
  -> Footer renders generic FooterModel
```

For a normal View event:

```text
View mutates only private state
  -> View returns a decision
  -> host commits the decision
```

A View must not clear or replace private input in anticipation of an
uncommitted navigation. Failed navigation leaves the source View's private
state intact and delivers a structured rejection.

Lifecycle callbacks must be idempotent with respect to the lifecycle contract:

```text
Mounted
Activated
Covered
Closing
Closed
TransitionCommitted
TransitionRejected
```

A View may keep covered resources alive when its policy requires it, but
covered input must not be delivered as active input. Background task results
must be correlated with the original View instance and View-owned generation.

## Required Sequences

These sequences are behavioral acceptance criteria for the target model.

### Normal Picker editing

```text
Key('d')
  -> InputPipeline emits Key('d') with raw bytes
  -> Router delivers the event to Picker
  -> Picker editor changes
  -> Picker parses the current View query
  -> Picker stores typed values or a diagnostic
  -> Picker schedules item work with its own request generation
  -> Picker returns Invalidate
```

The host does not edit Picker state and does not render the Picker query line.

### Picker route completion

```text
Key('s') / Key('y')
  -> Picker editor contains "sy"
  -> Picker queries RouteCatalog route matches
  -> Picker stores private completion state

Key(Tab)
  -> Picker replaces the selector in its editor
  -> Router stack remains unchanged

Key('d') / Key('e')
  -> Picker editor contains "sys de"
  -> Picker resolves "sys"
  -> Picker parses "de" using target QuerySchema
  -> Picker creates NavigationRequest with ParsedQuery
  -> Router validates and commits the transition
```

If target query parsing fails, Picker keeps its draft and reports a diagnostic.
Router is not called with an invalid request.

### Embedded popup

```text
Router creates an Embedded ViewInstance with popup presentation
  -> Embedded receives Mounted
  -> Embedded creates and owns its PTY if required
  -> ContentHost gives Embedded the popup content area
  -> Embedded renders its terminal surface
  -> InputPipeline emits Key or raw Bytes
  -> Router delivers the unreserved event to Embedded
  -> Embedded handles its binding or forwards raw bytes
  -> Footer renders only generic committed metadata
```

### Command return

```text
View binding identifies a local command
  -> View returns CommandRequest
  -> workflow::command prepares the configured action
  -> Navigate/Call becomes a structured Router transition
  -> Return becomes a ViewResult
  -> Run/clipboard becomes an EffectRequest
  -> Session-owned EffectExecutor executes the effect and reports success or error
```

Return handlers and Call continuations remain part of the generic command and
navigation contract. They must not be reimplemented separately by Picker and
Embedded.

### Task completion

```text
View requests a task
  -> Session-owned TaskRuntime schedules it with owner instance and generation
  -> TaskRuntime emits a result tagged with ViewInstanceId + task generation
  -> Router discards results for closed/replaced instances
  -> Router delivers valid TaskEvents to the instance
  -> View rejects stale generations
  -> View applies the result to its private state
  -> View returns Invalidate or another generic decision
```

## Dependency Rules

The target dependency direction is:

```text
Config
  -> RouteCatalog

Route query contract
  -> QuerySchema / ParsedQuery semantics

workflow::command
  -> static command definitions, validation, and action preparation

Session-owned CommandService
  -> runtime command resolution and orchestration

Session-owned EffectExecutor
  -> one-shot external effects

ViewFactory
  -> concrete Picker / Embedded / Capture construction

Router
  -> RouteCatalog
  -> ViewFactory
  -> opaque View instances and transitions

Session
  -> InputPipeline
  -> Router
  -> TaskRuntime
  -> CommandService
  -> EffectExecutor
  -> terminal and invocation adapters

ContentHost
  -> View placement and rendering

Footer
  -> Router location and generic View metadata
```

The following dependencies are forbidden:

```text
workflow -> Picker / Embedded / Capture
Picker -> Embedded internals
Embedded -> Picker internals
Capture -> Picker internals
Router -> concrete implementation types
Session -> implementation-private state
Chrome -> editor, cursor, Picker items, PTY state
ParsedQuery -> editor or completion state
```

Low-level editor helpers may remain inside Picker. PTY helpers may remain
inside Embedded. Capture process and stream helpers may remain inside Capture.
None become shared application state merely because more than one View has
input or output.

## Target Invariants

The model is complete only when these invariants hold:

- There is one `InputPipeline` for terminal input.
- There is one authoritative Router for the production runtime.
- All concrete Views receive the same lossless `InputEvent` envelope.
- Router owns the stack, View locations, lifecycle, and transitions.
- Session owns terminal hosting, task scheduling, command orchestration, and
  effect execution; workflow owns static command preparation.
- No Session field owns a View-private editor, cursor, completion, selection,
  Picker item list, or PTY screen.
- Picker is the only owner of Picker editor, route-entry, item, selection, and
  preview state.
- Embedded is the only owner of its child process, PTY, and terminal screen.
- Capture and Embedded do not receive an editor solely because Picker has one.
- `ParsedQuery` is route-bound and schema-validated and contains no editor
  state.
- Router receives structured NavigationRequests, never arbitrary editor text.
- Global binding arbitration has one defined precedence order.
- Local binding semantics remain inside the active View.
- Configured command actions use one workflow preparation path and one
  Session-owned runtime CommandService.
- Passthrough preserves original terminal bytes.
- Task results are checked by both instance identity and View-owned generation.
- A View renders its complete surface inside the area supplied by ContentHost.
- Footer consumes only generic committed location and View metadata.
- No concrete implementation is imported by another implementation or by
  workflow modules.
- Replacing the implementation selected for a Route does not require changes
  to Router, Session, Chrome, or unrelated Views.

## Implementation Note

This document defines the destination boundary, not the current completion
status. A migration may temporarily retain legacy Session, runtime, or Chrome code,
but such code is compatibility residue and must not change the ownership rules
above.

Implementation work should migrate one contract at a time:

1. Make the host-facing View protocol authoritative.
2. Make Router the only production navigation owner.
3. Make InputPipeline the only terminal input transport.
4. Move editor and completion state into the concrete Picker View.
5. Move task and effect execution behind one host service boundary.
6. Connect command preparation and continuation through one generic path.
7. Preserve Chrome as ContentHost/Footer without implementation-specific
   branches.
8. Add a second independently implemented View for the same host contract,
   such as a structured Embedded View, before removing compatibility code.

The success criterion is ownership and replaceability, not moving code between
files or reducing line count. A new View implementation is acceptable only
when it can be introduced at the composition root and used through the existing
Route, View, Router, Session, and Chrome contracts.

## Current Implementation Gap

The current migration status against the 8 implementation steps is:

- Steps 1–5 (Authoritative View protocol, Router ownership, InputPipeline transport,
  Picker-owned editor/completion, and Task/Effect host service boundaries): **Completed**.
  The production entrypoint (`app::App`) runs exclusively on `ProtocolSession` and `Router`.
- Step 6 (Command preparation and continuation): **Completed**.
  `ProtocolCommandService` resolves commands and coordinates Router transitions/effects,
  with legacy execution contracts, dead process abstractions, and monolithic session shims
  physically eliminated from the codebase.
- Step 7 (ContentHost and Footer separation): **Completed**.
  `ContentHost` owns framing, inline/popup layout, popup borders/clearing, and View
  rendering invocation. `FooterRenderer` renders committed location, status, errors,
  and command hints. The protocol draw path uses no implementation-specific Chrome branches.
- Step 8 (Second independent View and legacy cleanup): **Completed**.
  `Embedded` and `Capture` views are fully operational peer `View` implementations.
  Legacy `AppSession` and `src/session/` code (~11,265 lines) have been completely decommissioned
  and removed from the codebase. The application and all tests now run 100% on the single-track
  protocol runtime.

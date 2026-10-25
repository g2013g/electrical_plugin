# Host capabilities — Open CAD Studio v2026.36.0 (plugin API 5)

The ABI authority for `opencad.electrical`. Everything here was read out of the
binaries or measured against the running host, not taken from prose.

Supersede this file rather than silently editing it when the host moves.

> ⚠ Historical snapshot (v2026.36 / plugin API 5): the `tools/OpenCADStudio-v2026.36-windows-x86_64-portable.exe` binary it referenced was removed in the 2026-10-18 workspace cleanup; the `--check-manifest` example now points at the installed host.

## Host version record (from the portable exe)

| Field | Value |
| :--- | :--- |
| `ocs_version` | `2026.36.0` |
| `ocs_plugin_api_version` | `0.2.1` |
| `api_version` | `5` (min supported `2`) |
| `acadrust_version` | `0.5.4` @ rev `5b56571a190e7a17c8f12d36390d2938b0fb72f7` |
| `rustc_version` | `rustc 1.98.1 (48a229cea 2026-09-01)` |

Verify with the shared script rather than by grepping:

```bash
python ../.agents/skills/opencad-api-upgrade/scripts/host_version.py <host.exe>
python ../.agents/skills/opencad-api-upgrade/scripts/host_version.py \
    "$APPDATA/OpenCADStudio/plugins/opencad.electrical/el_plugin.dll"
python ../.agents/skills/opencad-api-upgrade/scripts/host_version.py --check-manifest \
    "C:/Program Files/Open CAD Studio/OpenCADStudio.exe" \
    "$APPDATA/OpenCADStudio/plugins/opencad.electrical/plugin.toml"
```

Last check: `ocs_plugin_api_version(): 5` compiled into the DLL, and
`manifest gate check: PASS`.

## Load gate

Applies to any plugin with `api_version >= 4` (`src/plugin/external.rs`):

| Gate | Rule | On failure |
| :--- | :--- | :--- |
| API range | `2 <= api_version <= 5`; runtime ceiling lowerable via `OCS_PLUGIN_MAX_API_VERSION` | plugin skipped, silently |
| `rustc_version` — **required** | token-for-token, case-insensitive equality with the host's `rustc --version`. Absent **or empty** fails. | `Plugin built with unknown, host requires … - rebuild required` |
| `acadrust_source` — optional | absent passes; if declared, the trailing 40-char commit must match | `Plugin built for acadrust @… but this host uses @…` |

A **second, independent** check reads the DLL's exported
`ocs_plugin_api_version()` symbol, so the manifest and the compiled binary are
validated separately and can disagree.

This plugin leaves `acadrust_source` **undeclared**, so a DLL built against a
different acadrust still loads — a latent crash rather than a caught error. The
build pins the host's exact rev through the workspace
`[patch."https://github.com/HakanSeven12/cadcodec.git"]`, and that rev must be
moved by hand when the host's moves.

### Gate triad — moves together

1. `crates/el-plugin/plugin.toml` (`api_version`, `rustc_version`)
2. `rust-toolchain.toml` — the toolchain yielding the matching `rustc --version`
3. `ocs-electrical/Cargo.toml` — the `[patch]` acadrust rev

## Live probe results — `EL_PROBE`

Captured against the host above; re-run whenever the host changes.

```text
[PASS] add_entity(Insert): accepted at 0x62; block name resolves in the block table = false
[PASS] xdata values round-trip: max tested 20000 Real values (≈160000 bytes)
[PASS] xdata long string: 255 chars → read back 255
[PASS] xdata long string: 4096 chars → read back 4096
[PASS] xdata long string: 65536 chars → read back 65536
[PASS] xdata long string: 262144 chars → read back 262144
[PASS] layer auto-create on add_entity: layer "EL_PROBE_LAYER" exists
[PASS] embedded xdata through add_entity: record visible on the committed entity = true
[PASS] scan→index sees embedded record: settings recovered = true
[PASS] settings record round-trip: revision Some(1)
[PASS] sibling app-id records are invisible: an OCSWH record written on 0x69
       is not returned by the OCSEL scan = true
```

Two consequences worth stating plainly:

- **XDATA payload is not a practical constraint.** 262 144-character strings and
  20 000 reals round-trip intact. Records here are small by comparison.
- **Layer auto-creation works on `add_entity`.** A novel layer name is
  registered with a real handle, so commands never need a layer-creating step.

## The block-definition finding

`EL_PROBE BLOCK` then `EL_PROBE BLOCKCHECK` (two dispatches — the only way a
persistence question can be answered) leaves this verdict:

```text
[FAIL] block definition survived the dispatch: false
[PASS] insert survived the dispatch: 1 entit(ies)
block records in the document: 2
VERDICT: no block definitions, for two independent reasons, both confirmed —
(1) `document_mut()` hands an out-of-process plugin a local cache, so a table
insert never reaches the host; and (2) a BlockRecord is a set of handles (BLOCK,
ENDBLK, and each member entity) that only the host can allocate, so even a
persisted record would define an empty block. Symbols stay primitives — see
geometry.rs.
```

The two pre-existing records are the host's own `*Model_Space` and
`*Paper_Space`. An `EntityType::Insert` is *accepted* by `add_entity` and
survives the dispatch, but its block name never resolves, so it renders nothing —
which is why symbols are drawn as primitives with the record attached as XDATA.

If this verdict ever flips to PASS, the symbol layer in `geometry.rs` should be
re-planned around real blocks with attributes.

## `NullHost` constraints during interactive events

Interactive callbacks (`on_point`, `on_object_pick`, `on_enter`) run against the
host's `NullHost` stub:

| Call | Behaviour |
| :--- | :--- |
| `document()` / `document_mut()` | **panics** |
| `add_entity` / `add_entities` | silent no-op, returns null handles |
| `write_record` / `remove_record` | fails silently |
| `read_record` | always `None` |
| `push_info` / `push_output` / `push_error` | dropped |
| `document_path` | `None` |
| `ensure_plugin_state_any` | panics |

Hence: capture at dispatch, defer the output
(`state::emit` → the registered `PluginRequestSender`), and commit new entities
through `CommandStep`.

## Serve-mode (`--serve`) notes

- The host writes a `{"ready": true}` banner on startup. Not draining it first
  makes every later read off by one.
- `push_info` / `push_output` are **not** forwarded to stdout. Only a command
  *error* surfaces, as `ok: false` with a message, and it can surface on the
  response to the next op.
- **Only the last error pushed is surfaced.** Two `push_error` calls in one
  command and the first is lost — which is why `EL_WIRENO` merges its "nothing to
  number" reason and its "you need a ladder" hint into a single message.
- **A session that never ran `new` or `open` is not a normal document.** Record
  writes to existing entities can be refused in that state while `add_entity`
  with embedded XDATA still succeeds. Every behavioural check therefore starts
  from an explicit `new` or `open`.
- Therefore every succeed-path check must assert on a side effect — an entity
  count, a file on disk, or a report the command wrote itself.
- A saved drawing stays locked by the host, so files can only be read after the
  session that wrote them has exited.
- In a saved DXF an XDATA pair reads as `1000\nKEY\n1000\nVALUE` — a group code
  sits between key and value, so matching `KEY\nVALUE` never matches. A bare
  substring search for a key can also hit a longer key containing it: `SHOWREF`
  contains `WREF`. `tests/serve_driver.py::xdata_pair` does this correctly.

All of these are handled in `tests/serve_driver.py`.

### A trap in the *test* harness, not the plugin

An interactive command freezes the command line until it finishes. A test helper
that follows a command with a bare `EL_INFO` therefore starts an interactive pick
and swallows every later command in the phase, turning one assertion into a phase
full of misleading failures. The follow-up probe must be a **protocol op**
(`{"op": "entities"}`), never a command that can go interactive.

## Structural limits found while building Phase 1

Three of these cost real time; each is confirmed against the running host, not
inferred from the API listing.

### `CommandStep` carries one entity, and nested requests hit `NullHost`

To place a ladder — a frame of bus lines, rungs and references — the obvious
approach is to add them all and commit once. Neither route works:

- `CommandStep::Commit`/`CommitAndEnd` take a single `EntityType`, and the host's
  internal `CmdResult::CommitEntities` (which commits a whole batch in one undo
  window) is not part of the plugin contract.
- A `PluginRequest` sent *from inside* a callback is serviced with `NullHost`, so
  `AddEntities` silently no-ops and `write_record` fails silently.

What does work: queue the entities and send them from a **background thread**
about 60 ms later, once the host has returned to its own event loop. That is
`state::spawn_flush`, and it delivers a whole multi-entity placement in one undo
window.

### A write is not visible to the same dispatch

`host.document()` returns a snapshot from the **start** of the command. After
`write_record` succeeds, re-reading the document in the same dispatch still shows
the old value, so any code of the shape "write, then re-read to render" produces
nothing — and produces it silently, because the write *did* work.

This is why `EL_WIRENO` renders wire-number labels from an explicit
`(handle, number)` map rather than from a re-read. Verified directly: a wire's
`WREF` update persisted to the saved DXF while a same-dispatch re-read showed it
empty.

### `erase_entities` and `write_record` refuse locked layers, silently

```rust
// src/app/plugin_host.rs
pub fn write_record(&mut self, handle: Handle, record: ExtendedDataRecord) -> bool {
    if self.app.tabs[self.tab].scene.is_layer_locked(handle) { return false; }
    ...
}
```

`remove_entity` has the same guard through `erase_entities`. Both return `false`
with no explanation, so a plugin that counts return values leaves the user with
entities that will not go away and no reason why. Every removal and write in this
plugin now reports a refusal, naming the layer.

## Multi-entity objects, and why the index folds them

The contract offers no way to group entities, so an object drawn as several
entities is grouped by **the record it puts on each of them**: every part carries
an identical record, and `el_id` is the identity.

This is not a stylistic choice. An index keyed by entity handle makes a
four-part coil read as four devices, and the consequences are all silent:

- a drawing-wide retag allocates the coil four tags and the last write wins, so
  it ends up as `K13` while `K10`–`K12` are written and overwritten;
- deleting the device removes one part and leaves three orphans carrying a
  record that no longer resolves;
- a cross-reference lists the same contact four times.

Measured, not theorised: `EL_RETAG START=10` on a drawing with two coils and a
breaker produced `K10` and `K13` before the fold, and `K10`/`K11`/`Q10` after.

The same folding applies to ladders. Wires are still keyed by handle, which is
valid only while a wire is exactly one polyline; `index.rs` has a test pinning
that invariant, because the deferred wire-gap work would break it.

## A placed symbol must record where it went

A symbol drawn as primitives has **no recoverable anchor**: a coil's first drawn
point is a corner of its rectangle, not its origin, and the rotation is not in the
geometry either. So the insertion point and rotation are part of the record — the
same thing a block insert carries, and for the same reason.

Everything downstream depends on it: a pin's drawing position is
`origin + rotate(local pin offset)`, which is what lets a wire say `FROM=K1.A1`
and record what it reaches, and what lets a wire drawn by eye resolve to the
nearest connection point. Without it a from/to report could only ever say
"unconnected".

## A moved device must take its wires with it

A wire names the pin it reaches, and is drawn as a polyline. Those two facts are
independent, so moving a device leaves the record correct and the geometry wrong —
a wire that claims to reach a pin it visibly does not touch. Re-seating closes that
gap, and it has to be driven from the **new** placement rather than a re-read: the
plugin's document snapshot still holds the old position for the rest of the
dispatch.

Verified end to end: moving a coil by `DX=40` and rotating it 90° while moving a
terminal to `200,300` re-seats one wire from `(57.5,196)→(62.5,204)` to
`(95,200)→(207.5,300)` — the coil's rotated pin and the terminal's moved
connection, to the hundredth of a millimetre.

## Standing hard limits

- **No block definitions** — see above.
- **No dockable panels or dialogs.** The UI vocabulary is exactly seven
  `RibbonItem` shapes: `Tool`, `LargeTool`, `Dropdown`, `LargeDropdown`,
  `LayerComboGroup`, `PropertiesGroup`, `StyleComboGroup`.
- **No text input in interactive commands.** `InteractiveEvent` is `Point`,
  `Enter`, `ObjectPick` only — a tag, part number or rating must arrive as a
  command-line token or through a ribbon dropdown dispatching `CMD KEY=VALUE`.
- **One entity per `CommandStep` commit**, hence the deferred mechanism for
  anything cosmetic.
- **Command prefixes do not route.** `command_prefixes` feeds autocomplete and
  the plugin manager only; `PluginManager::dispatch` offers *every* command to
  *every* plugin in load order until one handles it. A distinct prefix is
  collision avoidance, not routing — and a plugin can shadow a built-in command
  by returning `true` for it.
- **Handles need an explicit `0x` prefix.** Reference designators like `C1`,
  `D1`, `B1`, `F1` are valid hexadecimal strings, so a bare token is ambiguous.
- **`push_undo` opens a recording window**, it does not snapshot. Call it
  *before* the first mutation or the change is attributed to the wrong step.
- **64 MiB per IPC frame.** Batch `AddEntities` payloads count against it.

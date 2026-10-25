# OpenCAD Electrical

Electrical control-schematic design for [Open CAD Studio](https://github.com/HakanSeven12/OpenCADStudio) —
ladders and line references, wire numbering, component tagging and cross-references,
terminals, cables, reports and audit. Modelled on AutoCAD Electrical's workflow;
built as an out-of-tree plugin against the host's stable `ocs_plugin_api`.

The host is kept **pristine**. Everything lives here.

```
host:      Open CAD Studio v2026.40.1   (plugin API 7, min supported 3)
plugin:    opencad.electrical 0.1.3
prefix:    EL_            app id: OCSEL          ribbon order: 70
```

Contract notes (host v2026.40.1): `plugin.toml` must declare `acadrust_source`
matching the host fingerprint (the absent-passes fallback was removed), the
build must consume the same codec rev via the checkout's `ocs_plugin_api` path
dependency, and the build toolchain must equal the host's `rustc --version`
token-for-token. Release-provenance warning: the v2026.40.0 Windows build
(issue #1612) was not built from the release commit and rejects API-7 plugins —
only v2026.40.1+ desktop builds are valid targets.
Current ABI authority: `../ocs-wiring-harness/docs/host-capabilities-v2026.40.1.md`.

## Layout

```
crates/el-core/     host-free domain logic — plain `cargo test`, no host, no acadrust
  model.rs            records, settings (the WD_M equivalent), tag/wire-number formats
  codec.rs            ordered key/value record ↔ XDATA, with schema migration
  index.rs            resolved connectivity index, duplicate detection, tag/wire allocators
crates/el-plugin/   cdylib adapter — the only crate that sees ocs_plugin_api
  lib.rs              manifest, ribbon, dispatch
  codec.rs            ElRecord ↔ ExtendedDataRecord bridge, document scan
  geometry.rs         entity builders (symbols are primitives — see below)
  hostops.rs          token classification, picking, dispatch-time capture
  state.rs            session state and deferred command-line output
  probe.rs            EL_PROBE capability self-test + the block-definition spike
  cmds/               one module per command family
tests/serve_driver.py   behavioural suite against a live host
docs/                   host ABI authority, user manual
```

The `el-core` / `el-plugin` split is the host's own **Layer C** rule: the engine
crate imports neither `iced`, `acadrust`, nor `OpenCADStudio`, so all domain
logic is testable without a CAD host and is insulated from ABI churn.

## Commands

| Command | Purpose |
| :--- | :--- |
| `EL_SETTINGS` | Show this drawing's electrical settings. |
| `EL_SETTINGS KEY=VALUE …` | Set them — see the keys below. `CLEAR` removes the record. |
| `EL_WIRETYPE [LIST]` | List the drawing's wire types (or the built-in set). |
| `EL_WIRETYPE ADD\|EDIT <NAME> [COLOR=] [SIZE=] [PATTERN=] [NONUMBERED]` | Define a conductor class and the layer it draws on. `DEL`, `CLEAR`. |
| `EL_LADDER [x,y] [WIDTH=] [SPACING=] [RUNGS=] [REF=] [INC=] [PHASE=]` | Place a ladder. Bare = pick the top-left corner. `NORUNGS`/`NOBUS`/`NOREF` suppress parts; `CLEAR` removes every ladder. |
| `EL_RUNG [ladder] ADD=<n> \| RUNGS=<n> \| SPACING= \| REF= \| INC=` | Add or revise rungs. The frame is rebuilt from the record. |
| `EL_WIRE [x1,y1 x2,y2] [TYPE=] [GAUGE=] [COLOR=] [NET=]` | Draw a wire. Bare = pick both ends. Snaps to the dominant axis, and a picked start snaps onto a rung. |
| `EL_WIRENO` | Assign wire numbers — sequential or reference, per `WIREMODE`. `CLEAR` removes the drawn labels; `FIX`/`UNFIX <wire>` pin a number. |
| `EL_WIREGAP [<wire\|ALL>] [GAP=mm] \| CLEAR [<wire\|ALL>]` | Draw crossing gaps: a gap-flagged wire is rendered as several polylines sharing one `el_id`, with a hole centred on every point another wire properly crosses (junctions are never cut). Bare = report. `GAP=` sets the drawing-wide gap size (also `EL_SETTINGS WIREGAP=`). `CLEAR` draws the wires whole again. |
| `EL_TRIMWIRE <wire> <x,y>` | Move the wire's nearer end to the point (axis-snapped); the moved end re-resolves to a node within snapping distance or goes dangling, and `EL_AUDIT` reports it. |
| `EL_COMPONENT [symbol\|family] [tag] [x,y] [ROT=deg] [KEY=VALUE …]` | Place a device from the library, auto-tagging it unless a tag is given. `SWAP <tag> <new-symbol>` replaces a placed device's symbol, keeping the tag, placement, catalog and child/parent role; attached wires re-seat onto the new pins. `K` places the family's first parent. |
| `EL_LINK <child> <parent>` | Attach a device to a parent, or `CLEAR <child>` to detach. Bare = list every relationship. |
| `EL_XREF [ALL \| <tag>]` | Write cross-reference annotations. `CLEAR` removes them. |
| `EL_RETAG [ALL \| <family>] [START=n] [DRYRUN]` | Renumber device tags. `FIX`/`UNFIX <tag>` pins one. |
| `EL_LIB [LIST \| SHOW <name> \| FAMILIES \| PATH [LOCAL\|GLOBAL\|<file>] \| EXPORT \| RELOAD]` | Inspect and switch the symbol library. |
| `EL_TERMINAL [kind] [strip] [x,y] [NO=] [PART=] [DESC=] [LINK=]` | Place a terminal block. Kinds: screw, spring, ground, fused, disconnect, multi, test. `NUMBER` renumbers every strip; `REORDER <strip> <no…>` renumbers in a stated order (jumpers remap with their terminals); `PARTITION <d> <name\|CLEAR>` puts a terminal in a partition group; `JUMPER <a> <b>` bridges two terminals of a strip into one circuit (nets merge across it; `JUMPER CLEAR <a>`, `JUMPER LIST`); `LIST`, `CLEAR`. |
| `EL_TITLEBLOCK [x,y] [TITLE= DRAWN= DATE= APPROVED= SHEETS=]` \| UPDATE \| CLEAR \| SHOW` | The drawing frame + title table. **Fields are a render**: sheet/drawing/inst/loc/revision read live from `EL_SETTINGS`, the `OF n` count from the project register — `UPDATE` re-renders after any change (the plugin's title-block update). The frame follows the drawing's extents. |
| `EL_MINSERT <symbol\|family> <count> [REF=<rung>] [x,y] [DY=<mm>]` | **Multiple insert**: one symbol, N placements down consecutive rungs (or `DY=` spacing), each with its own allocated tag — the batch form of `EL_COMPONENT`. |
| `EL_EXPORT WH [path.csv] [FORCE]` | **The harness bridge**: writes the wirelist CSV `WH_IMPORT` consumes (`net,from,from_cav,to,to_cav,gauge,color,type`) — one row per numbered, fully-connected wire. Run `EL_WIRENO` first; audit-gated like every deliverable. |
| `EL_CIRCUIT <name> [REF=<rung>]` \| LIST` | **Circuit Builder**: place a prewired circuit on the ladder — `start-stop`, `two-wire` (maintained selector + overload aux), `dol`, `fwd-rev`, `star-delta` (timer-switched starter control). Tags allocate per family as the circuit lands; wires connect real pin names, so the circuit is immediately traceable, auditable and numberable (`EL_WIRENO`). |
| `EL_PLC <tag> <di>,<do> [x,y] [BASE= BASEQ= DESC= MFG= CAT=]` \| EDIT <tag> [DI= DO= BASEI= BASEQ=] \| LIST \| CLEAR <tag\|ALL>` | Place a **parametric PLC I/O module** — generated from its point counts: body, point stubs, per-point addresses (`I0.0`-style; `BASE=I:2/` for rack:slot style). `IMPORT <tag> <io.csv>` wires a module’s points from a `point,node[,net]` spreadsheet (the PLCIO report inverted; the net becomes the wire’s note). Points are ordinary pins (`P1.I3`, `P1.Q1`) so wires land by name; `EL_REPORT PLCIO` lists address/point/wire/far-end for every point. |
| `EL_PANEL <tag> [x,y] [W= H= ITEM= DESC= MFG= CAT=] \| NUMBER [START=n] \| LIST \| CLEAR <tag\|ALL>` | Place a panel-layout **footprint** for a schematic tag (default body size from the family; re-placing moves/resizes and keeps the item number). `NUMBER` renumbers items in reading order; the audit warns when a footprint has no schematic partner and errors on duplicate tags/item numbers. |
| `EL_BALLOON <tag\|ALL> \| CLEAR` | Balloon footprints: a circled item number with a leader to the body — a pure render of the footprint records, so renumbering and re-placing keep every balloon correct. |
| `EL_REPORT … PANELBOM` | Panel BOM (item, tag, size, description, catalog — the catalog carried over the schematic tag link). |
| `EL_PANEL <tag> [x,y] [W= H= ITEM= DESC= MFG= CAT=] \| NUMBER [START=n] \| LIST \| CLEAR <tag\|ALL>` | Place a panel-layout **footprint** for a schematic tag (default body size from the family; re-placing moves/resizes and keeps the item number). `NUMBER` renumbers items in reading order; the audit warns when a footprint has no schematic partner and errors on duplicate tags/item numbers. |
| `EL_BALLOON <tag\|ALL> \| CLEAR` | Balloon footprints: a circled item number with a leader to the body — a pure render of the footprint records, so renumbering and re-placing keep every balloon correct. |
| `EL_STRIP [<strip>] [x,y] \| CLEAR [<strip>\|ALL] \| LIST` | Draw the strip diagram **both-sides**: one box per terminal with a centre rail — from-end wires above the left half, to-end wires above the right — plus with its number inside, wires above, partition below, jumper bridges arcing between bridged cells, doubled lines at partition boundaries. One plot per strip — redrawing replaces. |
| `EL_CABLE [tag] [x,y] [COND=] [COLOR=] [GAUGE=] [MFG=] [CAT=] [PART=] [FROM=] [TO=]` | Place a cable marker; the next free conductor is taken automatically. `LIST`, `CLEAR`. |
| `EL_CONNECTOR [refdes] [x,y] [PINS=a,b,c \| COUNT=n] [SPACING=] [PERROW=] [ORIENT=] [WNC]` | Place a parametric connector. `EDIT <ref>` changes its pin field; `LIST`, `CLEAR`. |
| `EL_CATALOG [LIST \| SEARCH \| FAMILY \| **MOUSER <query>** \| ADD <n> <tag>]` | Browse the parts catalogue. `MOUSER` live-searches Mouser (`MOUSER_API_KEY` env; session-cached); `ADD <n> <tag>` lands the pick as a device's MFG/CAT/DESC. `ASSIGN <item> MFG= CAT=`, `CLEAR <item>`, `PATH`. |
| `EL_SIGNAL <name> SOURCE\|DEST [x,y] [WIRE=<wref>]` \| LIST \| CLEAR <name\|ALL> \| SET <name> NAME= DIR= | Place a signal arrow — an off-sheet continuation marker anchored to a wire end. Arrows pair across sheets by name through the project register; after both sheets sync, a destination arrow labels itself with the source sheet. |
| `EL_PROJECT [NEW <name> \| SET \| SYNC \| CHECK \| **BOM** \| PATH]` | The cross-sheet register (`el-project.json`, beside the drawings): the drawing list with sheet numbers, the signal register, and each sheet's **extract** — the tags and wire numbers its sync declared. `SYNC` folds a sheet's arrows and extract in, then says whether anything now collides; `CHECK` names every cross-sheet tag or wire-number collision. Bare shows it all. `BOM` merges the **project-wide bill of materials** from every sheet's extract: identical (mfg, part) pairs tally, sheets+tags name the contributors. |
| `EL_REPORT [kind] [path.csv] [FORCE] [TABLE [x,y]] [HEIGHT=] \| ALL \| LIST` | Export a report, or draw it on the sheet. Kinds: BOM, COMPONENTS, WIREFROMTO, WIRELABELS, TERMINALS, CABLES, MISSINGCATALOG, DRAWINGLIST. |
| `EL_AUDIT [QUIET] [CODES]` | What is wrong with this drawing, and whether it is reportable. |
| `EL_MOVE <target> <x,y> \| DX= DY= [ROT=]` | Move a device, terminal, connector or ladder; attached wires are re-seated. |
| `EL_ROTATE <target> [deg]` | Rotate one object about its anchor. |
| `EL_REATTACH` | Re-seat every wire onto the pin it names. |
| `EL_LIST [summary \| devices \| wires \| terminals \| cables \| connectors \| nets \| qc]` | The drawing's inventory. |
| `EL_HIGHLIGHT <net\|wire\|tag\|@SEL>` | Trace a net across the drawing. `CLEAR` removes the trace. |
| `EL_SYMBOL <name> FAMILY=<code> [DESC=] [CHILD] [CONTACT=NO\|NC] [BASE=x,y] [PINS=a,b]` | Capture drawn geometry as a library symbol. `DEL <name>` removes one. |
| `EL_INFO [tag\|wirenr\|0xHANDLE\|@SEL]` | Print one entity's record and its resolved relationships. Bare = interactive pick. |
| `EL_EDIT <tag\|wirenr\|ladder\|0xHANDLE\|@SEL> KEY=VALUE …` | Change one object in place. |
| `EL_DELETE <target> … [FORCE]` | Remove an object and everything drawn for it. `FORCE` is asked for only when a whole ladder would go. |
| `EL_PROBE [path]` | Host capability self-test; writes its report to `path` when given. |
| `EL_PROBE BLOCK` / `BLOCKCHECK [path]` | The block-definition spike, in two dispatches. |

Device keys, on `EL_COMPONENT` and `EL_EDIT`: `TAG`, `FAMILY`, `SYMBOL`, `DESC`,
`MFG`, `CAT`, `ASSY`, `INST`, `LOC`, `RATING` (comma-separated), `FIXED`.

Settings keys: `SHEET`, `DRAWING`, `INST`, `LOC`, `LADDER_WIDTH`, `RUNG_SPACING`,
`RUNG_COUNT`, `FIRST_RUNG_REF`, `TAGMODE` (`S`/`R`), `TAG_FORMAT`, `TAG_START`,
`TAG_SUFFIXES`, `WIREMODE` (`S`/`R`), `WIRE_FORMAT`, `WIRE_START`,
`WIRE_INCREMENT`. A `+` stands in for a space in a value.

Format strings use ACADE's tokens: `%F` family, `%N` number, `%X` the duplicate
suffix, `%S` sheet, `%D` drawing, `%I` installation, `%L` location. **`%N` is
mandatory** — a format without it makes every device in a family collide on one
string, so the command refuses to save one. Wire-layer patterns take `%C`
(colour) and `%S` (size).

```text
EL_SETTINGS SHEET=02 DRAWING=PANEL-01 TAG_FORMAT=%S-%F%N WIREMODE=R
EL_LADDER 10,200 WIDTH=170 SPACING=10 RUNGS=12 REF=100
EL_COMPONENT K 60,200           → a coil, auto-tagged K100 (its rung)
EL_COMPONENT contact-no K100 100,195   → a contact belonging to it
EL_WIRE 20,200 90,200
EL_WIRENO                       → 100, 100A, 101 …  (the rung's references)
EL_XREF                         → the coil lists its contacts, and each contact
                                  lists the coil
```

### Wire numbering

Reference mode numbers a wire from **the line reference of the rung it sits
on**, with a suffix when a rung carries more than one — which is why a ladder has
to exist first. Sequential mode is a running counter in drawing order (top to
bottom, left to right). Both honour *fixed* numbers: a wire pinned with
`EL_WIRENO FIX` keeps its number, and the counter steps over it rather than
colliding.

Drawn numbers live on the `EL_WIRENO` layer, which is a **pure render of the
records** — every operation that changes a number rebuilds it. So a renumber is
idempotent and cannot desync, at the cost of a hand-moved label returning to its
wire's midpoint. New wires are deliberately unnumbered: numbering is an explicit
act, not a side effect of drawing.

### Devices, tags and cross-references

`EL_COMPONENT` places a device from the library and allocates its tag from the
drawing's settings: **a counter per family** in sequential mode, so `Q1` is the
first breaker rather than the third device; or the **rung's line reference** in
reference mode, suffixed when two devices of one family share a rung. A tag
already in the drawing is reserved, so placing a device never collides with one
that is there.

A **child** — a contact — is placed by naming its parent (`EL_COMPONENT
contact-no K1`), and stores the parent's tag. That single field is the whole
parent/child mechanism: cross-references, orphan detection and the retag cascade
are all derived from it, so the relationship cannot drift. `EL_LINK` attaches one
after the fact, and refuses a link that would close a loop.

`EL_XREF` writes the annotations in both directions — a coil lists where its
contacts are, a contact names its coil — onto the `EL_XREF` layer, which is a
**pure render** of the records and is rebuilt on every run. A cross-reference
that is confidently wrong is worse than none, so it is never patched in place.

A device is drawn as **several entities** and they all carry the same record, so
the index folds them by `el_id` into one device. That is load-bearing rather than
tidy: keyed by handle, a four-part coil looks like four devices, and a retag gives
it four tags (last write wins) while a delete removes one part and leaves three
orphans. A ladder is folded the same way.

### The symbol library

The library is a JSON file — read, diff and edit it. **52 IEC 60617 shapes
ship built-in** — every family code (K Q F M G T X S H R C B V PE) has a
parent, with the common variants a control schematic reaches for (timer
coils, delayed and changeover contacts, 2/3-pole breakers, three-phase and DC
machines, transformers and CTs, selectors, limit and e-stop switches, float/
pressure/temperature switches, buzzers and horns, variable resistors and
potentiometers, polarized capacitors, proximity/flow/level/photoelectric
sensors, diodes and LEDs) — and JIC/NFPA is a second file rather than a
second code path.

A word of caution learned the hard way: a library **file always wins over the
built-in set**, so a bare `EL_LIB EXPORT` (which now refuses) used to freeze a
snapshot of the built-in over future expansions. Export to a named file, or
let `EL_SYMBOL` captures create the project library.

`EL_SYMBOL` is what keeps the library open: it captures geometry you drew with
the host's own tools as a definition, storing it relative to a base point so it
rotates and places anywhere. Connection points are inferred from the geometry's
free ends — a lead ending *on* another stroke (a motor's circle, a resistor's
rectangle) is not a connection — and when that inference is ambiguous the command
refuses and asks for `PINS=` rather than guessing. This is the substitute for
AutoCAD Electrical's Symbol Builder, and its absence is what the research
identified as a hard adoption blocker.

`EL_LIB PATH` reports which file is in effect and why. With a project-local
sidecar, an explicit override and a shared per-user file all candidates, "the
symbol is missing" and "you are editing a different library" look identical from
the drawing. The resolution rules live in `el-core/src/paths.rs`, host-free and
tested; a project-local library is never created implicitly.

### Terminals, cables and connectors

A **terminal** belongs to a strip and its designator is the pair (`X1:12`), which
is why it is its own record rather than a component: the terminal plan and the
strip audits both key off the strip. `EL_TERMINAL NUMBER` walks a strip left to
right and numbers every block — including a protective-earth block, whose number
is its position. Leaving one out is not untidy, it is a bug: the skipped terminal
keeps its old number and the next block numbered into that slot collides with it.

A **cable** is modelled as a marker per conductor. The first marker is the parent
carrying the cable's tag and part; each later one pulls the parent's tag and takes
the next unused conductor colour, so a four-conductor cable reads as four markers
that obviously belong together.

A **connector**'s pins are *generated* from an ordered list and a spacing, which
is what makes a 24-way connector one command. The list order is the geometry, so
swapping two entries moves two pins — and `EL_CONNECTOR EDIT` rebuilds the glyph
rather than only the record, because the pin field is drawn.

Because a symbol is drawn as many primitives, its **insertion point is part of the
record** — the same thing a block insert carries. Every pin position is derived
from it, which is what lets a wire say `FROM=K1.A1 TO=X1:1` and record what it
reaches; a wire drawn by eye resolves to the nearest connection point instead.

### Reports, and the audit that gates them

Every report is derived from the records and gated on `EL_AUDIT`: an **error**
blocks the export, a warning does not, and `FORCE` is how a user says they know.
The CSV and the on-canvas table come from one layout, so the file and the sheet
cannot disagree.

Two conventions matter in the output. A missing value is a dash rather than a
blank — a BOM that silently omits a part number looks complete, while one that
shows a dash does not. And the BOM **tallies**: ten identical terminals are one
order line with a quantity, while the component report lists each device, because
that is what gets wired.

The catalogue lives in the same JSON file as the symbols, so a project's shapes
and the parts they represent travel together. `EL_CATALOG ASSIGN` writes the
manufacturer and part number as a pair, because a part number with no manufacturer
cannot be ordered — which is why that one is an error and a missing part is only a
warning.

### Moving things, and tracing them

A device, a terminal and a connector all have pins, and wires name the pins they
reach — so moving one has two halves that must happen together: the glyph and the
recorded placement move, and **every attached wire is re-seated**. Doing only the
first is how a drawing ends up with wires that claim to reach a pin they visibly
do not touch. `EL_MOVE` and `EL_ROTATE` do both, rebuilding the glyph from its
definition rather than translating it, because rebuild-from-record is the one path
that cannot drift.

A **net** is derived from the wires and the pins they name, never from proximity.
Two wires that merely cross are not one net, and a terminal's two sides are
separate circuits — which is the distinction that makes `EL_HIGHLIGHT` worth
having, and the reason `EL_LIST nets` says how many nets join more than one
device. A terminal's identity is its whole designator (`X1:1`), because two
terminals on one strip are different connections.

`EL_LIST` is the one-screen answer to "what is on this sheet?" — counts first,
audit verdict last, every number derived from the records so the dashboard cannot
disagree with the reports it summarises.

## Two host limits that shape everything

**Symbols are primitives, not blocks.** A schematic symbol naturally wants to be
a block reference with attributes, but the contract exposes no `AddBlockRecord`.
`EL_PROBE BLOCKCHECK` tests this rather than assuming it, and confirms failure for
two independent reasons: `document_mut()` hands an out-of-process plugin a local
cache, so a table insert never reaches the host; and a `BlockRecord` is a set of
handles (BLOCK, ENDBLK, and each member entity) that only the host can allocate,
so even a persisted record would define an empty block. Symbols are therefore
built from `LwPolyline`/`Circle`/`Line`/`MText` with the record carried as XDATA.
The verdict is in [`docs/host-capabilities-v2026.36.md`](docs/host-capabilities-v2026.36.md);
re-run the spike if the API version moves.

**An object may be many entities, so `el_id` is the identity.** A ladder is a
frame — a bus, rungs, and a reference per rung — and the contract offers no way
to group entities. Every drawn part therefore carries the *same* record, and the
index folds the parts back into one object by `el_id`. That is what lets a ladder
be redrawn, retagged or deleted as a unit no matter which part a user picks, and
it is the same mechanism Phase 2's symbols will use. An entity handle is never
stored in a record: handles are renumbered by DWG round-trips and copy/paste, so
a stored handle is a dangling reference waiting to happen.

**Interactive callbacks run against a `NullHost`.** `document()` *panics* there,
`add_entity` is a silent no-op, `write_record` fails silently. So every
interactive command captures what it needs at dispatch (`hostops::Capture`) and
reports through deferred output (`state::emit`) rather than printing directly.
An inspect-and-report command must also return `CommandStep::Done` on pick —
returning `NeedPoint` keeps the command line frozen and turns the user's next
keystroke into a confusing "not a Cartesian point" error.

**A multi-entity placement cannot commit through `CommandStep`.** The host has an
internal `CommitEntities` but the plugin contract's `CommandStep` carries one
entity, and a nested request made *from inside* a callback is serviced by
`NullHost` and does nothing. A background thread asking ~60 ms later — once the
host is back in its own event loop — does work, which is `state::spawn_flush`.
Clicking a ladder is therefore one undo window, not a dozen.

## Build, test, deploy

The load gate requires `[opencad] rustc_version` in `plugin.toml` to match the
host's own `rustc --version` **token for token**:

```bash
rustc --version   # must be rustc 1.99.0 (b940084d7 2026-09-28)
```

```bash
cargo test                                    # 382 tests: 296 el-core + 86 el-plugin
cargo build --release --package el-plugin

DEST="$APPDATA/OpenCADStudio/plugins/opencad.electrical"
mkdir -p "$DEST"
cp target/release/el_plugin.dll "$DEST/el_plugin.dll"
cp crates/el-plugin/plugin.toml "$DEST/plugin.toml"

python tests/serve_driver.py                  # 184 behavioural checks
```

Restart Open CAD Studio after deploying so Windows unmaps the old DLL. The
plugin is installed alongside `opencad.wiring-harness` and the two are
independent: distinct command prefixes, distinct XDATA app ids, and `EL_PROBE`
asserts that a sibling plugin's records stay invisible to our scan.

`OCS_SERVE_EXE` overrides the host binary the behavioural suite drives.

## Status

Phases 0-4 of the plan are complete and verified against the real host, plus
the wire-crossing gap that Phase 1 deferred (2026-10-25):

- **Phase 0** — workspace, record model, settings carrier, dispatch, ribbon, the
  capability probe and the block-definition spike.
- **Phase 1** — ladders and line references, wire types, wire drawing with axis
  snapping, and both wire-numbering modes, plus `EL_EDIT`/`EL_DELETE`.
- **Phase 2** — the IEC symbol library, component placement with auto-tagging,
  parent/child links, cross-reference annotation, drawing-wide retag, and symbol
  capture.
- **Phase 3** — terminals and strips, cables and their conductors, parametric
  connectors, the parts catalogue, nine reports to CSV or the sheet, and the rule
  engine that gates them.
- **Phase 4** — net tracing, electrical-aware move and rotate with automatic
  wire re-seating, and the inventory dashboard.
- **Wire gap** — `EL_WIREGAP` / `EL_TRIMWIRE`: the Phase 1 deferral landed.
  A gap-flagged wire is drawn as several polylines sharing one `el_id` (the
  same fold ladders use; the index folds runs first-wins, and `wire_of_handle`
  resolves any run to the whole wire). Connectivity stays record-based, so the
  gap can never change a net; re-seats, moves and trims re-split the runs
  against the crossings as they are now. The maths lives in
  `el-core/src/wiregap.rs` (`split_runs`, `fold_runs`), on top of
  `geom.rs::proper_crossing`, which is what keeps a junction from ever being
  gapped.
- **Project + signals (T2, first increment)** — `EL_PROJECT` maintains the
  cross-sheet register `el-project.json` beside the drawings (drawing list
  with sheet numbers, signal register); `EL_SIGNAL` places source/destination
  arrows anchored to wire ends. After both sheets sync, a destination arrow
  labels itself with the sheet its source lives on — without that sheet being
  open. Arrows ride their wire end through moves, re-seats and trims, and the
  audit rules `SIGNAL_NO_NAME` / `SIGNAL_NO_WIRE` / `DUP_SIGNAL_SOURCE` keep a
  broken arrow from hiding.
- **Cross-sheet collisions (T2, second increment)** — every `EL_PROJECT SYNC`
  registers the sheet's **extract**: its parent device tags and wire numbers.
  `EL_PROJECT CHECK` (and every sync, and the drawing list report) name the
  designators two sheets both claim — the offline stand-in for ACADE's
  project-wide update/retag, possible because each sheet declares what it
  holds instead of the register reading drawings it cannot open. The drawing
  list report lists every project sheet and the collision verdict from the
  register.

The plan's phases are complete. What is deferred, named rather than forgotten:

A **terminal strip editor** (ACADE's TSE) — the electrical core and the drawn
diagram are in: `EL_TERMINAL JUMPER` bridges terminals into one circuit (nets
merge, the plan lists it, deletion cascades); `REORDER` renumbers a strip in a
stated order with jumpers remapping; `PARTITION` groups a strip and marks the
boundaries; `EL_STRIP` draws the diagram — boxes, wires, jumper arcs, partition
dividers — one plot per strip, replaced on redraw. The audit refuses dangling
or cross-strip bridges. Still missing from full TSE parity: multi-tier
**graphical terminal diagrams** (per-terminal both-sides drawings). **panel layout** is now in (footprints by schematic tag, item numbers,
balloons, panel BOM — CH8's core); remaining niceties: nameplates, DIN rail,
per-vendor footprint lookup. Also: **PLC I/O** (needs a module data file),
and **`EL_TITLEBLOCK`** (needs a title block to write into). The whole parity queue, in priority order, lives in
[`docs/PARITY_PLAN.md`](docs/PARITY_PLAN.md).

Deferred from Phase 3, in more detail: a **terminal strip editor** (ACADE's TSE, which reorders a
strip and charts its jumpers), **panel layout** — footprints, item numbers and
balloons — and **PLC I/O**, which needs a module data file. `EL_TITLEBLOCK` is also
not built; the drawing list reports a sheet's own metadata, but writing it into a
title block needs a title block to write into.

Next: an inventory dashboard and audit polish (`EL_LIST`), then the deferred
families above.

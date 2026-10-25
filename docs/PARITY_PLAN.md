# AutoCAD Electrical parity plan & agent handoff

> Written 2026-10-25 after a feature audit of `opencad.electrical` 0.1.4
> (API 10, host v2026.42) against AutoCAD Electrical's feature set. This file
> is the operational task ledger; `../README.md` is the knowledge base.

## Textbook reference map (Tickoo, *AutoCAD Electrical 2016*, 7th ed.)

The user's reference PDF lives at the workspace root. Its chapters map onto
this queue:

| Chapter | Topic | Our state |
|---|---|---|
| 1 Introduction | — | n/a |
| 2 Projects & drawings | project manager, per-drawing props | **T2** register + extracts ✓ (retag orchestration pending) |
| 3 Wires | types, numbering, gaps/trim | wire types ✓, dual numbering ✓, **gaps + trim ✓** |
| 4 Ladders | — | ladders + rungs ✓ |
| 5 Schematic components | icon menu, parent/child, catalog | **expanded icon set ✓ (52 symbols)**, links/xrefs ✓, catalog ✓, **multiple-insert ✓ (`EL_MINSERT`)**; swap/update pending |
| 6 Schematic editing | move/copy/retag/audit | move/rotate/retag ✓, audit ✓, **GUI hang fixed** |
| 7 Connectors & circuits | parametric connectors, circuit builder | connectors ✓; **circuit builder landed 2026-10-25**: `EL_CIRCUIT start-stop / dol / fwd-rev` — table-driven templates, slot-key link resolution (shared parent tags stay unambiguous), family tag allocation, prewired named-pin wires. **swap ✓ (`EL_COMPONENT SWAP`) · two-wire + star-delta templates ✓**; remaining: spreadsheet-driven template authoring. |
| 8 Panel layouts | footprints, balloons | **absent (T4)** |
| 9 Reports | schematic/panel reports | 9 reports ✓ (panel reports need T4) |
| 10 PLC modules | parametric modules | **T5 core landed 2026-10-25**: `EL_PLC <tag> di,do` parametric modules (KIND_PLC), auto-addressed points as ordinary pins (wires land by name, move re-seats), `EL_PLC EDIT` regenerates, PLCIO report, audit `PLC_NO_TAG/DUP_TAG/NO_POINTS/DUP_ADDRESS/NO_WIRES`. **spreadsheet I/O import ✓ (`EL_PLC IMPORT`)**; remaining: stand-alone points, module catalog lookup. |
| 11 Terminals | strips, jumpers, TSE | **T3 complete** ✓ (multi-tier diagrams pending) |
| 12 Settings/templates/plot | title blocks, plot | settings ✓; **T6 landed 2026-10-25**: `EL_TITLEBLOCK` frame + title table, fields rendered live from settings and the project register (OF n), `UPDATE` re-render. Remaining: plot styles/templates, WDT-style field mapping. |
| 13 Creating symbols | symbol builder | `EL_SYMBOL` capture ✓ |
| 14 Projects | — | see CH2 |

## Where the plugin stands

The core ACADE authoring loop is built and verified (Phases 0–4, 382 unit
tests + 184 serve checks): ladders with line references, dual wire-numbering
with ACADE format tokens, per-family tagging, parent/child cross-references,
terminals/cables/parametric connectors, a 32-rule audit gating 9 reports,
net tracing, rebuild-from-record move/rotate, symbol capture.

The remaining distance to ACADE parity is, in order of user value:

| # | Task | ACADE counterpart | Status |
|---|---|---|---|
| T1 | Wire-crossing gap rendering + `EL_WIREGAP` / `EL_TRIMWIRE` | WIREGAP / Trim Wire | **in progress** (this doc's first landed task) |
| T2 | Source/destination signal arrows + project index (multi-drawing) | Project Manager, off-page references | deferred by design; the single biggest parity jump |
| T3 | Terminal strip editor: reorder, jumpers, partitions; graphical terminal plan | TSE / terminal plan | deferred; strips + numbering already exist |
| T4 | Panel layout: footprints linked to schematic devices, item numbers, balloons | Panel Layout | deferred; footprint pattern proven in the sibling WH plugin |
| T5 | PLC I/O: JSON module data + `EL_IO IMPORT <csv>` + parametric modules | PLC I/O Utility | deferred; needs a module data file |
| T6 | `EL_TITLEBLOCK`: drawing frame + writing project metadata into it | Title blocks | deferred; `WH_REPORTMETA`/PDF title block is the template |
| T7 | Symbol library depth: shipped JIC/NFPA set, per-contactor auxiliary-contact tables | ~2000-symbol libraries | partial (IEC set + capture exist) |

### T7 first increment — landed 2026-10-25

The built-in IEC set grew **17 → 52 symbols**, covering every `IEC_FAMILIES`
code (T transformer and B sensor were entirely absent; V semiconductor added):
timer coils (on/off-delay), delayed NO/NC contacts, changeover contact,
2/3-pole breakers, fuse-breaker, 3-phase & DC motors, AC supply, battery,
generator, 2/3-winding transformers, current transformer, 2/3-position
selectors, limit switches, e-stop, float/pressure/temperature switches,
buzzer, horn, variable resistor, potentiometer, polarized capacitor,
proximity/flow/level/photoelectric sensors, diode, LED. Multi-pin symbols use
the new `LibraryBuilder::custom()` (explicit pin lists).

**Trap closed:** a bare `EL_LIB EXPORT` had snapshotted the old 17-symbol
built-in into the per-user `el-library.json`, which then *shadowed* the
compiled-in set — every new built-in symbol was invisible. The stale snapshot
(verified to contain zero user symbols) was renamed aside, and bare `EL_LIB
EXPORT` now refuses to freeze an unmodified built-in into the fallback
library. **Rule: a file library always wins over the built-in — export
deliberately, never bare.**
| T8 | Circuit builder: parametric circuit templates with retag/renumber | Circuit Builder | absent |
| T9 | Report configurability + project-wide report merge; XLS output | Report generator | 9 fixed kinds exist |
| T10 | Catalog depth: multi-mfr per device, live parts search (reuse WH `mouser.rs`) | Catalog database | partial |
| T11 | EL→WH bridge: netlist CSV the wiring-harness `WH_IMPORT` consumes | (ACADE→Inventor C&H) | **landed 2026-10-25: `EL_EXPORT WH`** — numbered connected wires → WH wirelist CSV; also `EL_HIGHLIGHT <tag>` bare-tag fallback fixed (bare TAG ends now trace) |
| T12 | Hygiene: git init (the tree is unversioned), README ABI refresh to v2026.42 | — | quick win, do first |

## Ground rules (do not violate)

1. **Records are truth; drawn geometry is a render.** Every new feature
   (arrows, jumpers, footprints, title block) must follow the rule that made
   renumber/retag/xref unable to desync: a layer is rebuilt from records,
   never patched in place.
2. **`el_id` is identity.** An object may be many entities; handles are never
   stored in records. New multi-entity objects fold in the index the way
   ladders and (now) gapped wires do.
3. **Additive records only.** New fields decode with `unwrap_or(default)` so
   older drawings open; a changed semantic gets a new key, not a new meaning
   for an old one.
4. **Host is pristine.** Everything out-of-tree; re-run `EL_PROBE`/`BLOCKCHECK`
   when the API version moves.
5. **One DLL is pinned to one host generation** (`rustc_version` +
   `acadrust_source` token-for-token). Current: host v2026.42, API 10,
   codec 0.6.4 — README's ABI notes were written against v2026.40.1 and need
   the refresh T12 names.

## Verification ritual

```bash
cd D:/Sandbox/OpenCad/ocs-electrical
cargo test                 # all unit tests green
cargo build --release --package el-plugin
DEST="$APPDATA/OpenCADStudio/plugins/opencad.electrical"
mkdir -p "$DEST" && cp target/release/el_plugin.dll "$DEST/el_plugin.dll" \
  && cp crates/el-plugin/plugin.toml "$DEST/plugin.toml"
python tests/serve_driver.py    # behavioural suite, ALL CHECKS PASSED
```

Restart Open CAD Studio after deploying so Windows unmaps the old DLL.

## Task ledger

| Task | State | Notes |
|---|---|---|
| T1 wire-gap + trim | **landed 2026-10-25** | wire is multi-entity when gapped (runs share one `el_id`); gap flag on the wire record (`GAP`), gap size in drawing settings (`WIREGAP=`); nets/reports/audit unchanged (one wire row per `el_id`, first-wins fold). Exactly one wire yields at a crossing (unflagged passes through; between flagged wires the smaller `el_id` keeps running). Suite 198 checks, 398 unit tests. **Lesson:** partner lookups must go through an `el_id`-keyed path map read live from the document (`wire_path_map`) — index handles go stale the moment any wire is re-rendered in the same dispatch, and a stale-handle partner read returns nothing, silently rendering the wire whole. |
| T12 hygiene | **half done 2026-10-25** | README refreshed (commands, status, deferred list, pointer here); `docs/PARITY_PLAN.md` created. **Still open: the tree is not a git repository** — init + push before any further work. |
| T2 projects | **two increments landed 2026-10-25** | ① `el-project.json` beside the drawings (`el-core/src/project.rs`): drawing list by sheet number + signal register; `EL_PROJECT NEW/SET/SYNC/PATH`; `EL_SIGNAL` source/dest arrows anchored to wire ends (KIND_SIGNAL, component-style fold), pairing notes rendered from the register at render time, arrows ride their wire end through reseat/trim/move; audit `SIGNAL_NO_NAME`/`SIGNAL_NO_WIRE`/`DUP_SIGNAL_SOURCE`. ② per-sheet **extracts**: SYNC registers the sheet's parent tags + wire numbers; `EL_PROJECT CHECK` + every sync + the drawing list report name cross-sheet tag/wire-number collisions (the offline stand-in for ACADE's project-wide retag). Remaining layers: retag orchestration across sheets (needs the host to open drawings or a batch CLI), project-wide reports beyond the drawing list. |
| T3 strip editor | **landed 2026-10-25 (two increments)** | ① jumpers: `EL_TERMINAL JUMPER <a> <b> | CLEAR | LIST` — KIND_JUMPER record, `net.rs` unions jumpered designators (two wires landing one per side are one net), terminal plan JUMPERED TO column, terminal-delete cascade, audit `JUMPER_INCOMPLETE/JUMPER_SELF/JUMPER_DANGLING/JUMPER_CROSS_STRIP/JUMPER_DUPLICATE`; `EL_DELETE <X1:1>` resolves terminals at all now. ② `EL_TERMINAL REORDER <strip> <no…>` renumbers in a stated order with jumper remap; `EL_TERMINAL PARTITION <d> <name|CLEAR>` groups a strip (PTN column on the plan); `EL_STRIP [<strip>] [x,y] | CLEAR | LIST` draws the strip diagram (KIND_STRIPPLOT, one plot per strip, replaced on redraw) — boxes, wire labels, jumper arcs, partition dividers. Remaining: multi-tier per-terminal diagrams (both-sides drawings). |
| T4–T11 | queued | take in table order |

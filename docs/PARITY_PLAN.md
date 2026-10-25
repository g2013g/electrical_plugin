# AutoCAD Electrical parity plan & agent handoff

> Written 2026-10-25 after a feature audit of `opencad.electrical` 0.1.4
> (API 10, host v2026.42) against AutoCAD Electrical's feature set. This file
> is the operational task ledger; `../README.md` is the knowledge base.

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
| T8 | Circuit builder: parametric circuit templates with retag/renumber | Circuit Builder | absent |
| T9 | Report configurability + project-wide report merge; XLS output | Report generator | 9 fixed kinds exist |
| T10 | Catalog depth: multi-mfr per device, live parts search (reuse WH `mouser.rs`) | Catalog database | partial |
| T11 | EL→WH bridge: netlist CSV the wiring-harness `WH_IMPORT` consumes | (ACADE→Inventor C&H) | absent; strategic differentiator — both ends are ours |
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
| T2 projects | **first increment landed 2026-10-25** | `el-project.json` beside the drawings (`el-core/src/project.rs`): drawing list by sheet number + signal register; `EL_PROJECT NEW/SET/SYNC/PATH`; `EL_SIGNAL` source/dest arrows anchored to wire ends (KIND_SIGNAL, component-style fold), pairing notes rendered from the register at render time, arrows ride their wire end through reseat/trim/move; audit `SIGNAL_NO_NAME`/`SIGNAL_NO_WIRE`/`DUP_SIGNAL_SOURCE`. Next layers: project-wide retag, cross-drawing wire-number uniqueness, DRAWINGLIST from the register, project-wide reports. |
| T3–T11 | queued | take in table order |

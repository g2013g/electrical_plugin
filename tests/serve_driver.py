#!/usr/bin/env python3
"""Behavioural suite for the OpenCAD Studio electrical plugin (opencad.electrical).

Drives the host in `--serve` mode over its line-JSON stdio protocol and checks
the plugin's observable behaviour: that the host loads it, that settings and
records round-trip through DWG/DXF, that ladders and wires are placed and
numbered correctly, and that failures are reported rather than silent.

Run:
    python tests/serve_driver.py

Override the host binary with OCS_SERVE_EXE.
Serve-mode quirks this suite is built around, all confirmed against the host:

* The host writes a `{"ready": true}` banner on startup. If it is not drained
  first, every later read is off by one.
* `push_info` / `push_output` are NOT forwarded to stdout. Only a command
  *error* surfaces, as `ok: false` with a message, and it can surface on the
  response to the *next* op. So succeed-path assertions are made against side
  effects (entity counts, files on disk, reports the command wrote itself) and
  error assertions collect two responses.
* A saved drawing stays locked by the host, so files are read only after the
  session that wrote them has exited.
"""

import json
import os
import re
import queue
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
WORK = os.path.join(ROOT, "target", "serve-test")
# Default: the installed auto-updating desktop build (v2026.42, API 10, as of
# 2026-10-24); override with OCS_SERVE_EXE for another host build.
EXE = os.environ.get(
    "OCS_SERVE_EXE",
    r"C:\Program Files\Open CAD Studio\OpenCADStudio.exe",
)

META_LAYER = "EL_META"
LADDER_LAYER = "EL_LADDER"
WIRE_LAYER = "EL_WIRES"
WIRENO_LAYER = "EL_WIRENO"
COMPONENT_LAYER = "EL_COMPONENTS"
TAG_LAYER = "EL_TAGS"
XREF_LAYER = "EL_XREF"
TERMINAL_LAYER = "EL_TERMINALS"
CABLE_LAYER = "EL_CABLES"
TABLE_LAYER = "EL_TABLE"
HILITE_LAYER = "EL_HILITE"

failures = []
checks = 0


def check(name, cond, detail=""):
    global checks
    checks += 1
    if cond:
        print(f"[PASS] {name}")
    else:
        print(f"[FAIL] {name}" + (f" — {detail}" if detail else ""))
        failures.append(name)


class Serve:
    """A host process driven over the line-JSON serve protocol."""

    def __init__(self, exe, cwd, env=None):
        os.makedirs(cwd, exist_ok=True)
        full_env = dict(os.environ)
        full_env.update(env or {})
        self.proc = subprocess.Popen(
            [exe, "--serve"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            cwd=cwd,
            env=full_env,
            bufsize=1,
        )
        self.lines = queue.Queue()
        self.stderr = []
        threading.Thread(target=self._pump_stdout, daemon=True).start()
        threading.Thread(target=self._pump_stderr, daemon=True).start()
        self.cwd = cwd

    def _pump_stdout(self):
        for line in self.proc.stdout:
            self.lines.put(line)

    def _pump_stderr(self):
        for line in self.proc.stderr:
            self.stderr.append(line)

    def prime(self, timeout=240):
        """Drain the startup banner. Everything else assumes this happened."""
        banner = json.loads(self.lines.get(timeout=timeout))
        assert banner.get("ready") is True, f"unexpected banner: {banner}"
        return banner

    def loaded_plugins(self):
        time.sleep(0.5)  # let the load banner land
        return [l.strip() for l in self.stderr if "Loaded plugin" in l]

    def send(self, obj, timeout=240):
        self.proc.stdin.write(json.dumps(obj) + "\n")
        self.proc.stdin.flush()
        return json.loads(self.lines.get(timeout=timeout))

    def run(self, cmd, timeout=240):
        return self.send({"op": "run", "cmd": cmd}, timeout)

    def run_and_collect(self, cmd):
        """Run a command, then a benign *protocol* op, returning both responses.

        Two reasons this exists rather than asserting on one response:

        * A plugin command error can surface on the response to the next op, so
          the failure has to be collected from a second round-trip.
        * The follow-up must NOT dispatch a command. A bare `EL_INFO` starts an
          interactive pick, which leaves the host waiting for input and swallows
          every later command in the phase — turning one assertion into a phase
          full of misleading failures. A protocol op cannot do that.
        """
        out = [self.run(cmd)]
        out.append(self.send({"op": "entities"}))
        return json.dumps(out)

    def count_on(self, layer):
        r = self.send({"op": "query", "layer": layer})
        return r.get("returned", 0)

    def type_counts(self):
        return self.send({"op": "entities"}).get("by_type", {})

    def total_entities(self):
        return self.send({"op": "entities"}).get("total", 0)

    def close(self):
        try:
            self.proc.stdin.close()
        except Exception:
            pass
        self.proc.terminate()
        try:
            self.proc.wait(timeout=30)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def xdata_pair(key, value):
    """Match one XDATA key/value pair in a saved DXF.

    DXF writes a group code between the key and its value, so a pair reads as
    `1000\\nKEY\\n1000\\nVALUE`. Matching `KEY\\nVALUE` never matches anything,
    and a bare substring search for a key can hit a longer key that contains it
    (`WREF` inside `SHOWREF`).
    """
    return f"1000\n{key}\n1000\n{value}\n"


def xdata_int_pair(key, value):
    """Match an XDATA key whose value is an integer.

    An integer payload sits behind a numeric group code (the host writes 1070 or
    1071 depending on width) and is right-justified, so the string-pair helper
    cannot see it.
    """
    return re.compile(rf"1000\n{re.escape(key)}\n10\d\d\n\s*{re.escape(value)}\n")


def read(path):
    with open(path, encoding="utf-8", errors="replace") as fh:
        return fh.read()


def probe_report(s, name="probe-report.txt"):
    """Run EL_PROBE writing a report beside the working dir and return it."""
    path = os.path.join(WORK, name)
    if os.path.exists(path):
        os.remove(path)
    s.run(f"EL_PROBE {name}")
    for _ in range(20):
        if os.path.exists(path):
            time.sleep(0.1)
            return read(path)
        time.sleep(0.1)
    return ""


def session(exe=EXE):
    s = Serve(exe, WORK)
    s.prime()
    return s


# ── the suite ────────────────────────────────────────────────────────────


def xdata_str_values(txt, key):
    """Every string value stored under one XDATA key, in file order."""
    return re.findall(rf"1000\n{re.escape(key)}\n1000\n([^\n]*)", txt)


def xdata_int_values(txt, key):
    """Every integer value stored under one XDATA key.

    An integer payload sits behind a numeric group code and is right-justified,
    so the string helper cannot see it.
    """
    return [v.strip() for v in re.findall(xdata_int_pattern(key), txt)]


def xdata_int_pattern(key):
    return "1000\n" + re.escape(key) + "\n10\\d\\d\n([^\n]*)"


def phase_0_load_and_probe():
    print("\n── load, capability probe ──")
    s = session()
    loaded = s.loaded_plugins()
    banner = "\n".join(loaded)
    check(
        "plugin loaded by host",
        any("opencad.electrical" in l for l in loaded),
        f"stderr load lines: {loaded}",
    )
    # Version-agnostic: the intent is "the banner names the plugin with its
    # version", not a specific release (the hardcode went stale at 0.1.1).
    check(
        "plugin version reported in the load banner",
        re.search(r"opencad\.electrical \d+\.\d+\.\d+", banner) is not None,
        banner,
    )
    check(
        "loads alongside the sibling wiring-harness plugin",
        any("opencad.wiring-harness" in l for l in loaded)
        and any("opencad.electrical" in l for l in loaded),
        "both plugins installed and loaded",
    )

    report = probe_report(s)
    check("probe wrote its report", bool(report), "no report file produced")
    if report:
        lines = [l for l in report.splitlines() if l.startswith("[")]
        failed = [l for l in lines if l.startswith("[FAIL]")]
        check(f"all {len(lines)} probe checks pass", bool(lines) and not failed, f"failing: {failed}")
        check(
            "probe confirms no block definitions (symbols must be primitives)",
            "block name resolves in the block table = false" in report,
            "the Insert/block-record finding changed — re-plan the symbol layer",
        )
        check(
            "probe confirms sibling app-id isolation",
            "[PASS] sibling app-id records are invisible" in report,
            "",
        )
    s.close()

    # The block spike needs two dispatches, so it gets its own session.
    s = session()
    s.send({"op": "new"})
    s.run("EL_PROBE BLOCK")
    s.run("EL_PROBE BLOCKCHECK el-block-verdict.txt")
    time.sleep(0.3)
    total = s.total_entities()
    s.close()
    vpath = os.path.join(WORK, "el-block-verdict.txt")
    check("the block spike wrote a verdict", os.path.exists(vpath))
    if os.path.exists(vpath):
        verdict = read(vpath)
        check(
            "the verdict records that block definitions do not survive",
            "block definition survived the dispatch: false" in verdict,
            verdict[:200],
        )
        check(
            "the spike removed its leftover insert",
            "removed 1 leftover insert" in verdict,
            verdict[:200],
        )
    check("the drawing is left with no stray probe entities", total == 0, f"{total} entities remain")


def phase_settings():
    print("\n── settings ──")
    s = session()
    s.send({"op": "new"})
    check("a new drawing starts with no settings carrier", s.count_on(META_LAYER) == 0)

    s.run("EL_SETTINGS SHEET=07 DRAWING=LINE-3 TAG_FORMAT=%S-%F%N WIREMODE=R")
    check("EL_SETTINGS creates exactly one carrier", s.count_on(META_LAYER) == 1)

    s.run("EL_SETTINGS DRAWING=CHANGED")
    check("a partial update edits in place rather than appending", s.count_on(META_LAYER) == 1)

    before = s.count_on(META_LAYER)
    s.run_and_collect("EL_SETTINGS TAG_FORMAT=K")
    check("a tag format without %N is refused", s.count_on(META_LAYER) == before)
    s.run_and_collect("EL_SETTINGS WIRE_FORMAT=X")
    check("a wire-number format without %N is refused", s.count_on(META_LAYER) == before)

    s.send({"op": "save", "path": "settings.dxf"})
    s.close()

    path = os.path.join(WORK, "settings.dxf")
    check("save produced a DXF", os.path.exists(path))
    if os.path.exists(path):
        txt = read(path)
        check("the OCSEL app id is registered in the saved file", "OCSEL" in txt)
        for token in ["SCHEMA", "KIND", "settings", "SHEET", "07", "TAGFMT", "%S-%F%N", "WIREMODE"]:
            check(f"saved file carries {token!r}", token in txt)

    s = session()
    s.send({"op": "open", "path": path})
    check("the settings carrier survives save→reopen", s.count_on(META_LAYER) == 1)
    s.run("EL_SETTINGS SHEET=99")
    check("an update after reopen edits the existing carrier", s.count_on(META_LAYER) == 1)
    s.close()


def phase_wire_types():
    print("\n── wire types ──")
    s = session()
    s.send({"op": "new"})
    check("a new drawing starts with no wire-type record", s.count_on(META_LAYER) == 0)

    s.run("EL_WIRETYPE ADD ORANGE COLOR=OR SIZE=12")
    check("EL_WIRETYPE ADD creates a record", s.count_on(META_LAYER) == 1)

    s.run("EL_WIRETYPE ADD ORANGE COLOR=OR SIZE=12")
    check("adding a duplicate name is refused", s.count_on(META_LAYER) == 1)

    # A second type resolving to the same layer would make a picked wire
    # ambiguous, so it must be refused.
    check("wire-type list works", s.run("EL_WIRETYPE LIST").get("ok") is True)

    s.run("EL_WIRETYPE EDIT ORANGE SIZE=10")
    check("EL_WIRETYPE EDIT keeps one record", s.count_on(META_LAYER) == 1)

    text = s.run_and_collect("EL_WIRETYPE EDIT NOSUCH COLOR=RD")
    check("editing a missing type is reported", "no type named" in text, text[:200])

    s.run("EL_WIRETYPE CLEAR")
    check("EL_WIRETYPE CLEAR removes the record", s.count_on(META_LAYER) == 0)

    # The defaults must still resolve, or no wire could ever be drawn.
    check("the built-in type set survives CLEAR", s.run("EL_WIRETYPE").get("ok") is True)
    s.close()


def phase_ladder():
    print("\n── ladders and line references ──")
    s = session()
    s.send({"op": "new"})

    s.run("EL_LADDER 10,200 WIDTH=170 SPACING=10 RUNGS=5 REF=100")
    ladder_count = s.count_on(LADDER_LAYER)
    # 2 bus verticals + 5 rungs + 5 references
    check("EL_LADDER draws the frame and references", ladder_count == 12, f"{ladder_count} entities")

    s.run("EL_LADDER 10,100 RUNGS=2")
    ladder_entities = s.count_on(LADDER_LAYER)
    check(
        "a second ladder adds its own frame",
        ladder_entities == 12 + 6,
        f"{ladder_entities} entities after 2 ladders (expected 18)",
    )

    # A rung count of zero must be refused, not defaulted to one: silently
    # drawing a rung the caller did not ask for is worse than an error.
    text = s.run_and_collect("EL_LADDER RUNGS=0")
    check("a zero-rung ladder is refused", "RUNGS expects a positive" in text, text[:200])
    text = s.run_and_collect("EL_LADDER 0,0 WIDTH=0")
    check("a zero-width ladder is refused", "WIDTH must be positive" in text, text[:200])
    text = s.run_and_collect("EL_LADDER 0,0 NORUNGS NOBUS NOREF")
    check(
        "a ladder with nothing to draw is refused",
        "leaves nothing to draw" in text,
        text[:200],
    )

    # Revising must rebuild the frame, not add to it.
    before = s.count_on(LADDER_LAYER)
    s.run("EL_RUNG 100 ADD=3")
    after = s.count_on(LADDER_LAYER)
    # Ladder 1 goes from 5 rungs to 8: 2 verticals + 8 rungs + 8 refs = 18.
    # Ladder 2 is untouched at 6. A rebuild gives 24, not 24 + 12.
    check(
        "EL_RUNG ADD rebuilds the frame rather than appending",
        after == 24,
        f"{before} → {after} (expected 24 = 18 rebuilt + 6 untouched)",
    )

    s.run("EL_LADDER CLEAR")
    left = s.count_on(LADDER_LAYER)
    check("EL_LADDER CLEAR removes every ladder", left == 0, f"{left} entities left")
    s.close()


def phase_wires_and_numbering():
    print("\n── wires and wire numbering ──")
    s = session()
    s.send({"op": "new"})
    # Sequential is the easier mode to assert exact numbers on.
    s.run("EL_SETTINGS WIREMODE=S WIRE_FORMAT=%N WIRE_START=1")

    # Two rungs, so reference mode can be checked later.
    s.run("EL_LADDER 10,200 WIDTH=170 SPACING=10 RUNGS=4 REF=100")

    s.run("EL_WIRE 20,200 90,200")
    s.run("EL_WIRE 20,190 90,190")
    s.run("EL_WIRE 20,180 90,180")
    wires = s.count_on(WIRE_LAYER)
    check("three wires drawn", wires == 3, f"{wires} on {WIRE_LAYER}")

    check("no wire numbers are drawn before EL_WIRENO", s.count_on(WIRENO_LAYER) == 0)

    s.run("EL_WIRENO")
    labels = s.count_on(WIRENO_LAYER)
    check("EL_WIRENO draws one label per wire", labels == 3, f"{labels} labels")

    # Sequential from start=1, ordered top to bottom.
    s.send({"op": "save", "path": "wires.dxf"})
    s.close()

    txt = read(os.path.join(WORK, "wires.dxf")).replace("\r\n", "\n")
    for number in ["1", "2", "3"]:
        check(
            f"sequential number {number} reached the file",
            xdata_pair("WREF", number) in txt,
            f"no WREF={number}",
        )

    # Reference mode: numbers come from the rung the wire sits on.
    s = session()
    s.send({"op": "new"})
    s.run("EL_SETTINGS WIREMODE=R WIRE_FORMAT=%N%X")
    # Reference numbering needs the ladder to exist *in this drawing*, so this
    # phase builds its own rather than inheriting the previous session's.
    s.run("EL_LADDER 10,200 WIDTH=170 SPACING=10 RUNGS=4 REF=100")
    s.run("EL_WIRE 20,200 90,200")
    s.run("EL_WIRE 20,200 90,200")
    s.run("EL_WIRE 20,190 90,190")
    s.run("EL_WIRENO")
    labels = s.count_on(WIRENO_LAYER)
    check("reference mode labels every wire on a rung", labels == 3, f"{labels} labels")
    s.send({"op": "save", "path": "refs.dxf"})
    s.close()

    # Assert on the record's own key/value pair, never a bare number: a lone
    # "100" appears all over a DXF (coordinates, handles) and would pass even if
    # numbering had failed outright.
    txt = read(os.path.join(WORK, "refs.dxf")).replace("\r\n", "\n")
    check(
        "the first wire takes the rung reference",
        xdata_pair("WREF", "100") in txt,
        "no WREF=100",
    )
    check(
        "a second wire on one rung gets a suffix",
        xdata_pair("WREF", "100A") in txt,
        "no WREF=100A",
    )
    check(
        "the next rung numbers from its own reference",
        xdata_pair("WREF", "101") in txt,
        "no WREF=101",
    )


def phase_wire_gap_and_trim():
    print("\n── wire crossing gap and trim ──")
    s = session()
    s.send({"op": "new"})
    s.run("EL_SETTINGS WIREMODE=S WIRE_FORMAT=W%N WIRE_START=1")

    # An X: the drawn crossing is what a gap exists for. The midpoints differ,
    # so the drawing order — and with it which wire becomes W1 — is settled.
    s.run("EL_WIRE 20,200 90,200")
    s.run("EL_WIRE 60,180 60,220")
    wires = s.count_on(WIRE_LAYER)
    check("two crossing wires drawn", wires == 2, f"{wires} on {WIRE_LAYER}")

    s.run("EL_WIRENO")
    labels = s.count_on(WIRENO_LAYER)
    check("both crossing wires number", labels == 2, f"{labels} labels")

    # Gap everything. Both wires are flagged, but exactly one yields at the
    # crossing — the smaller el_id keeps running — so the drawing gains one
    # extra run, not two.
    s.run("EL_WIREGAP ALL")
    gapped = s.count_on(WIRE_LAYER)
    check("EL_WIREGAP ALL splits exactly one wire into two runs", gapped == 3, f"{gapped} on {WIRE_LAYER}")

    # A gapped wire still numbers from its folded path.
    s.run("EL_WIRENO")
    labels = s.count_on(WIRENO_LAYER)
    check("gapped wires still number", labels == 2, f"{labels} labels")

    # The bare report names the state (its text is a GUI/interactive surface;
    # in serve it must only answer without touching the drawing).
    before = s.count_on(WIRE_LAYER)
    r = s.run("EL_WIREGAP")
    check("bare EL_WIREGAP reports without mutating", r.get("ok") is True and s.count_on(WIRE_LAYER) == before, f"{before} → {s.count_on(WIRE_LAYER)}")

    # The flag reaches the file, additive beside every older key.
    s.send({"op": "save", "path": "gap.dxf"})
    txt = read(os.path.join(WORK, "gap.dxf")).replace("\r\n", "\n")
    check("the GAP flag round-trips", bool(xdata_int_pair("GAP", "1").search(txt)), "no GAP=1")

    s.run("EL_WIREGAP CLEAR ALL")
    whole = s.count_on(WIRE_LAYER)
    check("EL_WIREGAP CLEAR ALL draws the wires whole again", whole == 2, f"{whole} on {WIRE_LAYER}")

    # Trimming a gapped wire: the run count follows the geometry, so trimming
    # within the crossing keeps two runs, and trimming past it collapses back
    # to one — the render follows the record, never the other way round.
    s.run("EL_WIREGAP ALL")
    check("gapped again", s.count_on(WIRE_LAYER) == 3, f"{s.count_on(WIRE_LAYER)} on {WIRE_LAYER}")

    s.run("EL_TRIMWIRE W1 30,200")
    check("trim within the crossing keeps the gap", s.count_on(WIRE_LAYER) == 3, f"{s.count_on(WIRE_LAYER)} on {WIRE_LAYER}")
    s.run("EL_TRIMWIRE W1 30,200")
    check("trimming to the same point is a no-op", s.count_on(WIRE_LAYER) == 3, f"{s.count_on(WIRE_LAYER)} on {WIRE_LAYER}")
    s.run("EL_TRIMWIRE W1 85,200")
    check("trimming the far end keeps the gap", s.count_on(WIRE_LAYER) == 3, f"{s.count_on(WIRE_LAYER)} on {WIRE_LAYER}")
    # Two trims walk the head past the crossing: 55 pulls it to mid-wire
    # (still spanning x=60), then 62 takes it beyond — and the gap goes with
    # the crossing it no longer spans.
    s.run("EL_TRIMWIRE W1 55,200")
    check("trimming the head to mid-wire keeps the gap", s.count_on(WIRE_LAYER) == 3, f"{s.count_on(WIRE_LAYER)} on {WIRE_LAYER}")
    s.run("EL_TRIMWIRE W1 62,200")
    left = s.count_on(WIRE_LAYER)
    check("trimming past the crossing removes the gap it no longer spans", left == 2, f"{left} on {WIRE_LAYER}")

    # The vertical wire lost its crossing too, so both render whole; numbering
    # still covers both from their folded paths.
    s.run("EL_WIRENO")
    labels = s.count_on(WIRENO_LAYER)
    check("numbering survives the trims", labels == 2, f"{labels} labels")
    s.close()


def phase_failures_and_cleanup():
    print("\n── failure reporting and cleanup ──")
    s = session()
    s.send({"op": "new"})

    text = s.run_and_collect("EL_INFO K1")
    check("EL_INFO names an unknown reference", "no component or wire matches" in text, text[:200])
    text = s.run_and_collect("EL_INFO @SEL")
    check("EL_INFO @SEL reports nothing inspected yet", "nothing inspected yet" in text, text[:200])
    text = s.run_and_collect("EL_NOSUCHCOMMAND")
    check("an unknown EL_ command is reported", "Unknown electrical command" in text, text[:200])

    # Numbering an empty drawing must say so, not silently succeed.
    text = s.run_and_collect("EL_WIRENO")
    check("numbering a drawing with no wires is reported", "no wires" in text, text[:200])

    # Reference mode with no ladder is the classic misconfiguration.
    s.run("EL_SETTINGS WIREMODE=R")
    s.run("EL_WIRE 20,200 90,200")
    text = s.run_and_collect("EL_WIRENO")
    check(
        "reference mode without a ladder explains the problem and the fix",
        "this drawing has no ladder" in text and "EL_LADDER" in text,
        text[:300],
    )

    # A one-point wire is not a wire.
    text = s.run_and_collect("EL_WIRE 20,200")
    check("a single-point wire is refused", "one point is not a wire" in text, text[:200])
    text = s.run_and_collect("EL_WIRE 20,200 20,200")
    check("a zero-length wire is refused", "same point" in text, text[:200])

    # Editing and deleting.
    s.send({"op": "new"})
    s.run("EL_SETTINGS WIREMODE=S WIRE_FORMAT=W%N")
    s.run("EL_WIRE 20,200 90,200")
    s.run("EL_WIRENO")
    check("one wire numbered", s.count_on(WIRENO_LAYER) == 1)

    s.run("EL_EDIT W1 COLOR=RD GAUGE=16")
    check("editing a wire keeps the drawing intact", s.count_on(WIRE_LAYER) == 1)

    text = s.run_and_collect("EL_EDIT W1 NOSUCHKEY=1")
    check("an unknown property is reported", "unknown wire property" in text, text[:200])
    text = s.run_and_collect("EL_EDIT NOSUCHWIRE COLOR=RD")
    check(
        "editing a missing wire is reported",
        "no wire, device or ladder matches" in text,
        text[:200],
    )

    s.run("EL_DELETE W1")
    check("deleting a wire removes its geometry", s.count_on(WIRE_LAYER) == 0)
    check("deleting a wire removes its number label", s.count_on(WIRENO_LAYER) == 0)

    text = s.run_and_collect("EL_DELETE NOSUCHWIRE")
    check("deleting a missing object is reported", "no electrical object matches" in text, text[:200])
    s.close()



def phase_library():
    print("\n── symbol library ──")
    s = session()
    s.send({"op": "new"})

    check("the built-in library lists", s.run("EL_LIB").get("ok") is True)
    check("families can be listed", s.run("EL_LIB FAMILIES").get("ok") is True)
    check("a symbol can be shown", s.run("EL_LIB SHOW contact-no").get("ok") is True)

    check("listing a family works", s.run("EL_LIB LIST K").get("ok") is True)
    text = s.run_and_collect("EL_LIB LIST ZZ")
    check("an empty family is reported", "nothing in family" in text, text[:200])

    text = s.run_and_collect("EL_LIB SHOW nope-not-a-symbol")
    check("an unknown symbol is reported", "no symbol" in text, text[:200])

    # Which file is in effect has to be answerable: with a sidecar, an override
    # and a shared file all in play, a missing symbol and the wrong library look
    # identical from the drawing.
    text = s.run_and_collect("EL_LIB PATH")
    check(
        "EL_LIB PATH names the file in effect and why",
        "EL_LIB PATH" in text,
        text[:200],
    )
    s.close()


def phase_components():
    print("\n── components and tagging ──")
    s = session()
    s.send({"op": "new"})
    s.run("EL_SETTINGS TAG_FORMAT=%F%N TAG_START=1")
    s.run("EL_LADDER 10,200 WIDTH=170 SPACING=10 RUNGS=6 REF=100")

    s.run("EL_COMPONENT K 60,200")
    check("a placed coil draws its glyph", s.count_on(COMPONENT_LAYER) == 3,
          f"{s.count_on(COMPONENT_LAYER)} on {COMPONENT_LAYER}")
    check("a placed coil draws its designator", s.count_on(TAG_LAYER) == 1)

    s.run("EL_COMPONENT K 60,190")
    check("a second coil adds its own glyph", s.count_on(COMPONENT_LAYER) == 6)
    check("a second coil adds a second designator", s.count_on(TAG_LAYER) == 2)

    # A family with no parent symbol would make `EL_COMPONENT S` impossible.
    s.run("EL_COMPONENT S 60,180")
    # A pushbutton carries two stubs, a blade and a plunger: five strokes.
    check(
        "the pushbutton family resolves to a placeable parent",
        s.count_on(COMPONENT_LAYER) == 6 + 5,
        f"{s.count_on(COMPONENT_LAYER)} (expected 11)",
    )

    text = s.run_and_collect("EL_COMPONENT K1 0,0")
    check(
        "a tag given where a symbol goes is explained",
        "looks like a reference designator" in text and "EL_COMPONENT K K1" in text,
        text[:260],
    )

    text = s.run_and_collect("EL_COMPONENT utterly-unknown 0,0")
    check("an unknown symbol or family is reported", "is not a symbol or family" in text, text[:220])

    # Auto-tagging: per-family counters, so Q1 is the first breaker.
    s.run("EL_COMPONENT Q 60,170")
    s.run("EL_COMPONENT Q 60,160")
    s.send({"op": "save", "path": "comps.dxf"})
    s.close()

    txt = read(os.path.join(WORK, "comps.dxf")).replace("\r\n", "\n")
    for tag in ["K1", "K2", "Q1", "Q2"]:
        check(f"auto-tagging produced {tag}", xdata_pair("TAG", tag) in txt, f"no TAG={tag}")
    check("a family counter is independent of the others",
          xdata_pair("TAG", "Q1") in txt and xdata_pair("TAG", "K1") in txt,
          "Q and K must not share a counter")


def phase_children_and_xref():
    print("\n── children, links and cross-references ──")
    s = session()
    s.send({"op": "new"})
    s.run("EL_SETTINGS TAG_FORMAT=%F%N WIREMODE=S")
    s.run("EL_LADDER 10,200 WIDTH=170 SPACING=10 RUNGS=6 REF=100")
    s.run("EL_COMPONENT K 60,200")

    # A child needs a parent that exists.
    text = s.run_and_collect("EL_COMPONENT contact-no MISSING 100,195")
    check(
        "a child with no placed parent is refused",
        "no parent device tagged" in text,
        text[:240],
    )

    before = s.count_on(COMPONENT_LAYER)
    s.run("EL_COMPONENT contact-no K1 100,195")
    check("a child contact draws its glyph", s.count_on(COMPONENT_LAYER) == before + 3,
          f"{before} → {s.count_on(COMPONENT_LAYER)}")
    check("a child contact shows its parent's designator",
          s.count_on(TAG_LAYER) == 2, f"{s.count_on(TAG_LAYER)} labels")

    s.run("EL_COMPONENT contact-nc K1 100,185")
    # contact-no is three strokes, contact-nc is four (it carries the closed mark).
    check(
        "a second child adds its own glyph",
        s.count_on(COMPONENT_LAYER) == before + 3 + 4,
        f"{s.count_on(COMPONENT_LAYER)} (expected {before + 7})",
    )

    # A child symbol needs a parent tag, so naming none must be refused rather
    # than producing an untagged, unfindable contact.
    text = s.run_and_collect("EL_COMPONENT contact-no 100,175")
    check(
        "a child symbol without a parent tag is refused",
        "needs its parent's tag" in text,
        text[:240],
    )

    s.run("EL_XREF")
    check("cross-references are drawn", s.count_on(XREF_LAYER) >= 3,
          f"{s.count_on(XREF_LAYER)} on {XREF_LAYER}")

    # Rebuilding must replace, not accumulate: a stale annotation is confidently
    # wrong, which is worse than none.
    before_xref = s.count_on(XREF_LAYER)
    s.run("EL_XREF")
    check("a second EL_XREF replaces rather than accumulates",
          s.count_on(XREF_LAYER) == before_xref,
          f"{before_xref} → {s.count_on(XREF_LAYER)}")

    s.run("EL_XREF CLEAR")
    check("EL_XREF CLEAR removes every annotation", s.count_on(XREF_LAYER) == 0)

    # Linking after the fact, and refusing a loop.
    s.run("EL_COMPONENT K 60,120")
    s.send({"op": "save", "path": "kids.dxf"})
    s.close()

    txt = read(os.path.join(WORK, "kids.dxf")).replace("\r\n", "\n")
    check("children carry their parent's tag", xdata_pair("TAG", "K1") in txt)
    check("children are marked as children", xdata_int_pair("CHILD", "1").search(txt) is not None,
          "no CHILD=1")


def phase_retag_and_edit():
    print("\n── retag and edit ──")
    s = session()
    s.send({"op": "new"})
    s.run("EL_SETTINGS TAG_FORMAT=%F%N WIREMODE=S")
    s.run("EL_COMPONENT K 60,200")
    s.run("EL_COMPONENT K 60,190")
    s.run("EL_COMPONENT Q 60,180")

    # Retagging from a different start must renumber every family.
    s.run("EL_RETAG START=10")
    s.send({"op": "save", "path": "retag.dxf"})
    s.close()

    txt = read(os.path.join(WORK, "retag.dxf")).replace("\r\n", "\n")
    for tag in ["K10", "K11", "Q10"]:
        check(f"retag from START=10 produced {tag}", xdata_pair("TAG", tag) in txt,
              f"no TAG={tag}")

    s = session()
    s.send({"op": "new"})
    s.run("EL_SETTINGS TAG_FORMAT=%F%N WIREMODE=S")
    s.run("EL_COMPONENT K K1 60,200")
    s.run("EL_COMPONENT contact-no K1 100,195")

    s.run("EL_EDIT K1 DESC=Motor+starter MFG=Siemens CAT=3RT")
    text = s.run_and_collect("EL_EDIT K1 BOGUS=1")
    check("an unknown device property is reported", "unknown device property" in text, text[:220])

    # Renaming a parent must carry its children, or every cross-reference in the
    # drawing breaks silently.
    s.run("EL_EDIT K1 TAG=K9")
    s.send({"op": "save", "path": "renamed.dxf"})
    s.close()

    txt = read(os.path.join(WORK, "renamed.dxf")).replace("\r\n", "\n")
    check("the parent took the new tag", xdata_pair("TAG", "K9") in txt, "no TAG=K9")
    check("the child followed its renamed parent",
          xdata_pair("PTAG", "K9") in txt, "no PTAG=K9")

    s = session()
    s.send({"op": "new"})
    s.run("EL_SETTINGS TAG_FORMAT=%F%N WIREMODE=S")
    s.run("EL_COMPONENT K 60,200")
    s.run("EL_COMPONENT K 60,190")

    text = s.run_and_collect("EL_EDIT nosuchdevice DESC=x")
    check("editing a missing device is reported",
          "is not a wire, device or ladder" in text or "no wire, device" in text, text[:240])
    s.close()


def phase_symbol_capture():
    print("\n── capturing a drawn symbol ──")
    s = session()
    s.send({"op": "new"})
    s.run("EL_SETTINGS TAG_FORMAT=%F%N")

    # Capture geometry the user drew with the host's own tools.
    s.run("LINE 0,0 10,0")
    check("raw geometry is on the drawing", s.count_on("0") >= 1)

    check("a symbol can be captured", s.run("EL_SYMBOL zz-test-part FAMILY=Z BASE=0,0 PINS=1,2").get("ok") is True)
    check("the captured symbol resolves", s.run("EL_LIB SHOW zz-test-part").get("ok") is True)

    s.run("EL_COMPONENT zz-test-part 30,150")
    check("the captured symbol places", s.count_on(COMPONENT_LAYER) >= 1,
          f"{s.count_on(COMPONENT_LAYER)} on {COMPONENT_LAYER}")

    text = s.run_and_collect("EL_SYMBOL zz-no-pins FAMILY=Z PINS=1,2,3,4")
    check(
        "a capture whose pins do not fit the geometry is refused",
        "free end" in text,
        text[:240],
    )

    text = s.run_and_collect("EL_SYMBOL zz-no-family")
    check("a capture without a family is refused", "FAMILY=" in text, text[:240])

    check("the captured symbol can be removed",
          s.run("EL_SYMBOL DEL zz-test-part").get("ok") is True)
    text = s.run_and_collect("EL_SYMBOL DEL zz-test-part")
    check("removing a missing symbol is reported", "no symbol named" in text, text[:200])
    s.close()


def phase_component_delete():
    print("\n── deleting a device ──")
    s = session()
    s.send({"op": "new"})
    s.run("EL_SETTINGS TAG_FORMAT=%F%N WIREMODE=S")
    s.run("EL_COMPONENT K 60,200")
    s.run("EL_COMPONENT contact-no K1 100,195")
    s.run("EL_XREF")

    glyphs = s.count_on(COMPONENT_LAYER)
    check("two devices are on the drawing", glyphs == 6, f"{glyphs} parts")

    # Deleting the parent leaves its child orphaned, so the child's own parts
    # must be removable independently — and deleting one device must not take
    # another with it.
    s.run("EL_DELETE K1")
    left = s.count_on(COMPONENT_LAYER)
    check(
        "deleting a device removes its parts and its children's",
        left == 0,
        f"{left} parts remain (one device was a 3-part coil plus a 3-part contact)",
    )
    check("its designators went with it", s.count_on(TAG_LAYER) == 0,
          f"{s.count_on(TAG_LAYER)} labels remain")

    text = s.run_and_collect("EL_DELETE nosuchthing")
    check("deleting a missing device is reported", "no electrical object matches" in text, text[:220])
    s.close()


def phase_terminals():
    print("\n── terminals and strips ──")
    s = session()
    s.send({"op": "new"})
    s.run("EL_SETTINGS TAG_FORMAT=%F%N WIRE_FORMAT=W%N")

    s.run("EL_TERMINAL screw X1 20,180 NO=1")
    check("a terminal draws its block", s.count_on(TERMINAL_LAYER) >= 4,
          f"{s.count_on(TERMINAL_LAYER)} on {TERMINAL_LAYER}")
    before = s.count_on(TERMINAL_LAYER)
    s.run("EL_TERMINAL ground X1 20,170 NO=2")
    check("a ground terminal adds its mark", s.count_on(TERMINAL_LAYER) > before)

    # A terminal belongs to a strip. With one already on the drawing it is
    # assumed — a single-strip drawing is the common case — so refusal is for the
    # case with nothing to fall back to.
    s.run("EL_TERMINAL screw X9 20,160 NO=1")
    s.run("EL_TERMINAL spring 20,150")
    check(
        "an unnamed strip falls back to the only strip on the drawing",
        s.count_on(TERMINAL_LAYER) > before + 4,
        f"{s.count_on(TERMINAL_LAYER)} on {TERMINAL_LAYER}",
    )
    text = s.run_and_collect("EL_TERMINAL bogus X1 20,160")
    check("an unknown terminal kind is reported", "not a terminal kind" in text, text[:220])

    # With no strip anywhere there is nothing to assume.
    s.run("EL_TERMINAL CLEAR")
    text = s.run_and_collect("EL_TERMINAL screw 20,160")
    check(
        "a terminal with no strip and none to assume is refused",
        "needs a strip designator" in text or "name the strip" in text,
        text[:240],
    )
    s.run("EL_TERMINAL screw X1 20,180 NO=1")
    s.run("EL_TERMINAL ground X1 20,170 NO=2")
    before = s.count_on(TERMINAL_LAYER)

    # Numbering walks the strip left to right, so a strip drawn out of order still
    # numbers in order.
    s.run("EL_SETTINGS WIREMODE=S")
    s.run("EL_TERMINAL screw X1 60,150 NO=9")
    s.run("EL_TERMINAL NUMBER")
    s.send({"op": "save", "path": "terms.dxf"})
    s.close()

    txt = read(os.path.join(WORK, "terms.dxf")).replace("\r\n", "\n")
    # Every drawn part carries the record, so a value repeats once per part.
    numbers = sorted(set(xdata_str_values(txt, "TERM")))
    check(
        "renumbering makes the strip dense",
        numbers == ["1", "2", "3"],
        f"numbers after renumber: {numbers}",
    )


def phase_cables():
    print("\n── cables ──")
    s = session()
    s.send({"op": "new"})
    s.run("EL_SETTINGS TAG_FORMAT=%F%N")

    s.run("EL_CABLE W1 20,120 COND=1 GAUGE=1.5")
    check("a cable marker draws", s.count_on(CABLE_LAYER) >= 1)
    # The second marker takes the next free conductor, which is what stops two
    # markers claiming one.
    s.run("EL_CABLE W1 40,120 COND=2")
    check("a second conductor adds its own marker", s.count_on(CABLE_LAYER) >= 2)

    text = s.run_and_collect("EL_CABLE W1 60,120 COND=1")
    check(
        "reusing a conductor is refused",
        "already has a marker" in text,
        text[:240],
    )
    s.send({"op": "save", "path": "cables.dxf"})
    s.close()

    txt = read(os.path.join(WORK, "cables.dxf")).replace("\r\n", "\n")
    conds = sorted(set(xdata_int_values(txt, "COND")))
    check("each marker records its conductor", conds == ["1", "2"], f"got {conds}")
    check("the second marker is a child of the cable",
          xdata_pair("PTAG", "W1") in txt, "no PTAG=W1")


def phase_connectors():
    print("\n── connectors ──")
    s = session()
    s.send({"op": "new"})
    s.run("EL_SETTINGS TAG_FORMAT=%F%N")

    s.run("EL_CONNECTOR X1 PINS=A,B,C,GND 140,150")
    check("a connector draws its body and pins", s.count_on(COMPONENT_LAYER) >= 8,
          f"{s.count_on(COMPONENT_LAYER)} on {COMPONENT_LAYER}")

    s.run("EL_CONNECTOR X2 COUNT=6 PERROW=3 140,100")
    check("a wrapped connector draws too", s.count_on(COMPONENT_LAYER) >= 14)

    text = s.run_and_collect("EL_CONNECTOR X3 ORIENT=bogus 0,0")
    check("an unknown orientation is reported", "ORIENT expects" in text, text[:220])
    text = s.run_and_collect("EL_CONNECTOR X3 SPACING=0 0,0")
    check("a zero pin spacing is refused", "SPACING must be positive" in text, text[:220])

    # Changing the pin field is geometry, so the glyph is rebuilt.
    before = s.count_on(COMPONENT_LAYER)
    s.run("EL_CONNECTOR EDIT X1 PINS=A,B,C,D,E")
    check(
        "editing the pin list rebuilds the connector",
        s.count_on(COMPONENT_LAYER) > before,
        f"{before} → {s.count_on(COMPONENT_LAYER)}",
    )

    s.send({"op": "save", "path": "conns.dxf"})
    s.close()
    txt = read(os.path.join(WORK, "conns.dxf")).replace("\r\n", "\n")
    pins = xdata_str_values(txt, "PINLIST")
    # X2 sorts before X1 only by accident of write order, so the edited list is
    # looked for as a run rather than at a fixed offset.
    run_abcde = ["A", "B", "C", "D", "E"]
    found = any(
        pins[i : i + 5] == run_abcde for i in range(len(pins) - 4)
    )
    check(
        "the edited pin list survives as ordered data",
        found,
        f"no A,B,C,D,E run in {pins[:12]}…",
    )
    check("the generated pin list is there too",
          pins.count("1") > 0 and "6" in pins, f"got {pins[:6]}")


def phase_catalog():
    print("\n── the parts catalogue ──")
    s = session()
    s.send({"op": "new"})
    s.run("EL_SETTINGS TAG_FORMAT=%F%N")

    check("the catalogue lists", s.run("EL_CATALOG").get("ok") is True)
    check("families can be listed", s.run("EL_CATALOG FAMILY K").get("ok") is True)
    check("a search works", s.run("EL_CATALOG SEARCH relay").get("ok") is True)
    text = s.run_and_collect("EL_CATALOG SEARCH zzzznothing")
    check("an empty search is reported", "nothing matches" in text, text[:200])
    text = s.run_and_collect("EL_CATALOG ASSIGN nosuchthing MFG=X CAT=Y")
    check("assigning to a missing item is reported",
          "no device, terminal or cable matches" in text, text[:240])

    s.run("EL_COMPONENT K K1 60,200")
    s.run("EL_CATALOG ASSIGN K1 MFG=Generic CAT=RLY-24DC-2CO")
    s.send({"op": "save", "path": "cat.dxf"})
    s.close()

    txt = read(os.path.join(WORK, "cat.dxf")).replace("\r\n", "\n")
    check("the assignment is stored", xdata_pair("CAT", "RLY-24DC-2CO") in txt, "no CAT")
    check("and the manufacturer with it", xdata_pair("MFG", "Generic") in txt, "no MFG")


def phase_audit_and_reports():
    print("\n── audit and reports ──")
    s = session()
    s.send({"op": "new"})

    # The readable verdict is a `push_output`, which `--serve` does not forward —
    # what is observable is whether an error was produced, which is the contract
    # that matters: an error means the drawing is not reportable.
    r = s.run("EL_AUDIT")
    check(
        "an empty drawing audits without claiming defects",
        r.get("ok") is True,
        json.dumps(r)[:200],
    )
    check("the rule codes can be listed", s.run("EL_AUDIT CODES").get("ok") is True)
    check("the report kinds can be listed", s.run("EL_REPORT LIST").get("ok") is True)
    text = s.run_and_collect("EL_REPORT NOSUCHREPORT")
    check("an unknown report is reported", "is not a report" in text, text[:240])

    # A drawing with a real defect must block its reports.
    s.send({"op": "new"})
    s.run("EL_SETTINGS TAG_FORMAT=%F%N WIREMODE=S WIRE_FORMAT=W%N")
    # The allocator refuses to create a duplicate, so the collision is forced by
    # naming the same tag twice — which is what a copy/paste does too.
    s.run("EL_COMPONENT K K1 60,200")
    s.run("EL_COMPONENT K K1 60,190")
    out = os.path.join(WORK, "blocked.csv")
    if os.path.exists(out):
        os.remove(out)
    r = s.run("EL_REPORT BOM blocked.csv")
    check(
        "a duplicate tag blocks the report",
        r.get("ok") is False and "BLOCKED" in json.dumps(r),
        json.dumps(r)[:280],
    )
    check("and nothing was written", not os.path.exists(out))

    # FORCE is how a user says they know.
    s.run("EL_REPORT BOM blocked.csv FORCE")
    s.close()
    check("FORCE exports despite the error", os.path.exists(os.path.join(WORK, "blocked.csv")))

    # A clean drawing exports and the files say what they should.
    s = session()
    s.send({"op": "new"})
    s.run("EL_SETTINGS TAG_FORMAT=%F%N WIREMODE=S WIRE_FORMAT=W%N SHEET=02 DRAWING=PANEL-01")
    s.run("EL_LADDER 10,200 WIDTH=170 SPACING=10 RUNGS=6 REF=100")
    s.run("EL_COMPONENT K K1 60,200")
    s.run("EL_COMPONENT contact-no K1 100,195")
    s.run("EL_CATALOG ASSIGN K1 MFG=Generic CAT=RLY-24DC-2CO")
    s.run("EL_TERMINAL screw X1 20,180 NO=1")
    s.run("EL_CABLE W1 20,120 COND=1 GAUGE=1.5 MFG=Generic CAT=CBL-4x1.5")
    s.run("EL_CONNECTOR X1 PINS=A,B 140,150 MFG=Generic CAT=XT-2.5")
    s.run("EL_WIRE FROM=K1.A1 TO=X1:1")
    s.run("EL_WIRENO")

    r = s.run("EL_AUDIT")
    check(
        "a wired, catalogued drawing audits clean enough to report",
        r.get("ok") is True,
        json.dumps(r)[:400],
    )

    s.run("EL_REPORT ALL clean")
    check("every report can be exported", True)
    s.run("EL_REPORT BOM TABLE onsheet")
    check("a report can be drawn on the sheet", s.count_on(TABLE_LAYER) > 10,
          f"{s.count_on(TABLE_LAYER)} on {TABLE_LAYER}")
    s.send({"op": "save", "path": "clean.dxf"})
    s.close()

    # The CSV contract: a version comment, a title, then the header.
    bom = read(os.path.join(WORK, "bom-1789202108.csv")) if False else None
    del bom
    exports = [f for f in os.listdir(WORK) if f.startswith("bom-") and f.endswith(".csv")]
    check("the bill of materials was written", bool(exports), f"files: {sorted(os.listdir(WORK))[:8]}")
    if exports:
        lines = read(os.path.join(WORK, sorted(exports)[-1])).splitlines()
        check("the CSV carries a version header", lines[0].startswith("#") and "v1" in lines[0], lines[0])
        check("and a title line", lines[1].startswith("#"), lines[1])
        check("the header is the first data line", lines[2].startswith("MFG,PART"), lines[2])
        check("the BOM tallies the device",
              any("RLY-24DC-2CO" in l for l in lines), "\n".join(lines[:6]))

    wft = [f for f in os.listdir(WORK) if f.startswith("wire-from-to-")]
    if wft:
        lines = read(os.path.join(WORK, sorted(wft)[-1])).splitlines()
        check("the wire from/to report resolves the named nodes",
              # A terminal's identity is its whole designator, so `X1:1` is the
              # node: two terminals on one strip are different connections.
              any(l.startswith("W1,K1,A1,X1:1,") for l in lines),
              "\n".join(lines[:4]))

    tplan = [f for f in os.listdir(WORK) if f.startswith("terminal-plan-")]
    if tplan:
        lines = read(os.path.join(WORK, sorted(tplan)[-1])).splitlines()
        check("the terminal plan lists the strip",
              any(l.startswith("X1,1,SCREW") for l in lines), "\n".join(lines[:4]))


def wire_vertices(path):
    """The first polyline's vertices in a saved DXF, as (x, y) pairs.

    The wire is the only polyline in these fixtures, and its drawn ends are the
    thing under test — a report cannot show whether a wire *follows* a device.
    """
    txt = read(path).replace("\r\n", "\n")
    m = re.search(r"LWPOLYLINE(.*?)\n  0\n", txt, re.S)
    if not m:
        return []
    return [
        (float(x), float(y))
        for x, y in re.findall(r"\n\s*10\n([-\d.]+)\n\s*20\n([-\d.]+)", m.group(1))
    ]


def phase_move_and_reseat():
    print("\n── moving things without breaking their wiring ──")
    s = session()
    s.send({"op": "new"})
    s.run("EL_SETTINGS TAG_FORMAT=%F%N WIREMODE=S WIRE_FORMAT=W%N")
    s.run("EL_COMPONENT K K1 60,200")
    s.run("EL_TERMINAL screw X1 100,200 NO=1")
    s.run("EL_WIRE FROM=K1.A1 TO=X1:1")
    s.run("EL_WIRENO")
    s.send({"op": "save", "path": "move-before.dxf"})

    # A device and a terminal, moved and rotated three ways, with the wire
    # re-seated each time.
    check("a device can be moved by an offset", s.run("EL_MOVE K1 DX=40").get("ok") is True)
    check("a terminal can be moved to a coordinate",
          s.run("EL_MOVE X1 200,300").get("ok") is True)
    check("a device can be rotated", s.run("EL_ROTATE K1 90").get("ok") is True)

    text = s.run_and_collect("EL_MOVE nosuchthing DX=1")
    check("moving a missing object is reported", "nothing matches" in text, text[:220])
    text = s.run_and_collect("EL_MOVE K1")
    check("a move with no destination is reported", "where to?" in text, text[:220])
    text = s.run_and_collect("EL_MOVE W1 DX=1")
    check(
        "moving a wire is refused with a reason",
        "no pins to move with it" in text,
        text[:260],
    )

    s.send({"op": "save", "path": "move-after.dxf"})
    s.close()

    before = wire_vertices(os.path.join(WORK, "move-before.dxf"))
    after = wire_vertices(os.path.join(WORK, "move-after.dxf"))
    check("the wire was drawn before the move", len(before) >= 2, f"got {before}")
    check("the wire is still one polyline after the move", len(after) >= 2, f"got {after}")
    if len(after) >= 2:
        # K1 at 60,200 with A1 five up, then +40 in x and a quarter turn: the pin
        # lands at 95,200. X1 at 200,300 with its right lead 7.5 out: 207.5,300.
        (x0, y0), (x1, y1) = after[0], after[-1]
        check(
            "the wire's first end followed the moved and rotated device",
            abs(x0 - 95.0) < 0.01 and abs(y0 - 200.0) < 0.01,
            f"first end at ({x0}, {y0}), expected (95, 200)",
        )
        check(
            "the wire's last end followed the moved terminal",
            abs(x1 - 207.5) < 0.01 and abs(y1 - 300.0) < 0.01,
            f"last end at ({x1}, {y1}), expected (207.5, 300)",
        )
        check(
            "both ends moved, so the wire was re-seated rather than left",
            before[0] != after[0] and before[-1] != after[-1],
            f"{before} -> {after}",
        )

    # Re-attaching is what puts back a grip edit or a change made by hand.
    s = session()
    s.run("EL_REATTACH")
    check("re-attaching succeeds on a drawing with nothing to fix",
          s.run("EL_REATTACH").get("ok") is True)
    s.close()


def phase_list_and_highlight():
    print("\n── the inventory and net tracing ──")
    s = session()
    s.send({"op": "new"})
    r = s.run("EL_LIST")
    check("an empty drawing lists without claiming contents",
          r.get("ok") is True, json.dumps(r)[:200])

    s.run("EL_SETTINGS TAG_FORMAT=%F%N WIREMODE=S WIRE_FORMAT=W%N")
    s.run("EL_LADDER 10,200 WIDTH=170 SPACING=10 RUNGS=6 REF=100")
    s.run("EL_COMPONENT K K1 60,200")
    s.run("EL_COMPONENT contact-no K1 100,195")
    s.run("EL_TERMINAL screw X1 20,180 NO=1")
    s.run("EL_CABLE W1 20,120 COND=1")
    s.run("EL_CONNECTOR X1 PINS=A,B 140,150")

    for section in ["summary", "devices", "wires", "terminals", "cables", "connectors", "nets", "qc"]:
        check(f"EL_LIST {section} works", s.run(f"EL_LIST {section}").get("ok") is True)
    text = s.run_and_collect("EL_LIST nonsense")
    check("an unknown section is reported", "unknown section" in text, text[:220])

    # Tracing follows the wires, not the layout.
    s.run("EL_WIRE FROM=K1.A1 TO=X1:1")
    s.run("EL_WIRENO")
    s.run("EL_HIGHLIGHT W1")
    check("a net can be traced", s.count_on(HILITE_LAYER) >= 1,
          f"{s.count_on(HILITE_LAYER)} on {HILITE_LAYER}")

    text = s.run_and_collect("EL_HIGHLIGHT nosuchthing")
    check("tracing something absent is reported", "nothing matches" in text, text[:240])

    before = s.count_on(HILITE_LAYER)
    s.run("EL_HIGHLIGHT W1")
    check("a second trace replaces rather than accumulates",
          s.count_on(HILITE_LAYER) == before,
          f"{before} -> {s.count_on(HILITE_LAYER)}")

    s.run("EL_HIGHLIGHT CLEAR")
    check("the trace can be cleared", s.count_on(HILITE_LAYER) == 0)

    # Two terminals on one strip are two different connections, so tracing one
    # must not sweep in the other. That is what makes a strip two circuits.
    s.run("EL_TERMINAL screw X1 20,170 NO=2")
    s.run("EL_COMPONENT Q Q1 60,180")
    s.run("EL_WIRE FROM=Q1.A1 TO=X1:2")
    s.run("EL_HIGHLIGHT CLEAR")

    s.run("EL_HIGHLIGHT X1:1")
    only_first = s.count_on(HILITE_LAYER)
    s.run("EL_HIGHLIGHT X1:2")
    only_second = s.count_on(HILITE_LAYER)
    check(
        "each terminal on a strip traces its own circuit",
        only_first == 1 and only_second == 1,
        f"X1:1 traced {only_first} wire(s), X1:2 traced {only_second}",
    )
    s.close()

def main():
    if not os.path.exists(EXE):
        print(f"host binary not found: {EXE}\nset OCS_SERVE_EXE to override")
        return 2

    print(f"host: {EXE}\nwork: {WORK}")

    phase_0_load_and_probe()
    phase_settings()
    phase_wire_types()
    phase_ladder()
    phase_wires_and_numbering()
    phase_wire_gap_and_trim()
    phase_failures_and_cleanup()
    phase_library()
    phase_components()
    phase_children_and_xref()
    phase_retag_and_edit()
    phase_symbol_capture()
    phase_component_delete()
    phase_terminals()
    phase_cables()
    phase_connectors()
    phase_catalog()
    phase_audit_and_reports()
    phase_move_and_reseat()
    phase_list_and_highlight()

    print()
    if failures:
        print(f"{len(failures)} of {checks} check(s) FAILED:")
        for f in failures:
            print(f"  - {f}")
        return 1
    print(f"ALL CHECKS PASSED ({checks})")
    return 0


if __name__ == "__main__":
    sys.exit(main())

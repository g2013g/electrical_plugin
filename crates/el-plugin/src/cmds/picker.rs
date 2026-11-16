//! Symbol picker: a reactive modal dialog (host plugin API 17 reactive
//! dialogs + v19 `DialogControl::Svg`) that browses the library — family
//! dropdown → symbol dropdown → live SVG preview rendered from the same
//! glyph that gets placed — then hands the choice to the ordinary
//! interactive placement (pick the insertion point, rung snap, tag
//! auto-number).
//!
//! `DialogValue` has no Svg variant, so a preview swap cannot ride
//! `UpdateValues`; every cascade instead returns `ReplaceDialog` built by
//! the pure [`dialog_def`] from (family, symbol, tag) — the event's
//! `current_values` snapshot keeps the tag text and current selections
//! across rebuilds.

use ocs_plugin_api::dialog::{
    DialogActionResponse, DialogControl, DialogDef, DialogEvent, DialogResponse, DialogRow,
    DialogValue,
};
use ocs_plugin_api::host::{CommandStep, HostApi, InteractiveCommand};

use el_core::model::ElSettingsRec;
use el_core::ElIndex;
use el_core::symbol::{GlyphPrim, SymbolDef, SymbolLibrary};

use crate::state::{self, PendingLine};

/// Control ids inside the picker dialog.
pub const FAMILY_CTL: &str = "el_pick_family";
pub const SYMBOL_CTL: &str = "el_pick_symbol";
pub const PREVIEW_CTL: &str = "el_pick_preview";
pub const TAG_CTL: &str = "el_pick_tag";

/// `EL_PICK` — bare opens the browser; any argument is the direct insert
/// form (`EL_PICK relay-coil K2 100,80` = `EL_COMPONENT …`), which is what
/// scripts and the `--serve` automation use.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    if !args.is_empty() {
        crate::cmds::component::run(host, args);
        return;
    }
    start_picker(host);
}

/// Open the picker: capture the document context like `EL_COMPONENT`'s
/// interactive path does, then start the command whose `modal_dialog` the
/// host displays automatically.
pub(crate) fn start_picker(host: &mut dyn HostApi) {
    let lib = match crate::library::load(host) {
        Ok(l) => l,
        Err(e) => {
            host.push_error(&format!("EL_PICK: {e}"));
            return;
        }
    };
    if lib.families().is_empty() {
        host.push_error("EL_PICK: the library has no symbols — capture one with EL_SYMBOL first");
        return;
    }
    let capture = crate::hostops::Capture::take(host);
    let settings = capture.index.settings_or_default();
    host.push_info("EL_PICK: choose a symbol, then Insert — pick the insertion point after");
    host.start_interactive(Box::new(PickCmd {
        lib,
        settings,
        idx: capture.index,
        placement: None,
    }));
}

/// The controls of the picker dialog for a given family/symbol selection,
/// with the tag text carried across rebuilds. Pure so the cascade is
/// unit-testable.
fn dialog_def(lib: &SymbolLibrary, family_idx: usize, symbol_idx: usize, tag: &str) -> DialogDef {
    let families = lib.families();
    let family_idx = family_idx.min(families.len().saturating_sub(1));
    let (fam_code, _) = &families[family_idx];
    let syms = lib.by_family(fam_code);
    let symbol_idx = if syms.is_empty() {
        0
    } else {
        symbol_idx.min(syms.len() - 1)
    };
    let def = syms.get(symbol_idx).copied();

    let tag_label = match def {
        Some(d) if d.is_child => "Parent tag (required, e.g. K1)",
        _ => "Tag (empty = auto-number)",
    };
    let hint = match def {
        Some(d) => format!(
            "{} — {} · pins: {}",
            d.name,
            d.description,
            if d.pins.is_empty() {
                "none".to_string()
            } else {
                d.pins.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(" ")
            }
        ),
        None => "this family has no symbols".to_string(),
    };

    let mut dlg = DialogDef::new("Pick a symbol")
        .submit_label("Insert")
        .cancel_label("Cancel")
        .width(380.0)
        .with_row(DialogRow::new(vec![
            DialogControl::Choice {
                id: FAMILY_CTL.to_string(),
                label: "Family".to_string(),
                options: families
                    .iter()
                    .map(|(c, d)| format!("{c} · {d}"))
                    .collect(),
                default_index: family_idx,
            },
            DialogControl::Choice {
                id: SYMBOL_CTL.to_string(),
                label: "Symbol".to_string(),
                options: syms.iter().map(|s| s.name.clone()).collect(),
                default_index: symbol_idx,
            },
        ]))
        .with_control(DialogControl::Svg {
            id: PREVIEW_CTL.to_string(),
            svg: def.map(symbol_svg).unwrap_or_else(empty_svg),
            height: Some(150.0),
        })
        .with_control(DialogControl::InfoText { text: hint, align_h: None })
        .with_control(DialogControl::Text {
            id: TAG_CTL.to_string(),
            label: tag_label.to_string(),
            default: tag.to_string(),
        });
    if def.is_some_and(|d| d.is_child) {
        dlg = dlg.with_control(DialogControl::InfoText {
            text: "A contact belongs to its parent device: the parent must already be placed."
                .to_string(),
            align_h: None,
        });
    }
    dlg
}

/// Walk a dialog's rows down to a flat control list (rows are one level).
#[cfg(test)]
fn controls_of(dlg: &DialogDef) -> Vec<DialogControl> {
    dlg.rows.iter().flat_map(|r| r.controls.clone()).collect()
}

/// A small honest placeholder for a family with no symbols.
fn empty_svg() -> String {
    "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 40 12\" data-symbol=\"none\">\
<text x=\"20\" y=\"8\" font-size=\"4\" text-anchor=\"middle\" fill=\"gray\">no symbols</text></svg>"
        .to_string()
}

/// Render a symbol's glyph as a standalone SVG string. The preview shows
/// exactly the geometry that gets placed: glyph primitives, pins as filled
/// dots with their names. SVG is y-down while the drawing is y-up, so y is
/// mirrored inside the viewBox.
fn symbol_svg(def: &SymbolDef) -> String {
    let [gx0, gy0, gx1, gy1] = def.extent();
    // Pins participate in the extent too (a coil's pins stick out past the
    // glyph box on some symbols).
    let mut ex = (gx0, gy0, gx1, gy1);
    for p in &def.pins {
        ex.0 = ex.0.min(p.at[0]);
        ex.1 = ex.1.min(p.at[1]);
        ex.2 = ex.2.max(p.at[0]);
        ex.3 = ex.3.max(p.at[1]);
    }
    let pad = 2.0;
    let (vx0, _, _, vy1) = (ex.0 - pad, ex.1 - pad, ex.2 + pad, ex.3 + pad);
    let w = (ex.2 + pad - vx0).max(1.0);
    let h = (ex.3 + pad - (ex.1 - pad)).max(1.0);
    // Flip y: y_svg = vy1 - y maps the drawing's y-up box into the SVG's
    // y-down box; the viewBox starts at 0 and every x shifts by -vx0.
    let fy = |y: f64| vy1 - y;
    let fx = |x: f64| x - vx0;

    let mut b = String::with_capacity(2048);
    b.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w:.3} {h:.3}\" \
         preserveAspectRatio=\"xMidYMid meet\" data-symbol=\"{}\">",
        xml_escape(&def.name)
    ));
    let stroke = "stroke=\"black\" stroke-width=\"0.35\" fill=\"none\"";
    for prim in &def.glyph {
        match prim {
            GlyphPrim::Line { a, b: p2 } => b.push_str(&format!(
                "<line x1=\"{:.3}\" y1=\"{:.3}\" x2=\"{:.3}\" y2=\"{:.3}\" {stroke}/>",
                fx(a[0]),
                fy(a[1]),
                fx(p2[0]),
                fy(p2[1])
            )),
            GlyphPrim::Polyline { points, closed } => {
                let pts = points
                    .iter()
                    .map(|p| format!("{:.3},{:.3}", fx(p[0]), fy(p[1])))
                    .collect::<Vec<_>>()
                    .join(" ");
                let (tag, extra) = if *closed {
                    ("polygon", "fill=\"none\"")
                } else {
                    ("polyline", "")
                };
                b.push_str(&format!("<{tag} points=\"{pts}\" {stroke} {extra}/>"));
            }
            GlyphPrim::Circle { center, radius } => b.push_str(&format!(
                "<circle cx=\"{:.3}\" cy=\"{:.3}\" r=\"{:.3}\" {stroke}/>",
                fx(center[0]),
                fy(center[1]),
                radius
            )),
            GlyphPrim::Text { at, height, text } => b.push_str(&format!(
                "<text x=\"{:.3}\" y=\"{:.3}\" font-size=\"{:.3}\" text-anchor=\"middle\" \
                 fill=\"black\" stroke=\"none\" font-family=\"sans-serif\">{}</text>",
                fx(at[0]),
                fy(at[1]) + *height * 0.35,
                height,
                xml_escape(text)
            )),
        }
    }
    for p in &def.pins {
        b.push_str(&format!(
            "<circle cx=\"{:.3}\" cy=\"{:.3}\" r=\"0.5\" fill=\"black\" stroke=\"none\"/>\
<text x=\"{:.3}\" y=\"{:.3}\" font-size=\"1.6\" text-anchor=\"middle\" fill=\"gray\" \
stroke=\"none\" font-family=\"sans-serif\">{}</text>",
            fx(p.at[0]),
            fy(p.at[1]),
            fx(p.at[0]),
            fy(p.at[1]) - 1.0,
            xml_escape(&p.name)
        ));
    }
    b.push_str("</svg>");
    b
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Pull (symbol name, tag text) out of a submitted picker dialog.
fn parse_submission(values: &std::collections::HashMap<String, DialogValue>) -> (String, String) {
    let symbol = match values.get(SYMBOL_CTL) {
        Some(DialogValue::Choice { text, .. }) => text.clone(),
        _ => String::new(),
    };
    let tag = match values.get(TAG_CTL) {
        Some(DialogValue::Text(t)) => t.trim().to_string(),
        _ => String::new(),
    };
    (symbol, tag)
}

/// The index carried by a Choice value, or a fallback.
fn choice_index(v: Option<&DialogValue>, fallback: usize) -> usize {
    match v {
        Some(DialogValue::Choice { index, .. }) => *index,
        _ => fallback,
    }
}

/// True when the event reports an interaction on `control_id` (the host
/// reports "pick" and "change" verbs across control kinds).
fn is_change(control_id: &str, event: &DialogEvent) -> bool {
    event.control_id == control_id && matches!(event.action.as_str(), "change" | "pick" | "click")
}

// ── the interactive command ────────────────────────────────────────────────

struct PickCmd {
    lib: SymbolLibrary,
    settings: ElSettingsRec,
    idx: ElIndex,
    /// Some after the dialog submitted: the ordinary placement command that
    /// owns the rest of the flow (point pick, rung snap, tag auto-number).
    placement: Option<crate::cmds::component::ComponentCmd>,
}

impl InteractiveCommand for PickCmd {
    fn prompt(&self) -> String {
        match &self.placement {
            Some(c) => c.prompt(),
            None => "EL_PICK: choose a symbol, then Insert".to_string(),
        }
    }

    fn modal_dialog(&self) -> Option<DialogDef> {
        // Only while still choosing — once a symbol is submitted the host
        // must collect the insertion point, not re-open the form.
        (self.placement.is_none()).then(|| dialog_def(&self.lib, 0, 0, ""))
    }

    fn on_dialog_action(&mut self, event: DialogEvent) -> DialogActionResponse {
        if self.placement.is_some() {
            return DialogActionResponse::None;
        }
        let tag = match event.current_values.get(TAG_CTL) {
            Some(DialogValue::Text(t)) => t.clone(),
            _ => String::new(),
        };
        let families = self.lib.families();
        let fam_idx = match event.current_values.get(FAMILY_CTL) {
            Some(DialogValue::Choice { index, .. }) => *index,
            _ => 0,
        }
        .min(families.len().saturating_sub(1));

        if is_change(FAMILY_CTL, &event) {
            // A new family: its first symbol, preview follows.
            let idx = choice_index(event.value.as_ref(), 0).min(families.len().saturating_sub(1));
            return DialogActionResponse::ReplaceDialog(Box::new(dialog_def(
                &self.lib, idx, 0, &tag,
            )));
        }
        if is_change(SYMBOL_CTL, &event) {
            let idx = choice_index(event.value.as_ref(), 0);
            return DialogActionResponse::ReplaceDialog(Box::new(dialog_def(
                &self.lib, fam_idx, idx, &tag,
            )));
        }
        DialogActionResponse::None
    }

    fn on_dialog_response(&mut self, response: DialogResponse) -> CommandStep {
        let DialogResponse::Submitted(values) = response else {
            state::emit(PendingLine::Output("EL_PICK: cancelled.".to_string()));
            return CommandStep::Done;
        };
        let (symbol, tag) = parse_submission(&values);
        let Some(def) = self.lib.get(&symbol).cloned() else {
            state::emit(PendingLine::Error(format!(
                "EL_PICK: {symbol:?} vanished from the library"
            )));
            return CommandStep::Done;
        };
        // A child needs its parent's tag; refuse before the host starts
        // collecting a point that could never commit.
        if def.is_child && tag.is_empty() {
            state::emit(PendingLine::Error(format!(
                "EL_PICK: {symbol} is a child symbol — enter its parent's tag (e.g. K1)"
            )));
            return CommandStep::Done;
        }
        self.placement = Some(crate::cmds::component::ComponentCmd::new(
            def,
            self.settings.clone(),
            self.idx.clone(),
            (!tag.is_empty()).then_some(tag),
        ));
        state::emit(PendingLine::Output(format!(
            "EL_PICK: {symbol} — pick the insertion point"
        )));
        CommandStep::NeedPoint
    }

    fn on_point(&mut self, pt: [f64; 3]) -> CommandStep {
        match &mut self.placement {
            Some(c) => c.on_point(pt),
            None => CommandStep::Cancel,
        }
    }

    fn on_enter(&mut self) -> CommandStep {
        match &mut self.placement {
            Some(c) => c.on_enter(),
            None => CommandStep::Cancel,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use el_core::symbol::{ContactState, PinDef};

    fn def(name: &str, family: &str, is_child: bool) -> SymbolDef {
        SymbolDef {
            name: name.to_string(),
            family: family.to_string(),
            description: format!("{name} description"),
            is_child,
            contact: ContactState::default(),
            pins: Vec::new(),
            glyph: Vec::new(),
            default_ratings: Vec::new(),
            tag_offset: [-8.0, 6.0],
        }
    }

    /// An empty library with just the symbols the test upserts (the built-in
    /// starter set has 52 of its own that would pollute the assertions).
    fn bare_lib(defs: &[SymbolDef]) -> SymbolLibrary {
        SymbolLibrary {
            name: "test".to_string(),
            version: 1,
            symbols: defs.to_vec(),
            catalog: Default::default(),
        }
    }

    fn two_family_lib() -> SymbolLibrary {
        bare_lib(&[
            def("relay-coil", "K", false),
            def("contactor-coil", "K", false),
            def("fuse", "F", false),
        ])
    }

    fn coil_with_glyph() -> SymbolDef {
        SymbolDef {
            name: "relay-coil".to_string(),
            family: "K".to_string(),
            description: "relay coil, 2-pin".to_string(),
            is_child: false,
            contact: ContactState::default(),
            pins: vec![
                PinDef { name: "A1".to_string(), at: [0.0, -8.0] },
                PinDef { name: "A2".to_string(), at: [0.0, 8.0] },
            ],
            glyph: vec![
                GlyphPrim::Circle { center: [0.0, 0.0], radius: 6.0 },
                GlyphPrim::Line { a: [0.0, -8.0], b: [0.0, -6.0] },
                GlyphPrim::Text { at: [0.0, 0.0], height: 3.0, text: "K&M".to_string() },
            ],
            default_ratings: Vec::new(),
            tag_offset: [-8.0, 6.0],
        }
    }

    fn control(dlg: &DialogDef, id: &str) -> DialogControl {
        controls_of(dlg)
            .into_iter()
            .find(|c| c.id() == Some(id))
            .unwrap_or_else(|| panic!("control {id} present"))
    }

    #[test]
    fn dialog_lists_families_symbols_and_carries_the_tag() {
        let lib = two_family_lib();
        // families() sorts by code: F (fuse) first, K second. Build with
        // family K, its second symbol.
        let d = dialog_def(&lib, 1, 1, "K7");
        let DialogControl::Choice { options, default_index, .. } = control(&d, FAMILY_CTL) else {
            panic!("family control");
        };
        assert_eq!(options.len(), 2);
        assert_eq!(default_index, 1);
        assert!(options[1].starts_with('K'), "family label keeps its code: {options:?}");
        let DialogControl::Choice { options, default_index, .. } = control(&d, SYMBOL_CTL) else {
            panic!("symbol control");
        };
        assert_eq!(options, vec!["relay-coil".to_string(), "contactor-coil".to_string()]);
        assert_eq!(default_index, 1);
        let DialogControl::Text { default, .. } = control(&d, TAG_CTL) else {
            panic!("tag control");
        };
        assert_eq!(default, "K7");
        let DialogControl::Svg { svg, height, .. } = control(&d, PREVIEW_CTL) else {
            panic!("preview control");
        };
        assert_eq!(height, Some(150.0));
        assert!(svg.contains("data-symbol=\"contactor-coil\""), "{svg}");
    }

    #[test]
    fn child_symbol_dialog_asks_for_the_parent_tag() {
        let lib = bare_lib(&[
            def("relay-coil", "K", false),
            def("contact-no", "K", true),
        ]);
        let d = dialog_def(&lib, 0, 1, "");
        let text = format!("{d:?}");
        assert!(text.contains("Parent tag"), "{text}");
        assert!(text.contains("belongs to its parent"), "{text}");
    }

    #[test]
    fn family_change_swaps_symbol_options_and_keeps_the_tag() {
        let mut cmd = pick_cmd(two_family_lib());
        let event = DialogEvent {
            control_id: FAMILY_CTL.to_string(),
            action: "change".to_string(),
            value: Some(DialogValue::Choice { index: 0, text: "F · fuse description".into() }),
            current_values: std::collections::HashMap::from([
                (
                    FAMILY_CTL.to_string(),
                    DialogValue::Choice { index: 0, text: "K · coils".into() },
                ),
                (TAG_CTL.to_string(), DialogValue::Text("K3".into())),
            ]),
        };
        let DialogActionResponse::ReplaceDialog(d) = cmd.on_dialog_action(event) else {
            panic!("expected ReplaceDialog");
        };
        let DialogControl::Choice { options, default_index, .. } = control(&d, SYMBOL_CTL) else {
            panic!("symbol control");
        };
        assert_eq!(options, vec!["fuse".to_string()]);
        assert_eq!(default_index, 0, "first symbol of the new family");
        let DialogControl::Text { default, .. } = control(&d, TAG_CTL) else {
            panic!("tag control");
        };
        assert_eq!(default, "K3", "tag text survives the rebuild");
    }

    #[test]
    fn symbol_change_swaps_the_preview() {
        let mut cmd = pick_cmd(two_family_lib());
        let event = DialogEvent {
            control_id: SYMBOL_CTL.to_string(),
            action: "change".to_string(),
            value: Some(DialogValue::Choice { index: 1, text: "contactor-coil".into() }),
            current_values: std::collections::HashMap::from([
                (
                    FAMILY_CTL.to_string(),
                    DialogValue::Choice { index: 1, text: "K · relay-coil description".into() },
                ),
            ]),
        };
        let DialogActionResponse::ReplaceDialog(d) = cmd.on_dialog_action(event) else {
            panic!("expected ReplaceDialog");
        };
        let DialogControl::Svg { svg, .. } = control(&d, PREVIEW_CTL) else {
            panic!("preview control");
        };
        assert!(
            svg.contains("data-symbol=\"contactor-coil\""),
            "preview follows the picked symbol: {svg}"
        );
    }

    #[test]
    fn submission_resolves_symbol_and_tag() {
        let values = std::collections::HashMap::from([
            (
                SYMBOL_CTL.to_string(),
                DialogValue::Choice { index: 0, text: "relay-coil".to_string() },
            ),
            (TAG_CTL.to_string(), DialogValue::Text(" K5 ".to_string())),
        ]);
        let (symbol, tag) = parse_submission(&values);
        assert_eq!(symbol, "relay-coil");
        assert_eq!(tag, "K5", "tag is trimmed");
    }

    #[test]
    fn svg_renders_glyph_and_pins_y_flipped() {
        let def = coil_with_glyph();
        let svg = symbol_svg(&def);
        assert!(svg.starts_with("<svg xmlns"), "{svg}");
        assert!(svg.contains("<circle"), "{svg}");
        assert!(svg.contains("<line"), "{svg}");
        assert!(svg.contains("&amp;"), "text is escaped: {svg}");
        assert!(svg.contains("A1") && svg.contains("A2"), "pin names shown: {svg}");
        // Y-flip: A2 sits ABOVE the glyph centre in the drawing, A1 BELOW it,
        // so in the mirrored SVG A2's dot must sit at a smaller y than the
        // circle's and A1's at a larger one. The glyph circle is the r=6
        // element; the pin dots are the r=0.5 ones.
        let circles = svg.split("<circle").skip(1).filter_map(|el| {
            let el = &el[..el.find("/>").unwrap_or(el.len())];
            let num = |key: &str| -> Option<f64> {
                let v = el.split(key).nth(1)?;
                v.trim_start_matches('"').split('"').next()?.parse::<f64>().ok()
            };
            Some((num("r=\"")?, num("cy=\"")?))
        });
        let glyph: Vec<f64> = circles.clone().filter(|(r, _)| *r > 1.0).map(|(_, cy)| cy).collect();
        let pins: Vec<f64> = circles.filter(|(r, _)| *r <= 1.0).map(|(_, cy)| cy).collect();
        assert_eq!(glyph.len(), 1, "one glyph circle: {svg}");
        assert_eq!(pins.len(), 2, "two pin dots: {svg}");
        let cy = glyph[0];
        assert!(
            pins.iter().any(|p| *p < cy) && pins.iter().any(|p| *p > cy),
            "pins straddle the glyph centre in flipped space: glyph {cy}, pins {pins:?}"
        );
    }

    fn pick_cmd(lib: SymbolLibrary) -> PickCmd {
        PickCmd {
            lib,
            settings: ElSettingsRec::default(),
            idx: ElIndex::default(),
            placement: None,
        }
    }
}

//! CSV writing for reports.
//!
//! Separate from [`crate::report`] because the escaping rules are the whole job
//! here, and they are the part that silently corrupts output: a description
//! containing a comma or a quote is ordinary in an electrical BOM, and getting it
//! wrong produces a file that opens with the columns shifted.

use crate::report::ReportKind;
use crate::tables::TableLayout;

/// A version header on every export.
///
/// Consumers of these files — a spreadsheet, a script, a purchasing system — need
/// to know which shape they are reading before they read it, and a version on
/// line one is the cheapest way to say so.
pub const CSV_VERSION: &str = "ocs-electrical report v1";

/// Quote one field if it needs it.
///
/// RFC 4180: wrap in quotes when the value contains a comma, a quote, or a line
/// break, and double any embedded quote. The line-break case matters because a
/// note carried in from a wirelist can contain one, and an unquoted newline
/// splits a row in two.
pub fn field(value: &str) -> String {
    let needs_quotes = value.contains([',', '"', '\n', '\r']);
    if !needs_quotes {
        return value.to_string();
    }
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// Render a table as CSV: a version line, a title line, the header, then rows.
///
/// The title is written as a comment rather than a bare first row, so a reader
/// that treats line 1 as the header is not silently handed a title instead — the
/// exact failure that makes a report look shifted by one row.
pub fn to_csv(layout: &TableLayout) -> String {
    let mut out = String::new();
    out.push_str("# ");
    out.push_str(CSV_VERSION);
    out.push('\n');
    out.push_str("# ");
    out.push_str(&layout.title);
    out.push('\n');

    let headers: Vec<String> = layout.headers.iter().map(|h| field(h)).collect();
    out.push_str(&headers.join(","));
    out.push('\n');

    for r in 0..layout.row_count() {
        let cells: Vec<String> = (0..layout.col_count())
            // The raw value, not the padded cell: padding belongs to the
            // fixed-width rendering, and would add noise to every field here.
            .map(|c| {
                let raw = layout
                    .rows
                    .get(r)
                    .and_then(|row| row.get(c))
                    .map(|s| s.as_str())
                    .unwrap_or("");
                field(raw)
            })
            .collect();
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    out
}

/// A file name for a report, with a timestamp so two exports do not collide.
pub fn file_name(kind: ReportKind, stamp: &str) -> String {
    format!("{}-{stamp}.csv", kind.file_stem())
}

/// Parse one CSV line, honouring quotes.
///
/// Paired with [`field`] so a test can prove the round trip, which is the only
/// way to be sure an escaped description survives.
pub fn parse_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes => {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            }
            '"' => in_quotes = true,
            ',' if !in_quotes => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tables::TableLayout;

    fn table() -> TableLayout {
        TableLayout::new(
            "Test report",
            &["TAG", "DESC"],
            vec![
                vec!["K1".into(), "Relay, 24 VDC".into()],
                vec!["K2".into(), "He said \"go\"".into()],
            ],
        )
    }

    #[test]
    fn a_plain_field_is_not_quoted() {
        assert_eq!(field("K1"), "K1");
        assert_eq!(field(""), "");
    }

    #[test]
    fn a_field_with_a_comma_is_quoted() {
        // Unquoted, this becomes two columns and shifts every later value.
        assert_eq!(field("Relay, 24 VDC"), "\"Relay, 24 VDC\"");
    }

    #[test]
    fn an_embedded_quote_is_doubled() {
        assert_eq!(field("He said \"go\""), "\"He said \"\"go\"\"\"");
    }

    #[test]
    fn a_line_break_is_quoted_or_the_row_splits() {
        assert_eq!(field("line1\nline2"), "\"line1\nline2\"");
        assert_eq!(field("line1\r\nline2"), "\"line1\r\nline2\"");
    }

    #[test]
    fn the_output_starts_with_a_version_comment_and_a_title() {
        let csv = to_csv(&table());
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[0].starts_with('#'), "a reader must see a comment first");
        assert!(lines[0].contains("v1"));
        assert!(lines[1].starts_with('#'));
        assert!(lines[1].contains("Test report"));
        assert_eq!(lines[2], "TAG,DESC", "the header is the first data line");
    }

    #[test]
    fn rows_are_written_unpadded() {
        // Padding belongs to the fixed-width rendering; in CSV it would make
        // every field differ from the value it represents.
        let csv = to_csv(&table());
        assert!(csv.contains("K1,\"Relay, 24 VDC\"\n"), "{csv}");
        assert!(!csv.contains("K1 ,"), "no padding: {csv}");
    }

    #[test]
    fn every_row_has_the_same_field_count() {
        let csv = to_csv(&table());
        let data: Vec<&str> = csv
            .lines()
            .filter(|l| !l.starts_with('#'))
            .collect();
        let counts: Vec<usize> = data.iter().map(|l| parse_line(l).len()).collect();
        assert!(counts.windows(2).all(|w| w[0] == w[1]), "ragged rows: {counts:?}");
        assert_eq!(counts[0], 2);
    }

    #[test]
    fn escaping_round_trips() {
        // The property that matters: what went in comes back out.
        for value in [
            "plain",
            "with, comma",
            "with \"quotes\"",
            "multi\nline",
            "trailing,",
            ",leading",
            "",
        ] {
            let line = field(value);
            let parsed = parse_line(&line);
            assert_eq!(parsed, vec![value.to_string()], "round trip failed for {value:?}");
        }
    }

    #[test]
    fn a_whole_row_round_trips() {
        let cells = vec![
            "K1".to_string(),
            "Relay, 24 VDC".to_string(),
            "He said \"go\"".to_string(),
            "note\nwith newline".to_string(),
        ];
        let line: Vec<String> = cells.iter().map(|c| field(c)).collect();
        assert_eq!(parse_line(&line.join(",")), cells);
    }

    #[test]
    fn an_empty_cell_is_an_empty_field_not_a_dash() {
        // The dash is a *rendering* convention. A machine-read file wants a
        // genuinely empty field, or a consumer would compare against "—".
        let t = TableLayout::new("t", &["A", "B"], vec![vec!["x".into(), "".into()]]);
        let csv = to_csv(&t);
        assert!(csv.contains("x,\n"), "{csv}");
        assert!(!csv.contains('—'), "{csv}");
    }

    #[test]
    fn a_missing_cell_is_an_empty_field() {
        let t = TableLayout::new("t", &["A", "B"], vec![vec!["only".into()]]);
        let line = t;
        let csv = to_csv(&line);
        let data: Vec<&str> = csv.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(data[1], "only,");
    }

    #[test]
    fn an_empty_table_still_writes_its_header() {
        let t = TableLayout::new("Empty", &["A", "B"], vec![]);
        let csv = to_csv(&t);
        assert!(csv.contains("A,B"));
        assert_eq!(csv.lines().filter(|l| !l.starts_with('#')).count(), 1);
    }

    #[test]
    fn file_names_are_distinct_per_report_and_stamped() {
        let a = file_name(ReportKind::Bom, "1700000000");
        assert_eq!(a, "bom-1700000000.csv");
        assert_ne!(a, file_name(ReportKind::TerminalPlan, "1700000000"));
    }
}

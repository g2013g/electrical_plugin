//! Table layout: the shape every report shares.
//!
//! A report is a title, a header row and rows of text. Keeping that as one type
//! means the CSV writer and the on-canvas renderer both consume the same thing,
//! so a report cannot look different in a file than it does in the drawing — a
//! discrepancy that would make the printed output the only trustworthy copy.

/// A laid-out table, with column widths measured in characters.
#[derive(Debug, Clone, PartialEq)]
pub struct TableLayout {
    pub title: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Column widths in characters, sized to the widest cell including the
    /// header so a title is never clipped.
    pub col_widths: Vec<usize>,
}

/// How many characters wide a report column may grow before it is clipped.
///
/// A description is free text and can be arbitrarily long; letting one row set
/// the width would push the table off the sheet.
pub const MAX_COL_CHARS: usize = 42;

/// The replacement for an empty cell.
///
/// An em dash rather than a blank, because a blank cell reads as "not reported"
/// while a dash reads as "nothing to report" — and for a report whose whole job
/// is to be scanned, that distinction matters.
pub const EMPTY_CELL: &str = "—";

impl TableLayout {
    /// Build a layout, sizing every column to its widest cell.
    pub fn new(title: &str, headers: &[&str], rows: Vec<Vec<String>>) -> Self {
        let mut widths: Vec<usize> = headers.iter().map(|h| display_width(h)).collect();
        for row in &rows {
            for (i, cell) in row.iter().enumerate() {
                if i >= widths.len() {
                    widths.resize(i + 1, 0);
                }
                widths[i] = widths[i].max(display_width(cell).min(MAX_COL_CHARS));
            }
        }
        // A table with more columns in its rows than headers would render
        // ragged, so the header list is padded to match.
        let mut headers: Vec<String> = headers.iter().map(|h| h.to_string()).collect();
        while headers.len() < widths.len() {
            headers.push(String::new());
        }
        TableLayout {
            title: title.to_string(),
            headers,
            rows,
            col_widths: widths,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn col_count(&self) -> usize {
        self.col_widths.len()
    }

    /// One cell, clipped to its column and padded out.
    pub fn cell(&self, row: usize, col: usize) -> String {
        let raw = self
            .rows
            .get(row)
            .and_then(|r| r.get(col))
            .map(|s| s.as_str())
            .unwrap_or("");
        let text = if raw.trim().is_empty() {
            EMPTY_CELL.to_string()
        } else {
            raw.to_string()
        };
        match self.col_widths.get(col) {
            // A column that does not exist has no width to clip to; returning the
            // value whole beats returning nothing, which would read as an empty
            // cell rather than as a caller mistake.
            None => text,
            Some(width) => clip(&text, *width),
        }
    }

    /// The header for a column, padded out.
    pub fn header(&self, col: usize) -> String {
        let text = self.headers.get(col).map(|s| s.as_str()).unwrap_or("");
        clip(text, self.col_widths.get(col).copied().unwrap_or(0))
    }

    /// The header and rows in a fixed-width text form, for a report file or a
    /// console printout.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        if !self.title.is_empty() {
            out.push_str(&self.title);
            out.push('\n');
        }
        let header: Vec<String> = (0..self.col_count()).map(|c| self.header(c)).collect();
        out.push_str(header.join(" | ").trim_end());
        out.push('\n');
        out.push_str(
            &self
                .col_widths
                .iter()
                .map(|w| "-".repeat(*w))
                .collect::<Vec<_>>()
                .join("-+-"),
        );
        out.push('\n');
        for r in 0..self.row_count() {
            let cells: Vec<String> = (0..self.col_count()).map(|c| self.cell(r, c)).collect();
            out.push_str(cells.join(" | ").trim_end());
            out.push('\n');
        }
        out
    }

    /// The total rendered width in characters, for placing a title.
    pub fn text_width(&self) -> usize {
        let inner: usize = self.col_widths.iter().sum();
        inner + self.col_widths.len().saturating_sub(1) * 3
    }
}

/// Clip to `width` characters, marking a clip with an ellipsis so a truncated
/// value is never mistaken for the whole one.
pub fn clip(s: &str, width: usize) -> String {
    let w = display_width(s);
    if w <= width {
        return format!("{s}{}", " ".repeat(width - w));
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_string();
    }
    let kept: String = s.chars().take(width - 1).collect();
    format!("{kept}…")
}

/// Character width, counting a multi-byte character as one column.
///
/// Not a monospace metric — it is a stable ordering for column sizing, and the
/// renderer scales it to drawing units.
pub fn display_width(s: &str) -> usize {
    s.chars().count()
}

/// Sort rows by a column, naturally so `W2` precedes `W10`.
pub fn sort_rows(rows: &mut [Vec<String>], col: usize) {
    rows.sort_by(|a, b| {
        let x = a.get(col).map(|s| s.as_str()).unwrap_or("");
        let y = b.get(col).map(|s| s.as_str()).unwrap_or("");
        crate::refdes_cmp(x, y)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> TableLayout {
        TableLayout::new(
            "Test",
            &["REF", "PART"],
            vec![
                vec!["K1".into(), "3RT".into()],
                vec!["K2".into(), "".into()],
                vec!["LONG".into(), "a very long description that will be clipped somewhere".into()],
            ],
        )
    }

    #[test]
    fn columns_size_to_their_widest_cell() {
        let t = table();
        assert_eq!(t.col_count(), 2);
        assert_eq!(t.col_widths[0], 4, "the REF column fits LONG");
        assert_eq!(t.col_widths[1], MAX_COL_CHARS, "a long value is capped");
    }

    #[test]
    fn an_empty_cell_reads_as_nothing_to_report() {
        let t = table();
        assert!(t.cell(1, 1).trim() == EMPTY_CELL, "got {:?}", t.cell(1, 1));
    }

    #[test]
    fn a_missing_cell_does_not_panic() {
        let t = TableLayout::new("t", &["A", "B"], vec![vec!["only-a".into()]]);
        assert_eq!(t.cell(0, 1).trim(), EMPTY_CELL);
        assert_eq!(t.cell(9, 9).trim(), EMPTY_CELL, "out of range is empty, not a panic");
    }

    #[test]
    fn a_clipped_cell_is_marked() {
        let long = "x".repeat(100);
        let t = TableLayout::new("t", &["A"], vec![vec![long.clone()]]);
        let cell = t.cell(0, 0);
        assert!(cell.ends_with('…'), "a truncated value must show it: {cell:?}");
        assert_eq!(display_width(&cell), MAX_COL_CHARS);
    }

    #[test]
    fn rows_wider_than_the_headers_stay_rectangular() {
        // A ragged table would misalign in both the file and the drawing.
        let t = TableLayout::new("t", &["A"], vec![vec!["1".into(), "2".into()]]);
        assert_eq!(t.col_count(), 2);
        assert_eq!(t.headers.len(), 2);
        assert_eq!(t.header(1).trim(), "", "a padded blank header is still blank");
    }

    #[test]
    fn an_empty_table_is_reported_as_empty() {
        let t = TableLayout::new("Nothing", &["A", "B"], vec![]);
        assert!(t.is_empty());
        assert_eq!(t.row_count(), 0);
        // It still renders its header, so "no rows" is visibly different from
        // "no report".
        assert!(t.to_text().contains('A'));
    }

    #[test]
    fn text_output_has_a_header_and_a_rule() {
        let t = table();
        let text = t.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Test");
        assert!(lines[1].starts_with("REF"));
        assert!(lines[2].contains("---"), "a rule separates header from rows");
        assert_eq!(lines.len(), 3 + 3);
    }

    #[test]
    fn text_output_does_not_pad_the_last_column() {
        // Trailing whitespace in a generated file is noise in every diff.
        let t = table();
        for line in t.to_text().lines().skip(1) {
            assert_eq!(line, line.trim_end(), "line has trailing spaces: {line:?}");
        }
    }

    #[test]
    fn rows_sort_naturally_by_a_column() {
        let mut rows = vec![
            vec!["W10".to_string()],
            vec!["W2".to_string()],
            vec!["W1".to_string()],
        ];
        sort_rows(&mut rows, 0);
        assert_eq!(rows[0][0], "W1");
        assert_eq!(rows[1][0], "W2");
        assert_eq!(rows[2][0], "W10");
    }

    #[test]
    fn sorting_tolerates_a_missing_column() {
        let mut rows = vec![vec!["a".to_string()], vec![]];
        sort_rows(&mut rows, 3);
        assert_eq!(rows.len(), 2, "a missing column must not drop rows");
    }

    #[test]
    fn clip_is_exact_at_the_boundary() {
        assert_eq!(clip("abc", 3), "abc");
        assert_eq!(clip("ab", 3), "ab ");
        assert_eq!(clip("abcd", 3), "ab…");
        assert_eq!(clip("abcd", 1), "…");
        assert_eq!(clip("abcd", 0), "");
    }

    #[test]
    fn width_counts_characters_not_bytes() {
        // A description with an accent or a degree sign must not widen a column
        // to twice its rendered size.
        assert_eq!(display_width("2.5 mm²"), 7, "the superscript is one character");
        assert_eq!("2.5 mm²".len(), 8, "but two bytes, which is the trap");
        assert_eq!(clip("2.5 mm²", 20).trim_end(), "2.5 mm²");
    }
}

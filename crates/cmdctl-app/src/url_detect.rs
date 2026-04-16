use cmdctl_daemon::ipc::GridSnapshot;
use regex::Regex;
use std::sync::OnceLock;

/// A URL occurrence within a pane grid.
pub struct UrlSpan {
    pub row: usize,
    pub col_start: usize,
    pub col_end: usize,
    pub url: String,
}

impl UrlSpan {
    pub fn contains(&self, row: usize, col: usize) -> bool {
        self.row == row && col >= self.col_start && col < self.col_end
    }
}

fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"https?://[^\s<>"'`\[\]{}|\\^]+"#).expect("valid url regex")
    })
}

/// Scan a grid snapshot and return URL spans in pane-local coordinates.
pub fn find_urls(grid: &GridSnapshot) -> Vec<UrlSpan> {
    let mut rows: Vec<Vec<(u16, char)>> = vec![Vec::new(); grid.rows as usize];
    for cell in &grid.cells {
        let r = cell.row as usize;
        if r < rows.len() {
            rows[r].push((cell.col, cell.ch));
        }
    }

    let mut spans = Vec::new();
    for (r, mut row) in rows.into_iter().enumerate() {
        row.sort_by_key(|(c, _)| *c);
        let mut text = String::new();
        let mut map: Vec<usize> = Vec::new();
        let mut next_col: u16 = 0;
        for (col, ch) in row {
            while next_col < col {
                map.push(next_col as usize);
                text.push(' ');
                next_col += 1;
            }
            map.push(col as usize);
            text.push(ch);
            next_col = col + 1;
        }

        for m in url_regex().find_iter(&text) {
            let start_char = text[..m.start()].chars().count();
            let end_char = text[..m.end()].chars().count();
            if start_char >= map.len() || end_char == 0 { continue; }

            let mut matched: &str = m.as_str();
            let mut end_char = end_char;
            // Strip trailing punctuation likely to be sentence terminators.
            while let Some(last) = matched.chars().last() {
                if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '"' | '\'' | '`') {
                    matched = &matched[..matched.len() - last.len_utf8()];
                    end_char -= 1;
                } else {
                    break;
                }
            }
            if matched.is_empty() || end_char <= start_char { continue; }

            let col_start = map[start_char];
            let col_end = if end_char - 1 < map.len() { map[end_char - 1] + 1 } else { map[map.len() - 1] + 1 };
            spans.push(UrlSpan {
                row: r,
                col_start,
                col_end,
                url: matched.to_string(),
            });
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmdctl_daemon::ipc::CellData;

    fn grid_from_row(text: &str) -> GridSnapshot {
        let cells = text.chars().enumerate().map(|(i, ch)| CellData {
            col: i as u16,
            row: 0,
            ch,
            fg: [255, 255, 255, 255],
            bg: [0, 0, 0, 255],
            is_cursor: false,
        }).collect();
        GridSnapshot {
            session_id: "t".into(),
            cols: text.chars().count() as u16,
            rows: 1,
            cells,
        }
    }

    #[test]
    fn finds_simple_url() {
        let g = grid_from_row("visit https://example.com today");
        let spans = find_urls(&g);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].url, "https://example.com");
        assert_eq!(spans[0].col_start, 6);
        assert_eq!(spans[0].col_end, 25);
    }

    #[test]
    fn strips_trailing_punctuation() {
        let g = grid_from_row("see https://example.com, then");
        let spans = find_urls(&g);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].url, "https://example.com");
    }

    #[test]
    fn contains_reports_hit() {
        let g = grid_from_row("x https://foo.dev y");
        let spans = find_urls(&g);
        assert_eq!(spans.len(), 1);
        assert!(spans[0].contains(0, 2));
        assert!(spans[0].contains(0, 10));
        assert!(!spans[0].contains(0, 1));
        assert!(!spans[0].contains(0, spans[0].col_end));
    }
}

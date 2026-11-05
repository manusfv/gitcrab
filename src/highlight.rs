//! Small UTF-8 safe lexical cache. Original patch text/coordinates are untouched.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: char,
}
pub fn spans(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut iter = text.char_indices().peekable();
    while let Some((start, c)) = iter.next() {
        if start >= 8192 {
            break;
        }
        let mut end = start + c.len_utf8();
        let kind = if (c == '/' && iter.peek().is_some_and(|(_, n)| *n == '/'))
            || (c == '#' && text[..start].trim().is_empty())
        {
            let mut end = text.len().min(8192);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            out.push(Span {
                start,
                end,
                kind: 'c',
            });
            break;
        } else if c == '\'' || c == '"' {
            let mut escaped = false;
            for (i, n) in iter.by_ref() {
                end = i + n.len_utf8();
                if n == c && !escaped {
                    break;
                }
                escaped = n == '\\' && !escaped;
            }
            's'
        } else if c.is_alphabetic() || c == '_' {
            while let Some(&(i, n)) = iter.peek() {
                if !n.is_alphanumeric() && n != '_' {
                    break;
                }
                iter.next();
                end = i + n.len_utf8();
            }
            if "fn pub let mut impl struct enum use mod if else match return const async await function class def import from true false None Some".split_whitespace().any(|k|k==&text[start..end]) {'k'} else {continue;}
        } else if c.is_ascii_digit() {
            while let Some(&(i, n)) = iter.peek() {
                if !n.is_ascii_digit() && n != '.' {
                    break;
                }
                iter.next();
                end = i + 1;
            }
            'n'
        } else {
            continue;
        };
        out.push(Span { start, end, kind });
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_coordinates_and_lossless_source() {
        let text = "pub fn café() { let x = \"🦀\"; // comment";
        let s = spans(text);
        assert_eq!(&text[s[0].start..s[0].end], "pub");
        assert!(s.iter().any(|r| &text[r.start..r.end] == "\"🦀\""));
        for r in s {
            assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
        }
    }
}

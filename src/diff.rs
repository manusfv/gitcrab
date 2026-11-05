//! Presentation model for Git unified patches. Line coordinates belong to blobs,
//! never to the patch's header lines.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Line {
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub text: String,
    pub kind: char,
    #[serde(skip)]
    pub highlights: Vec<crate::highlight::Span>,
}
pub fn parse(patch: &str) -> Vec<Line> {
    let (mut old, mut new) = (0, 0);
    let mut in_hunk = false;
    patch
        .lines()
        .map(|text| {
            let kind = if text.starts_with("@@ ") {
                let fields: Vec<_> = text.split_whitespace().collect();
                old = fields
                    .get(1)
                    .and_then(|x| x.trim_start_matches('-').split(',').next()?.parse().ok())
                    .unwrap_or(0);
                new = fields
                    .get(2)
                    .and_then(|x| x.trim_start_matches('+').split(',').next()?.parse().ok())
                    .unwrap_or(0);
                in_hunk = true;
                '@'
            } else if text.starts_with("diff --git ") {
                in_hunk = false;
                '@'
            } else if in_hunk {
                text.chars().next().unwrap_or(' ')
            } else {
                '@'
            };
            let (o, n) = match kind {
                '+' => {
                    let n = new;
                    new += 1;
                    (None, Some(n))
                }
                '-' => {
                    let o = old;
                    old += 1;
                    (Some(o), None)
                }
                ' ' => {
                    let (o, n) = (old, new);
                    old += 1;
                    new += 1;
                    (Some(o), Some(n))
                }
                _ => (None, None),
            };
            Line {
                old: o,
                new: n,
                text: text.into(),
                kind,
                highlights: if kind != '@' && !text.is_empty() {
                    crate::highlight::spans(&text[1..])
                        .into_iter()
                        .map(|mut s| {
                            s.start += 1;
                            s.end += 1;
                            s
                        })
                        .collect()
                } else {
                    vec![]
                },
            }
        })
        .collect()
}
pub fn split(lines: &[Line]) -> Vec<(Option<Line>, Option<Line>)> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if matches!(lines[i].kind, '+' | '-') {
            let (mut removed, mut added) = (Vec::new(), Vec::new());
            while i < lines.len() && matches!(lines[i].kind, '+' | '-') {
                if lines[i].kind == '-' {
                    removed.push(lines[i].clone());
                } else {
                    added.push(lines[i].clone());
                }
                i += 1;
            }
            for j in 0..removed.len().max(added.len()) {
                result.push((removed.get(j).cloned(), added.get(j).cloned()));
            }
        } else {
            result.push((Some(lines[i].clone()), Some(lines[i].clone())));
            i += 1;
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blob_coordinates_and_replacement_alignment() {
        let p=parse("--- a/file\n+++ b/file\n@@ -10,3 +10,4 @@\n same\n-old\n+new\n+extra\n tail\n\\ No newline at end of file\n");
        assert_eq!((p[3].old, p[3].new), (Some(10), Some(10)));
        assert_eq!((p[4].old, p[4].new), (Some(11), None));
        assert_eq!((p[6].old, p[6].new), (None, Some(12)));
        assert_eq!((p[7].old, p[7].new), (Some(12), Some(13)));
        let s = split(&p);
        assert_eq!(s[4].0.as_ref().unwrap().text, "-old");
        assert_eq!(s[4].1.as_ref().unwrap().text, "+new");
        assert!(s[5].0.is_none());
    }
}

//! Conservative reverse-hunk planning. Unsupported encodings/filters reject.
use crate::git::Hunk;
pub fn reversed_worktree(hunk: &Hunk, current: &[u8]) -> Result<Vec<u8>, String> {
    let patch = &hunk.patch;
    if [
        "new file mode ",
        "deleted file mode ",
        "rename from ",
        "rename to ",
        "old mode ",
        "new mode ",
        "GIT binary patch",
    ]
    .iter()
    .any(|s| patch.lines().any(|l| l.starts_with(s)))
    {
        return Err(
            "Discard supports ordinary modified text files only. Use Git for this change type."
                .into(),
        );
    }
    let text = std::str::from_utf8(current).map_err(|_| "Discard requires UTF-8 text")?;
    if text.contains('\r') {
        return Err("Discard for CRLF or filtered content is not yet qualified. Use Git.".into());
    }
    let mut lines = patch.split_inclusive('\n');
    let header = lines
        .find(|l| l.starts_with("@@ "))
        .ok_or("Missing hunk header")?;
    let range = header
        .split_whitespace()
        .nth(2)
        .and_then(|s| s.strip_prefix('+'))
        .ok_or("Invalid hunk range")?;
    let mut parts = range.split(',');
    let start = parts
        .next()
        .unwrap()
        .parse::<usize>()
        .map_err(|_| "Invalid hunk start")?;
    let count = parts
        .next()
        .unwrap_or("1")
        .parse::<usize>()
        .map_err(|_| "Invalid hunk count")?;
    let offset_line = if count == 0 {
        start
    } else {
        start.checked_sub(1).ok_or("Invalid zero start")?
    };
    let mut before = String::new();
    let mut after = String::new();
    let mut previous = ' ';
    for line in lines {
        if line.starts_with("\\ No newline at end of file") {
            if matches!(previous, ' ' | '+') && before.ends_with('\n') {
                before.pop();
            }
            if matches!(previous, ' ' | '-') && after.ends_with('\n') {
                after.pop();
            }
            continue;
        }
        let kind = line.chars().next().ok_or("Empty hunk line")?;
        if !matches!(kind, ' ' | '-' | '+') {
            return Err("Unsupported hunk structure".into());
        }
        let content = &line[1..];
        match kind {
            ' ' => {
                before.push_str(content);
                after.push_str(content);
            }
            '+' => before.push_str(content),
            '-' => after.push_str(content),
            _ => return Err("Unsupported hunk structure".into()),
        };
        previous = kind;
    }
    let offset = if offset_line == 0 {
        0
    } else {
        text.split_inclusive('\n')
            .take(offset_line)
            .map(str::len)
            .sum()
    };
    if text.split_inclusive('\n').count() < offset_line
        || !text.get(offset..).is_some_and(|s| s.starts_with(&before))
    {
        return Err("Working file changed. Review a fresh diff before discarding.".into());
    }
    let mut result = current[..offset].to_vec();
    result.extend_from_slice(after.as_bytes());
    result.extend_from_slice(&current[offset + before.len()..]);
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_unrelated_text_unicode_and_no_final_newline() {
        let h=Hunk{title:String::new(),patch:"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -2 +2 @@\n-old\n+日本語\n\\ No newline at end of file\n".into()};
        assert_eq!(
            reversed_worktree(&h, "keep\n日本語".as_bytes()).unwrap(),
            b"keep\nold\n"
        );
        assert!(reversed_worktree(&h, b"keep\nnewer").is_err());
    }
    #[test]
    fn zero_count_inserts_at_the_git_boundary() {
        let h = Hunk {
            title: String::new(),
            patch: "@@ -2 +1,0 @@\n-deleted\n".into(),
        };
        assert_eq!(
            reversed_worktree(&h, b"keep\nlast\n").unwrap(),
            b"keep\ndeleted\nlast\n"
        );
    }
}

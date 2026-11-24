//! Byte-level preflight for reversing a working-tree hunk.

use crate::git::Hunk;

const CRLF_UNSUPPORTED: &str =
    "Discard doesn't support CRLF line endings or filtered files yet. Use Git.";

/// Why Discard cannot reverse hunks of `patch`, a working-tree comparison,
/// judging by the patch alone. The worker checks file attributes and the
/// working file itself again when it runs.
pub fn discard_blocked_reason(patch: &str) -> Option<&'static str> {
    if patch.lines().any(|l| l.starts_with("new file mode ")) {
        return Some("Discard works on files Git already tracks. This file is new; delete it if you no longer need it.");
    }
    if [
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
        return Some(
            "Discard supports ordinary modified text files only. Use Git for this change type.",
        );
    }
    patch.contains('\r').then_some(CRLF_UNSUPPORTED)
}

/// Returns the bytes that reversing a hunk would produce in a working file.
///
/// `current` must contain the current contents of an ordinary UTF-8 file with
/// LF line endings. The hunk must match at its recorded location. The caller
/// handles file attributes and any actual write.
///
/// # Errors
///
/// Returns an error for unsupported text or patch formats, malformed hunk
/// coordinates, or source text that differs from the hunk.
pub fn reversed_worktree(hunk: &Hunk, current: &[u8]) -> Result<Vec<u8>, String> {
    let patch = &hunk.patch;
    if let Some(reason) = discard_blocked_reason(patch) {
        return Err(reason.into());
    }
    let text = std::str::from_utf8(current).map_err(|_| "Discard requires UTF-8 text")?;
    if text.contains('\r') {
        return Err(CRLF_UNSUPPORTED.into());
    }
    // Each hunk's working-file text and its replacement, located by the
    // new-side range, which counts lines in the current file.
    struct Edit {
        offset_line: usize,
        before: String,
        after: String,
    }
    let mut edits: Vec<Edit> = Vec::new();
    let mut previous = ' ';
    for line in patch
        .split_inclusive('\n')
        .skip_while(|l| !l.starts_with("@@ "))
    {
        if line.starts_with("@@ ") {
            let (_, (start, count), _) = parse_header(line)?;
            let offset_line = if count == 0 {
                start
            } else {
                start.checked_sub(1).ok_or("Invalid zero start")?
            };
            edits.push(Edit {
                offset_line,
                before: String::new(),
                after: String::new(),
            });
            previous = ' ';
            continue;
        }
        let edit = edits.last_mut().ok_or("Missing hunk header")?;
        let (before, after) = (&mut edit.before, &mut edit.after);
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
    if edits.is_empty() {
        return Err("Missing hunk header".into());
    }
    let starts: Vec<usize> = text
        .split_inclusive('\n')
        .scan(0, |offset, line| {
            let start = *offset;
            *offset += line.len();
            Some(start)
        })
        .chain([text.len()])
        .collect();
    let mut result = current.to_vec();
    let mut limit = text.len();
    // Splice from the end so earlier offsets stay valid.
    for edit in edits.iter().rev() {
        let offset = *starts
            .get(edit.offset_line)
            .ok_or("Working file changed. Review a fresh diff before discarding.")?;
        let end = offset + edit.before.len();
        if end > limit
            || !text
                .get(offset..)
                .is_some_and(|s| s.starts_with(&edit.before))
        {
            return Err("Working file changed. Review a fresh diff before discarding.".into());
        }
        result.splice(offset..end, edit.after.bytes());
        limit = offset;
    }
    Ok(result)
}

/// A hunk side's start line and line count, as written in its header.
type Range = (usize, usize);

/// Parses `@@ -a,b +c,d @@ rest` into the old range, the new range and the
/// remainder after the closing `@@`, including its line ending.
fn parse_header(line: &str) -> Result<(Range, Range, &str), String> {
    let invalid = || "Invalid hunk header".to_string();
    let body = line.strip_prefix("@@ ").ok_or_else(invalid)?;
    let (ranges, rest) = body.split_once(" @@").ok_or_else(invalid)?;
    let (old, new) = ranges.split_once(' ').ok_or_else(invalid)?;
    let range = |text: &str, sign: char| -> Result<(usize, usize), String> {
        let text = text.strip_prefix(sign).ok_or_else(invalid)?;
        let (start, count) = text.split_once(',').unwrap_or((text, "1"));
        Ok((
            start.parse().map_err(|_| invalid())?,
            count.parse().map_err(|_| invalid())?,
        ))
    };
    Ok((range(old, '-')?, range(new, '+')?, rest))
}

/// Side of a comparison that a partial patch leaves exactly as reviewed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// The before side matches the target; apply the patch forward.
    /// Used to stage lines from the unstaged comparison.
    Before,
    /// The after side matches the target; apply the patch in reverse.
    /// Used to unstage lines from the staged comparison, or to discard
    /// lines from the working file.
    After,
}

/// Builds a patch that carries only the selected changed lines.
///
/// `rows` index the displayed patch lines, which start at the first hunk
/// header (see [`crate::diff::parse`] and the worker's trimming). Unselected
/// changes are dropped or turned into context so the `keep` side still
/// matches its file. Hunks without a selected change are left out.
///
/// # Errors
///
/// Returns a user-facing message when nothing changed is selected, when the
/// selection would leave a missing final newline in the middle of a file,
/// or when only part of a new file would be unstaged or discarded.
pub fn select_lines(patch: &str, rows: &[usize], keep: Keep) -> Result<Hunk, String> {
    let lines: Vec<&str> = patch.split_inclusive('\n').collect();
    let first = lines
        .iter()
        .position(|l| l.starts_with("@@ "))
        .ok_or("This comparison has no text hunks")?;
    let selected: std::collections::HashSet<usize> = rows.iter().map(|row| first + row).collect();
    let mut output = String::new();
    let mut file_header: Option<(usize, usize)> = None;
    let mut header_written = false;
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if line.starts_with("diff --git ") {
            let start = index;
            while index < lines.len() && !lines[index].starts_with("@@ ") {
                if index > start && lines[index].starts_with("diff --git ") {
                    break;
                }
                index += 1;
            }
            file_header = Some((start, index));
            header_written = false;
            continue;
        }
        if !line.starts_with("@@ ") {
            index += 1;
            continue;
        }
        let (old, new, rest) = parse_header(line)?;
        let body_start = index + 1;
        let mut end = body_start;
        while end < lines.len()
            && !lines[end].starts_with("@@ ")
            && !lines[end].starts_with("diff --git ")
        {
            end += 1;
        }
        index = end;
        let chosen = (body_start..end).any(|i| {
            selected.contains(&i) && matches!(lines[i].as_bytes().first(), Some(b'+' | b'-'))
        });
        if !chosen {
            continue;
        }
        let header = file_header
            .map(|(start, end)| lines[start..end].concat())
            .unwrap_or_default();
        // Source lines as (kind, text without prefix, newline notice, selected).
        let mut body: Vec<(char, &str, bool, bool)> = Vec::new();
        for (i, text) in lines.iter().enumerate().take(end).skip(body_start) {
            match text.chars().next().unwrap_or(' ') {
                '\\' => {
                    if let Some(last) = body.last_mut() {
                        last.2 = true;
                    }
                }
                kind @ (' ' | '+' | '-') => {
                    body.push((kind, &text[1..], false, selected.contains(&i)))
                }
                _ => return Err("Unsupported hunk structure".into()),
            }
        }
        let partial_new_file = body.iter().any(|(kind, _, _, pick)| *kind == '+' && !pick);
        // Kept lines as (kind, text without prefix, followed by a newline notice).
        let mut kept: Vec<(char, &str, bool)> = Vec::new();
        let mut position = 0;
        while position < body.len() {
            if body[position].0 == ' ' {
                let (_, text, notice, _) = body[position];
                kept.push((' ', text, notice));
                position += 1;
                continue;
            }
            // A change block: removals, then additions, as Git writes them.
            let block_end = body[position..]
                .iter()
                .position(|line| line.0 == ' ')
                .map_or(body.len(), |offset| position + offset);
            let block = &body[position..block_end];
            position = block_end;
            let minus: Vec<_> = block.iter().filter(|l| l.0 == '-').collect();
            let plus: Vec<_> = block.iter().filter(|l| l.0 == '+').collect();
            fn line<'a>(l: &(char, &'a str, bool, bool)) -> (char, &'a str, bool) {
                (l.0, l.1, l.2)
            }
            fn context<'a>(l: &(char, &'a str, bool, bool)) -> (char, &'a str, bool) {
                (' ', l.1, l.2)
            }
            match keep {
                // Unselected removals stay as context. Those after the last
                // selected removal follow the additions, so a partly staged
                // replacement keeps both sides in order.
                Keep::Before => {
                    let split = minus.iter().rposition(|l| l.3).map_or(0, |last| last + 1);
                    for l in &minus[..split] {
                        kept.push(if l.3 { line(l) } else { context(l) });
                    }
                    kept.extend(plus.iter().filter(|l| l.3).map(|l| line(l)));
                    kept.extend(minus[split..].iter().map(|l| context(l)));
                }
                // Unselected additions stay as context. Those before the
                // first selected addition precede the removals.
                Keep::After => {
                    let split = plus.iter().position(|l| l.3).unwrap_or(0);
                    kept.extend(plus[..split].iter().map(|l| context(l)));
                    kept.extend(minus.iter().filter(|l| l.3).map(|l| line(l)));
                    for l in &plus[split..] {
                        kept.push(if l.3 { line(l) } else { context(l) });
                    }
                }
            }
        }
        if keep == Keep::After
            && partial_new_file
            && header.lines().any(|l| l.starts_with("new file mode "))
        {
            return Err(
                "Part of a new file cannot be removed on its own. Select all of its lines, or use the whole-file action."
                    .into(),
            );
        }
        // A line without a final newline must stay last on each side it is on.
        let sides = |kind: char| match kind {
            '-' => (true, false),
            '+' => (false, true),
            _ => (true, true),
        };
        for (position, (kind, _, notice)) in kept.iter().enumerate() {
            if !notice {
                continue;
            }
            let (old_side, new_side) = sides(*kind);
            if kept[position + 1..].iter().any(|(later, _, _)| {
                let (o, n) = sides(*later);
                (old_side && o) || (new_side && n)
            }) {
                return Err(
                    "This selection would move the file's missing final newline. Include the last lines of the file, or use the whole hunk."
                        .into(),
                );
            }
        }
        let old_count = kept.iter().filter(|(k, _, _)| *k != '+').count();
        let new_count = kept.iter().filter(|(k, _, _)| *k != '-').count();
        // The kept side keeps its range; the other side starts at the same line.
        let first_line =
            |(start, count): (usize, usize)| if count == 0 { start + 1 } else { start };
        let start = |count: usize, line: usize| if count == 0 { line - 1 } else { line };
        let (old_start, new_start) = match keep {
            Keep::Before => (old.0, start(new_count, first_line(old))),
            Keep::After => (start(old_count, first_line(new)), new.0),
        };
        if !header_written {
            output.push_str(&header);
            header_written = true;
        }
        output.push_str(&format!(
            "@@ -{old_start},{old_count} +{new_start},{new_count} @@{rest}"
        ));
        if !rest.ends_with('\n') {
            output.push('\n');
        }
        for (kind, text, notice) in kept {
            output.push(kind);
            output.push_str(text);
            if !text.ends_with('\n') {
                output.push('\n');
            }
            if notice {
                output.push_str("\\ No newline at end of file\n");
            }
        }
    }
    if output.is_empty() {
        return Err("Select one or more added or removed lines.".into());
    }
    Ok(Hunk {
        title: "Selected lines".into(),
        patch: output,
    })
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
    fn selected_lines_keep_one_side_and_recount_ranges() {
        let patch = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,4 +1,4 @@ fn x\n keep\n-old1\n-old2\n+new1\n+new2\n tail\n";
        // Display rows: 0 header, 1 keep, 2 -old1, 3 -old2, 4 +new1, 5 +new2, 6 tail.
        let staged = select_lines(patch, &[2, 4], Keep::Before).unwrap();
        assert!(staged
            .patch
            .ends_with("@@ -1,4 +1,4 @@ fn x\n keep\n-old1\n+new1\n old2\n tail\n"));
        let unstaged = select_lines(patch, &[3, 5], Keep::After).unwrap();
        assert!(unstaged
            .patch
            .ends_with("@@ -1,4 +1,4 @@ fn x\n keep\n new1\n-old2\n+new2\n tail\n"));
        let removal = select_lines(patch, &[2], Keep::Before).unwrap();
        assert!(removal
            .patch
            .ends_with("@@ -1,4 +1,3 @@ fn x\n keep\n-old1\n old2\n tail\n"));
        assert!(select_lines(patch, &[1, 6], Keep::Before).is_err());
    }
    #[test]
    fn selected_lines_skip_untouched_hunks_and_guard_missing_newlines() {
        let patch = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n@@ -9,2 +9,2 @@\n x\n-y\n\\ No newline at end of file\n+y\n";
        // Rows: 0 @@, 1 -a, 2 +b, 3 @@, 4 x, 5 -y, 6 \, 7 +y.
        let first = select_lines(patch, &[1, 2], Keep::Before).unwrap();
        assert_eq!(first.patch.matches("@@ ").count(), 1);
        // The kept "y" without a newline stays last; the addition goes before it.
        let added_only = select_lines(patch, &[7], Keep::Before).unwrap();
        assert!(added_only
            .patch
            .ends_with("@@ -9,2 +9,3 @@\n x\n+y\n y\n\\ No newline at end of file\n"));
        // A context line without a final newline cannot sit before other lines.
        let moved = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,1 @@\n-a\n-b\n\\ No newline at end of file\n+c\n\\ No newline at end of file\n";
        assert!(select_lines(moved, &[1], Keep::Before).is_ok());
        assert!(select_lines(moved, &[4], Keep::Before).is_err());
        let whole = select_lines(patch, &[5, 7], Keep::Before).unwrap();
        assert!(whole
            .patch
            .ends_with("@@ -9,2 +9,2 @@\n x\n-y\n\\ No newline at end of file\n+y\n"));
        // Removing a new file's lines one at a time is refused.
        let added = "diff --git a/n b/n\nnew file mode 100644\n--- /dev/null\n+++ b/n\n@@ -0,0 +1,2 @@\n+a\n+b\n";
        assert!(select_lines(added, &[1], Keep::After).is_err());
        let staged = select_lines(added, &[2], Keep::Before).unwrap();
        assert!(staged.patch.ends_with("@@ -0,0 +1,1 @@\n+b\n"));
    }
    #[test]
    fn discards_several_hunks_from_the_bottom_up() {
        let h = Hunk {
            title: String::new(),
            patch: "@@ -1 +1 @@\n-a\n+A\n@@ -3 +3,2 @@\n-c\n+C\n+D\n".into(),
        };
        assert_eq!(
            reversed_worktree(&h, b"A\nb\nC\nD\ne\n").unwrap(),
            b"a\nb\nc\ne\n"
        );
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

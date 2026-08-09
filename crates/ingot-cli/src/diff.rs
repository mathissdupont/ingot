//! A unified diff, for proposing a source change instead of making one.
//!
//! Model assistance on an existing project must show what it would write before
//! it writes anything, and a review must show what a proposal changed rather
//! than printing a wall of source. Both need the same thing: a review-shaped
//! rendering of two texts.
//!
//! Line-based and LCS-derived. That is enough for a `.ing` file, and it avoids a
//! dependency whose licence would have to be cleared for something this small.
//! Both sides are normalised to end with a newline before comparison, so the
//! rendering never needs the `\ No newline at end of file` marker — every source
//! this writes ends with one.

/// Above this many line pairs the quadratic table stops being worth building,
/// and the diff degrades to one replace-everything hunk rather than allocating
/// gigabytes. No `.ing` source comes close; a pasted artifact might.
const MAX_TABLE_CELLS: usize = 4_000_000;

/// How many unchanged lines surround each hunk.
pub(crate) const CONTEXT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Keep,
    Remove,
    Insert,
}

/// Render `old` -> `new` as a unified diff, or `None` when they are identical.
pub(crate) fn unified(
    old_label: &str,
    old: &str,
    new_label: &str,
    new: &str,
    context: usize,
) -> Option<String> {
    let old = normalise(old);
    let new = normalise(new);
    if old == new {
        return None;
    }

    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let script = script(&old_lines, &new_lines);

    let mut out = format!("--- {old_label}\n+++ {new_label}\n");
    for hunk in hunks(&script, context) {
        out.push_str(&render(&hunk, &script, &old_lines, &new_lines));
    }
    Some(out)
}

/// A `.ing` source always ends with a newline, so comparing as if it does keeps
/// "the file gained a trailing newline" out of the review.
fn normalise(text: &str) -> String {
    if text.is_empty() || text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    }
}

/// One step of the edit script, with the line it applies to.
#[derive(Debug, Clone, Copy)]
struct Step {
    op: Op,
    /// Index into the old lines, for `Keep` and `Remove`.
    old: usize,
    /// Index into the new lines, for `Keep` and `Insert`.
    new: usize,
}

fn script(old: &[&str], new: &[&str]) -> Vec<Step> {
    if old.len().saturating_mul(new.len()) > MAX_TABLE_CELLS {
        // Honest degradation: the whole file is replaced, which is true, rather
        // than a minimal diff nobody can afford to compute.
        let mut steps: Vec<Step> = (0..old.len())
            .map(|index| Step {
                op: Op::Remove,
                old: index,
                new: 0,
            })
            .collect();
        steps.extend((0..new.len()).map(|index| Step {
            op: Op::Insert,
            old: old.len(),
            new: index,
        }));
        return steps;
    }

    // Longest common subsequence, filled from the end so the walk forward below
    // reads naturally.
    let width = new.len() + 1;
    let mut table = vec![0usize; (old.len() + 1) * width];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            table[i * width + j] = if old[i] == new[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }

    let mut steps = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            steps.push(Step {
                op: Op::Keep,
                old: i,
                new: j,
            });
            i += 1;
            j += 1;
        } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
            steps.push(Step {
                op: Op::Remove,
                old: i,
                new: j,
            });
            i += 1;
        } else {
            steps.push(Step {
                op: Op::Insert,
                old: i,
                new: j,
            });
            j += 1;
        }
    }
    while i < old.len() {
        steps.push(Step {
            op: Op::Remove,
            old: i,
            new: j,
        });
        i += 1;
    }
    while j < new.len() {
        steps.push(Step {
            op: Op::Insert,
            old: i,
            new: j,
        });
        j += 1;
    }
    steps
}

/// Ranges of the script that belong to one hunk, as `start..end` step indices.
fn hunks(script: &[Step], context: usize) -> Vec<std::ops::Range<usize>> {
    let changed: Vec<usize> = script
        .iter()
        .enumerate()
        .filter(|(_, step)| step.op != Op::Keep)
        .map(|(index, _)| index)
        .collect();

    let mut hunks: Vec<std::ops::Range<usize>> = Vec::new();
    for index in changed {
        let start = index.saturating_sub(context);
        let end = (index + context + 1).min(script.len());
        match hunks.last_mut() {
            // Overlapping or touching context blocks read as one hunk.
            Some(last) if start <= last.end => last.end = last.end.max(end),
            _ => hunks.push(start..end),
        }
    }
    hunks
}

fn render(range: &std::ops::Range<usize>, script: &[Step], old: &[&str], new: &[&str]) -> String {
    let steps = &script[range.clone()];
    let old_start = steps.first().map(|step| step.old).unwrap_or(0);
    let new_start = steps.first().map(|step| step.new).unwrap_or(0);
    let old_count = steps.iter().filter(|step| step.op != Op::Insert).count();
    let new_count = steps.iter().filter(|step| step.op != Op::Remove).count();

    // Unified diff line numbers are 1-based, and a zero-length side is written
    // as the line it would follow.
    let mut out = format!(
        "@@ -{},{} +{},{} @@\n",
        if old_count == 0 {
            old_start
        } else {
            old_start + 1
        },
        old_count,
        if new_count == 0 {
            new_start
        } else {
            new_start + 1
        },
        new_count,
    );
    for step in steps {
        match step.op {
            Op::Keep => out.push_str(&format!(" {}\n", old[step.old])),
            Op::Remove => out.push_str(&format!("-{}\n", old[step.old])),
            Op::Insert => out.push_str(&format!("+{}\n", new[step.new])),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_texts_have_no_diff() {
        assert!(unified("a", "one\ntwo\n", "b", "one\ntwo\n", CONTEXT).is_none());
    }

    #[test]
    fn a_missing_trailing_newline_is_not_a_change() {
        assert!(unified("a", "one\ntwo", "b", "one\ntwo\n", CONTEXT).is_none());
    }

    #[test]
    fn a_changed_line_is_shown_with_context() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let new = "a\nb\nc\nD\ne\nf\ng\nh\n";
        let diff = unified("main.ing", old, "proposed", new, CONTEXT).expect("a change");
        assert!(diff.starts_with("--- main.ing\n+++ proposed\n"), "{diff}");
        assert!(diff.contains("@@ -1,7 +1,7 @@\n"), "{diff}");
        assert!(diff.contains("-d\n"), "{diff}");
        assert!(diff.contains("+D\n"), "{diff}");
        // The eighth line is beyond the context window, so it stays out.
        assert!(!diff.contains(" h\n"), "{diff}");
    }

    #[test]
    fn distant_changes_become_separate_hunks() {
        let old: String = (0..40).map(|n| format!("line {n}\n")).collect();
        let new = old
            .replace("line 1\n", "line one\n")
            .replace("line 30\n", "line thirty\n");
        let diff = unified("old", &old, "new", &new, CONTEXT).expect("a change");
        assert_eq!(diff.matches("@@ ").count(), 2, "{diff}");
    }

    #[test]
    fn creating_a_file_is_one_insertion_hunk() {
        let diff = unified("nothing", "", "main.ing", "language 0.1\n", CONTEXT).expect("a change");
        assert!(diff.contains("@@ -0,0 +1,1 @@\n"), "{diff}");
        assert!(diff.contains("+language 0.1\n"), "{diff}");
    }

    #[test]
    fn every_removed_and_added_line_is_accounted_for() {
        let old = "one\ntwo\nthree\n";
        let new = "one\nthree\nfour\n";
        let diff = unified("old", old, "new", new, CONTEXT).expect("a change");
        let body: Vec<&str> = diff.lines().skip(2).collect();
        assert_eq!(
            body.iter().filter(|line| line.starts_with('-')).count(),
            1,
            "{diff}"
        );
        assert_eq!(
            body.iter().filter(|line| line.starts_with('+')).count(),
            1,
            "{diff}"
        );
        assert!(diff.contains("-two\n"), "{diff}");
        assert!(diff.contains("+four\n"), "{diff}");
    }
}

use std::io;
use std::process::Output;

use klik_core::module::WindowMatch;
use klik_core::platform::{Process, Run};

use crate::HELPER_LIMIT;

/// A window id as `kdotool getactivewindow` prints it, with the tool's braces
/// and trailing newline taken off, and `None` when it named no window.
///
/// KWin's `getWindowInfo` returned the same map with or without the braces on
/// KWin 6.7.90 on 2026-09-22, so trimming them leaves the id in one form to log
/// and to pass on rather than meeting a requirement. The `None` is the part
/// that decides something: it is how a caller learns there is no window in
/// front.
pub fn parse_window(text: &str) -> Option<String> {
    let trimmed = text.trim().trim_start_matches('{').trim_end_matches('}');
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

/// A `kdotool search` pattern that matches exactly this text.
///
/// `kdotool` builds `new RegExp(String.raw`<pattern>`, "i")` and asks
/// `caption.search(re) >= 0`, read out of `kdotool -n` on 2026-09-27. That is
/// an unanchored match, so the anchors are what stop a shorter name from
/// matching a longer one, and the escaping is what stops a name like `c++`
/// from being read as a repetition.
///
/// `None` means the text cannot be expressed as a pattern. A backtick ends
/// the `String.raw` literal and `${` opens an interpolation, and neither can
/// be escaped inside one, so such a text is refused rather than handed to a
/// script interpreter.
pub fn search_pattern(text: &str) -> Option<String> {
    if text.is_empty() || text.contains('`') || text.contains("${") {
        return None;
    }
    let mut pattern = String::with_capacity(text.len() + 2);
    pattern.push('^');
    for c in text.chars() {
        if r"\^$.|?*+()[]{}/-".contains(c) {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push('$');
    Some(pattern)
}

/// The ids `kdotool search` printed, one per line, without the tool's braces.
pub fn parse_windows(text: &str) -> Vec<String> {
    text.lines().filter_map(parse_window).collect()
}

/// What a finished `kdotool` printed, or why it failed.
///
/// `Process::output` fails when the process cannot be started or does not
/// end in time. A `kdotool` that ran and exited with a failure is an `Ok`
/// there, so without this its failure reads as a search that found nothing.
/// Measured on 2026-09-28: a search for the pattern `(` exits with 1 and says
/// `Script error: SyntaxError: Invalid regular expression` on stderr.
pub fn printed(out: Output) -> io::Result<String> {
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "kdotool failed with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn kdotool(process: &mut dyn Process, args: &[&str]) -> io::Result<String> {
    let run = Run {
        program: "kdotool".to_string(),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        env: Vec::new(),
        limit: HELPER_LIMIT,
    };
    printed(process.output(&run)?)
}

/// Which window has keyboard focus.
///
/// KWin's own D-Bus does not answer this, so it goes through `kdotool`.
/// Measured at 3.88 ms per call on 2026-09-19 against a 1.15 ms bare process
/// spawn.
pub fn active_window(process: &mut dyn Process) -> io::Result<Option<String>> {
    Ok(parse_window(&kdotool(process, &["getactivewindow"])?))
}

/// Every window whose class or caption is exactly the text. `-C` makes the
/// match case sensitive; without it `kdotool` searches case insensitively.
pub fn search(process: &mut dyn Process, which: &WindowMatch) -> io::Result<Vec<String>> {
    let (field, text) = match which {
        WindowMatch::Class(class) => ("--class", class),
        WindowMatch::Title(title) => ("--title", title),
    };
    let Some(pattern) = search_pattern(text) else {
        return Ok(Vec::new());
    };
    Ok(parse_windows(&kdotool(
        process,
        &["search", "-C", field, &pattern],
    )?))
}

/// Activate a window, switching desktop if it is on another one, and confirm
/// that it took the focus.
///
/// `kdotool windowactivate` does both. The id is passed back in braces, which
/// is the form the script compares `internalId` with.
///
/// The exit status cannot say whether anything was activated: the script
/// walks the window list and activates the one that carries the id, and a
/// list without such a window is no error to it, read out of
/// `kdotool -n windowactivate` on 2026-09-28. So the window that holds the
/// focus afterwards is asked for, and anything but the one wanted is a
/// failure.
pub fn activate(process: &mut dyn Process, id: &str) -> io::Result<()> {
    kdotool(process, &["windowactivate", &format!("{{{id}}}")])?;
    if active_window(process)?.as_deref() != Some(id) {
        return Err(io::Error::other(format!(
            "window {id} did not take the focus"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    use super::*;

    /// A process that has finished, with the exit code and the two streams
    /// given. `from_raw` takes a wait status, which holds the code in its
    /// second byte.
    fn finished(code: i32, stdout: &str, stderr: &str) -> Output {
        Output {
            status: ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    const ID: &str = "11111111-2222-3333-4444-555555555555";

    #[test]
    fn a_kdotool_that_succeeded_gives_what_it_printed() {
        let out = finished(0, &format!("{{{ID}}}\n"), "");
        assert_eq!(printed(out).expect("a success"), format!("{{{ID}}}\n"));
    }

    /// The exit code and the message measured on 2026-09-28 for a search with
    /// the pattern `(`. Without the check this is a search that found nothing.
    #[test]
    fn a_kdotool_that_failed_is_an_error_that_says_why() {
        let out = finished(
            1,
            "",
            "ERROR: Script error: SyntaxError: Invalid regular expression\n",
        );
        let error = printed(out).expect_err("a failure");
        assert!(
            error.to_string().contains("Invalid regular expression"),
            "{error}"
        );
    }

    /// The braces are the tool's, as `kdotool getactivewindow` printed them on
    /// 2026-09-19.
    #[test]
    fn the_window_id_loses_its_braces() {
        assert_eq!(parse_window(&format!("{{{ID}}}")), Some(ID.to_string()));
    }

    /// `kdotool`'s real stdout ends in a newline. Without the leading `trim()`,
    /// `trim_end_matches('}')` finds a `\n` at the end instead of the brace and
    /// leaves it.
    #[test]
    fn a_trailing_newline_from_the_command_is_not_part_of_the_id() {
        assert_eq!(parse_window(&format!("{{{ID}}}\n")), Some(ID.to_string()));
    }

    #[test]
    fn no_active_window_is_refused_rather_than_returned_empty() {
        assert_eq!(parse_window(""), None);
        assert_eq!(parse_window("{}"), None);
    }

    #[test]
    fn a_plain_title_is_anchored_and_otherwise_unchanged() {
        assert_eq!(
            search_pattern("VSCodium - klik").as_deref(),
            Some("^VSCodium \\- klik$")
        );
    }

    /// `kdotool` searches with `String.search`, which matches anywhere in the
    /// caption, so without the anchors a shorter name finds every window whose
    /// name starts with it.
    #[test]
    fn a_shorter_title_no_longer_matches_a_longer_caption() {
        let pattern = search_pattern("VSCodium - kli").expect("a pattern");
        assert!(pattern.starts_with('^') && pattern.ends_with('$'));
    }

    /// Every JavaScript regular expression metacharacter, escaped. A name like
    /// `c++` is the everyday case: unescaped, `+` repeats the previous
    /// character and the pattern matches nothing.
    #[test]
    fn regular_expression_metacharacters_are_escaped() {
        assert_eq!(
            search_pattern("VSCodium - c++").as_deref(),
            Some("^VSCodium \\- c\\+\\+$")
        );
        assert_eq!(
            search_pattern("a.b(c)[d]{e}|f*g?h^i$j/k").as_deref(),
            Some("^a\\.b\\(c\\)\\[d\\]\\{e\\}\\|f\\*g\\?h\\^i\\$j\\/k$")
        );
    }

    /// Inside `String.raw` a backslash is literal, so the pattern asks the
    /// engine for a literal backslash and nothing else.
    #[test]
    fn a_backslash_is_escaped_too() {
        assert_eq!(search_pattern("a\\b").as_deref(), Some("^a\\\\b$"));
    }

    /// The two sequences that end or escape a `String.raw` template literal.
    /// Neither can be escaped inside one, so a text carrying either is
    /// refused.
    #[test]
    fn a_title_that_could_break_out_of_the_template_is_refused() {
        assert_eq!(search_pattern("VSCodium - `whoami`"), None);
        assert_eq!(search_pattern("VSCodium - ${x}"), None);
        assert_eq!(search_pattern(""), None);
    }

    /// `kdotool search` prints one id per line, with the tool's braces, as a
    /// search for the class `codium` did on 2026-09-27.
    #[test]
    fn a_search_gives_one_id_per_line() {
        let out =
            "{aaaaaaaa-0000-0000-0000-000000000001}\n{aaaaaaaa-0000-0000-0000-000000000002}\n";
        assert_eq!(
            parse_windows(out),
            vec![
                "aaaaaaaa-0000-0000-0000-000000000001".to_string(),
                "aaaaaaaa-0000-0000-0000-000000000002".to_string(),
            ]
        );
    }

    #[test]
    fn a_search_that_found_nothing_gives_no_ids() {
        assert!(parse_windows("").is_empty());
        assert!(parse_windows("\n\n").is_empty());
    }
}

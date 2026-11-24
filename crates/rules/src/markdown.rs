//! Markdown headings, sections and fenced blocks, as the Python tools find them.
//!
//! Two fence dialects live here, because the tools differ and their boundaries differ on indented
//! and info-string fences:
//! - [`headings`], [`section`] and [`issue_body`] follow `.mintswarm/dogfood/plan-approval/floor.py`:
//!   a fence opens on a run of three or more backticks or tildes after any spaces and tabs;
//! - [`scan_fences`] follows `.mintswarm/dogfood/landing/collect-facts.py`: a fence opens after at
//!   most three spaces, and a backtick fence's info string holds no backtick, as CommonMark has it.
//!
//! Whitespace is Python's `str.isspace` set throughout, which is Unicode White_Space plus the
//! separators U+001C to U+001F.

/// Whether `c` is whitespace to Python's `str.isspace`.
pub fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `s` without leading and trailing whitespace, as Python's `str.strip()` gives.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// `s` without trailing whitespace, as Python's `str.rstrip()` gives.
pub fn py_rstrip(s: &str) -> &str {
    s.trim_end_matches(is_py_space)
}

/// The lines of a document split on `\n`, each without one trailing `\r`, as `floor.py`'s
/// `document_lines` gives. A final newline leaves an empty last line.
pub fn document_lines(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

/// The leading run of `c` in `s`, in bytes, when it is at least three long.
fn run_of(s: &str, c: char) -> Option<usize> {
    let len = s.len() - s.trim_start_matches(c).len();
    (len >= 3).then_some(len)
}

/// The fence run `s` starts with, as its character and length: three or more backticks or tildes.
fn fence_run(s: &str) -> Option<(char, usize)> {
    ['`', '~']
        .into_iter()
        .find_map(|c| run_of(s, c).map(|len| (c, len)))
}

/// The index and level of each `# ` (1) and `## ` (2) heading outside fenced blocks, as
/// `floor.py`'s `headings` finds them.
///
/// A line opens a fence when, after any leading spaces and tabs, it starts with three or more
/// backticks or tildes. The fence closes on a line that, after the same stripping, starts with an
/// equal or longer run of the same character followed only by whitespace.
pub fn headings(lines: &[&str]) -> Vec<(usize, u8)> {
    let mut found = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for (index, line) in lines.iter().enumerate() {
        let stripped = line.trim_start_matches([' ', '\t']);
        let run = fence_run(stripped);
        if let Some((open, open_len)) = fence {
            if let Some((c, len)) = run {
                if c == open && len >= open_len && py_strip(&stripped[len..]).is_empty() {
                    fence = None;
                }
            }
            continue;
        }
        if run.is_some() {
            fence = run;
        } else if line.starts_with("## ") {
            found.push((index, 2));
        } else if line.starts_with("# ") {
            found.push((index, 1));
        }
    }
    found
}

/// The one `## <title>` section of a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section<'a> {
    /// The index of the section's first body line, the line after its heading.
    pub start: usize,
    /// The body lines, up to the next heading of a level that ends it.
    pub body: &'a [&'a str],
}

/// Why a document has no single `## <title>` section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionError {
    /// No heading names the section.
    Missing,
    /// This many headings name it.
    Repeated(usize),
}

impl SectionError {
    /// The reason `floor.py`'s `section` gives for the document `what`.
    pub fn message(&self, what: &str, title: &str) -> String {
        match self {
            SectionError::Missing => format!("{what}: no '## {title}' section"),
            SectionError::Repeated(count) => format!("{what}: {count} '## {title}' sections"),
        }
    }
}

/// The one `## <title>` section of `lines`, as `floor.py`'s `section` finds it.
///
/// A heading names the section when it is a level-2 heading that equals `## <title>` once its
/// trailing whitespace is stripped. The section runs to the next heading whose level is in
/// `ends` (the Python default is `[1, 2]`), or to the end.
pub fn section<'a>(
    lines: &'a [&'a str],
    title: &str,
    ends: &[u8],
) -> Result<Section<'a>, SectionError> {
    let marks = headings(lines);
    let heading = format!("## {title}");
    let starts: Vec<usize> = marks
        .iter()
        .filter(|(i, level)| *level == 2 && py_rstrip(lines[*i]) == heading)
        .map(|(i, _)| *i)
        .collect();
    let start = match starts.as_slice() {
        [] => return Err(SectionError::Missing),
        [start] => *start,
        _ => return Err(SectionError::Repeated(starts.len())),
    };
    let end = marks
        .iter()
        .find(|(i, level)| *i > start && ends.contains(level))
        .map_or(lines.len(), |(i, _)| *i);
    Ok(Section {
        start: start + 1,
        body: &lines[start + 1..end],
    })
}

/// The issue document's lines before its first `## Comments` heading, as `floor.py`'s
/// `issue_body` gives; `lines` is the document's [`document_lines`].
pub fn issue_body<'a>(lines: &'a [&'a str]) -> &'a [&'a str] {
    headings(lines)
        .into_iter()
        .find(|(i, level)| *level == 2 && py_rstrip(lines[*i]) == "## Comments")
        .map_or(lines, |(i, _)| &lines[..i])
}

/// A fenced block [`scan_fences`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FencedBlock<'a> {
    /// The first word of the info string, or empty.
    pub info: &'a str,
    /// The lines between the opening and closing fence.
    pub content: Vec<&'a str>,
}

/// The lines of a document split by [`scan_fences`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fences<'a> {
    /// The index and text of each line outside every fenced block.
    pub outside: Vec<(usize, &'a str)>,
    /// The closed fenced blocks, in order.
    pub blocks: Vec<FencedBlock<'a>>,
}

/// The opening fence on `line`, by `collect-facts.py`'s `FENCE_OPEN`: at most three spaces, a run
/// of three or more backticks or tildes, spaces and tabs, then an info word of characters that are
/// neither backticks nor whitespace. A backtick fence opens only when no backtick follows its run.
fn fence_open(line: &str) -> Option<(char, usize, &str)> {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return None;
    }
    let (c, len) = fence_run(rest)?;
    let after = &rest[len..];
    if c == '`' && after.contains('`') {
        return None;
    }
    let info = after.trim_start_matches([' ', '\t']);
    let end = info
        .find(|ch: char| ch == '`' || is_py_space(ch))
        .unwrap_or(info.len());
    Some((c, len, &info[..end]))
}

/// Whether `line` closes a fence of `len` characters `c`: at most three spaces, a run of `c` at
/// least `len` long, then only spaces and tabs.
fn fence_closes(line: &str, c: char, len: usize) -> bool {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return false;
    }
    let after = rest.trim_start_matches(c);
    rest.len() - after.len() >= len && after.trim_start_matches([' ', '\t']).is_empty()
}

/// The lines outside fenced blocks and the closed blocks, as `collect-facts.py`'s `scan_fences`
/// gives. A block still open at the end is dropped: its lines are neither outside nor in a block.
pub fn scan_fences<'a>(lines: &[&'a str]) -> Fences<'a> {
    let mut outside = Vec::new();
    let mut blocks = Vec::new();
    let mut open: Option<(char, usize, &'a str, Vec<&'a str>)> = None;
    for (index, line) in lines.iter().copied().enumerate() {
        match open.take() {
            None => match fence_open(line) {
                Some((c, len, info)) => open = Some((c, len, info, Vec::new())),
                None => outside.push((index, line)),
            },
            Some((c, len, info, content)) if fence_closes(line, c, len) => {
                blocks.push(FencedBlock { info, content });
            }
            Some((c, len, info, mut content)) => {
                content.push(line);
                open = Some((c, len, info, content));
            }
        }
    }
    Fences { outside, blocks }
}

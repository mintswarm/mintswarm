//! Pure rules shared by the runner and the plane.
//!
//! - [`field`] and [`size`]: how a value is written to a tracker field, how a read is
//!   interpreted, what counts as drift, and how a size is resolved.
//! - The building blocks the ported Python tools rest on, each matching its Python helper:
//!   - [`glob`]: path globs with the plan floor's semantics;
//!   - [`json`]: the strict JSON reader and a writer that reproduces `json.dumps` byte for byte;
//!   - [`markdown`]: headings, sections and fenced blocks, in both of the tools' fence dialects;
//!   - [`py`]: Python's string semantics, `repr()`, `str.lower()` and reasons cut to one line;
//!   - [`binding`]: sha256 digests and the bindings of files to them;
//!   - [`exit`]: the 0, 1 and 2 exit-code contract.
//! - [`output`]: a stage's output document and `msw-result` block, captured from its final reply
//!   or a Claude Code transcript as `save-output.py` captures them, and the stage-aware checks
//!   on the outcome.
//! - [`plan_floor`]: the plan floor, `plan-approval/floor.py`'s rules over a sha256-bound input
//!   bundle.
//! - [`plan_decision`]: the plan-approval decision, `plan-approval/decision.py`'s two layers and
//!   mode over the same bundle, with the `decision.json` record.
//! - [`pr_labels`]: the pull request labelling rule, the component and band labels a factory PR
//!   should carry.
//! - [`render`]: the plan-approval comment bodies, the Commands section, the decision records,
//!   answers, set-backs, merge records and the shadow decision line, and the confinement of model,
//!   session and user text in them.
//! - [`command`]: the approve, reject and help commands a comment gives, the hints for near
//!   misses, and the digest an approval package binds.
//!
//! Nothing here does I/O or records anything; callers supply what was read and act on the
//! result.

pub mod binding;
pub mod command;
pub mod exit;
pub mod field;
pub mod glob;
pub mod json;
pub mod markdown;
pub mod output;
pub mod plan_decision;
pub mod plan_floor;
pub mod pr_labels;
pub mod py;
pub mod render;
pub mod size;

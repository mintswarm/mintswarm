//! The exit-code contract every rules tool keeps: 0 for a clean verdict, 1 for a verdict with a
//! hit, 2 for input it cannot use.

use std::fmt;

/// A verdict a tool reached on input it could use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Verdict {
    /// No rule is hit; exit code 0.
    Clean,
    /// At least one rule is hit; exit code 1.
    Hit,
}

/// Input a tool cannot use, with the message it reports; exit code 2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unusable {
    message: String,
}

impl Unusable {
    /// Input that cannot be used, for the reason `message` gives.
    pub fn new(message: impl Into<String>) -> Self {
        Unusable {
            message: message.into(),
        }
    }

    /// The message the tool reports.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Unusable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Unusable {}

/// What a tool run comes to: a verdict, or input it cannot use.
pub type Outcome = Result<Verdict, Unusable>;

/// The process exit code for `outcome`: 0 for [`Verdict::Clean`], 1 for [`Verdict::Hit`] and 2
/// for [`Unusable`].
pub fn code(outcome: &Outcome) -> u8 {
    match outcome {
        Ok(Verdict::Clean) => 0,
        Ok(Verdict::Hit) => 1,
        Err(_) => 2,
    }
}

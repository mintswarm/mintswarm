//! Identifiers shared by jobs, records and the runner's traits.
//!
//! Each is a newtype that serialises as its inner value.

use serde::{Deserialize, Serialize};

macro_rules! id {
    ($(#[$doc:meta])* $name:ident($inner:ty)) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub $inner);
    };
}

id!(
    /// A job: one issue's way through the pipeline.
    JobId(String)
);
id!(
    /// One attempt at a stage or step of a job.
    AttemptId(String)
);
id!(
    /// A worker slot of a runner.
    WorkerId(String)
);
id!(
    /// A project the plane serves: one repository with its config.
    ProjectId(String)
);
id!(
    /// A runner daemon.
    RunnerId(String)
);
id!(
    /// The position of an event or record in a runner's outbox.
    Seq(u64)
);
id!(
    /// A full hexadecimal commit id, never revision syntax.
    RevId(String)
);
id!(
    /// An issue number on the tracker.
    IssueNumber(u64)
);
id!(
    /// A pull request number on the code host.
    PrNumber(u64)
);
id!(
    /// A harness account, by the nickname the runner config gives it.
    AccountNickname(String)
);
id!(
    /// The fence of an attempt's lease: the plane raises it on every reassignment, so writes
    /// from an older holder are refused.
    LeaseEpoch(u64)
);
id!(
    /// A directive, so the runner can acknowledge it with a receipt.
    DirectiveId(String)
);

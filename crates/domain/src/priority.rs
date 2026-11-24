//! The priority scale, a job's own level and the stride selector that orders phase-2 entries,
//! design §4.1, §4.2, §6.2 and §8.1.
//!
//! [`Priority`] is a canonical [`Scale`], so a tracker field maps onto it through a
//! [`FieldMap<Priority>`]. [`own_level`] turns one read of that field into a job's own level and
//! the source it came from. [`ShareState`] holds one pass per level and orders the candidates of
//! a claim by stride scheduling, so that the levels share the entries by their configured shares
//! and no level waits indefinitely.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::field::{FieldMap, FieldRead, OptionSpec};
use crate::ids::JobId;
use crate::job::named_enum;
use crate::scale::{Scale, Size};

named_enum! {
    /// How urgent a piece of work is, in ascending order (design §4.2).
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Priority {
        /// Work that can wait.
        Low => "low",
        /// Ordinary work; the default level.
        Medium => "medium",
        /// Work that goes before ordinary work.
        High => "high",
        /// Work that goes first.
        Urgent => "urgent",
    }
}

impl Scale for Priority {
    const ALL: &'static [Self] = &[Self::Low, Self::Medium, Self::High, Self::Urgent];

    fn name(self) -> &'static str {
        self.as_str()
    }
}

impl Priority {
    /// The level's position in [`Priority::ALL`], which indexes per-level arrays.
    fn index(self) -> usize {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
            Self::Urgent => 3,
        }
    }
}

named_enum! {
    /// Where a job's priority level came from (design §4.2).
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum PrioritySource {
        /// A mapped option on the job's own issue.
        Tracker => "tracker",
        /// The issue has no value and no parent level applies: the role's default.
        Unset => "unset",
        /// The issue has no value; the level is inherited from the parent.
        Parent => "parent",
        /// The issue holds a drifted, unmapped or conflicting value: the role's default.
        Default => "default",
        /// The field could not be read. Set by callers; [`own_level`] never returns it.
        Unreadable => "unreadable",
        /// The priority role is off. Set by callers; [`own_level`] never returns it.
        Off => "off",
    }
}

/// A job's own level from one read of its issue's priority field (design §4.1, §4.2 and §5.2).
///
/// - An unset value inherits `parent` when there is one, else it takes the map's default.
/// - A value naming a mapped option reads as that option's `reads_as`: its range's upper (more
///   urgent) bound unless the option states otherwise.
/// - A value naming no mapped option, or a conflict between labels, takes the map's default; a
///   value that is set never inherits from the parent.
pub fn own_level(
    map: &FieldMap<Priority>,
    observed: &FieldRead,
    parent: Option<Priority>,
) -> (Priority, PrioritySource) {
    match observed {
        FieldRead::Unset => match parent {
            Some(level) => (level, PrioritySource::Parent),
            None => (map.default(), PrioritySource::Unset),
        },
        FieldRead::Value(value) => match map.option(&value.name) {
            Some(option) => (option.reads_as, PrioritySource::Tracker),
            None => (map.default(), PrioritySource::Default),
        },
        FieldRead::Conflict(_) => (map.default(), PrioritySource::Default),
    }
}

/// The option entries setup proposes for a tracker's priority options (design §4.1, §4.2 and
/// §5.2).
///
/// An option whose name equals a level's name, ignoring ASCII case, maps to that level and is
/// never written. Any other option gets an empty range, which the operator fills in before the
/// map validates. The input order is kept.
pub fn setup_preset<I, N>(option_names: I) -> Vec<OptionSpec>
where
    I: IntoIterator<Item = N>,
    N: AsRef<str>,
{
    option_names
        .into_iter()
        .map(|name| {
            let name = name.as_ref();
            let range = Priority::ALL
                .iter()
                .find(|level| level.name().eq_ignore_ascii_case(name))
                .map_or("", |level| level.name());
            OptionSpec {
                name: name.to_owned(),
                range: range.to_owned(),
                write: false,
                reads_as: None,
            }
        })
        .collect()
}

/// The numerator of every stride: divisible by each share from 1 to 16, so strides are exact.
pub const STRIDE_SCALE: u64 = 720_720;

/// The smallest share a level may have.
pub const MIN_SHARE: u32 = 1;

/// The largest share a level may have.
pub const MAX_SHARE: u32 = 16;

/// One job that is ready to enter phase 2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryCandidate {
    /// The job.
    pub job: JobId,
    /// The job's effective level.
    pub level: Priority,
    /// When the job became ready, in milliseconds since the Unix epoch.
    pub ready_at: u64,
    /// The job's size.
    pub size: Size,
}

/// One level's place in the stride schedule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ClassState {
    /// The level's virtual time: it advances by the level's stride per unit of cost charged.
    pub pass: u64,
    /// Whether the level had candidates at the last observation or charge.
    pub active: bool,
}

/// Why a [`ShareState`] cannot be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareError {
    /// A level's share lies outside [`MIN_SHARE`] to [`MAX_SHARE`].
    OutOfRange {
        /// The level.
        level: Priority,
        /// Its share.
        share: u32,
    },
}

impl fmt::Display for ShareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRange { level, share } => write!(
                f,
                "the share {share} of {level} lies outside {MIN_SHARE} to {MAX_SHARE}"
            ),
        }
    }
}

impl std::error::Error for ShareError {}

/// The stride schedule of one project's phase-2 entries (design §6.2 and §8.1).
///
/// Arrays are indexed in [`Priority::ALL`] order: low, medium, high, urgent. A claim follows
/// this order: [`observe`](Self::observe) with its candidates, then [`ranked`](Self::ranked),
/// then [`charge`](Self::charge) for the entry it admits, and for that entry only. A cost of 0
/// counts as 1.
///
/// A level that becomes active again has its pass raised to the smallest pass among the levels
/// that stay active, so it banks no credit while idle. When no level stays active, the largest
/// pass stands in for the last one charged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareState {
    shares: [u32; 4],
    classes: [ClassState; 4],
}

impl Default for ShareState {
    /// Shares 8:4:2:1 from urgent down to low, every pass 0 and every level inactive.
    fn default() -> Self {
        Self {
            shares: [1, 2, 4, 8],
            classes: [ClassState::default(); 4],
        }
    }
}

impl ShareState {
    /// A fresh schedule with these shares, in low, medium, high, urgent order.
    pub fn new(shares: [u32; 4]) -> Result<Self, ShareError> {
        Self::from_parts(shares, [ClassState::default(); 4])
    }

    /// A schedule restored from persisted shares and level states, both in low, medium, high,
    /// urgent order.
    pub fn from_parts(shares: [u32; 4], classes: [ClassState; 4]) -> Result<Self, ShareError> {
        for (&level, &share) in Priority::ALL.iter().zip(&shares) {
            if !(MIN_SHARE..=MAX_SHARE).contains(&share) {
                return Err(ShareError::OutOfRange { level, share });
            }
        }
        Ok(Self { shares, classes })
    }

    /// The level's share.
    pub fn share(&self, level: Priority) -> u32 {
        self.shares[level.index()]
    }

    /// The level's stride: how far its pass advances per unit of cost.
    pub fn stride(&self, level: Priority) -> u64 {
        STRIDE_SCALE / u64::from(self.share(level))
    }

    /// The level's place in the schedule.
    pub fn class(&self, level: Priority) -> ClassState {
        self.classes[level.index()]
    }

    /// Record which levels have candidates now: a level that becomes active has its pass raised
    /// to the schedule's virtual time, and a level without candidates becomes inactive.
    pub fn observe(&mut self, candidates: &[EntryCandidate]) {
        let mut backlogged = [false; 4];
        for candidate in candidates {
            backlogged[candidate.level.index()] = true;
        }
        self.activate(backlogged);
    }

    /// Every candidate's job, in the order the schedule would admit them. Within a level,
    /// candidates go by `ready_at`, then by job id; between levels, the smallest pass goes
    /// first, and a tie goes to the higher level. The state is not changed.
    pub fn ranked(
        &self,
        candidates: &[EntryCandidate],
        cost: impl Fn(&EntryCandidate) -> u32,
    ) -> Vec<JobId> {
        let mut state = self.clone();
        state.observe(candidates);
        let mut queues: [Vec<&EntryCandidate>; 4] = Default::default();
        for candidate in candidates {
            queues[candidate.level.index()].push(candidate);
        }
        for queue in &mut queues {
            queue.sort_by(|a, b| (a.ready_at, &a.job).cmp(&(b.ready_at, &b.job)));
        }
        let mut next = [0_usize; 4];
        let mut order = Vec::with_capacity(candidates.len());
        loop {
            let pick = Priority::ALL
                .iter()
                .rev()
                .copied()
                .filter(|level| next[level.index()] < queues[level.index()].len())
                .min_by_key(|level| state.class(*level).pass);
            let Some(level) = pick else {
                break;
            };
            let i = level.index();
            let candidate = queues[i][next[i]];
            next[i] += 1;
            order.push(candidate.job.clone());
            state.advance(level, cost(candidate));
        }
        order
    }

    /// Charge an admitted entry of `level` at `cost`. An inactive level first has its pass raised
    /// to the schedule's virtual time and becomes active; the other levels keep their flags.
    pub fn charge(&mut self, level: Priority, cost: u32) {
        let i = level.index();
        if !self.classes[i].active {
            let vt = self.virtual_time([true; 4]);
            let class = &mut self.classes[i];
            class.pass = class.pass.max(vt);
            class.active = true;
        }
        self.advance(level, cost);
    }

    fn advance(&mut self, level: Priority, cost: u32) {
        let step = self.stride(level).saturating_mul(u64::from(cost.max(1)));
        let class = &mut self.classes[level.index()];
        class.pass = class.pass.saturating_add(step);
    }

    /// The smallest pass among the levels that are active and kept; when there is none, the
    /// largest pass over all levels.
    fn virtual_time(&self, kept: [bool; 4]) -> u64 {
        self.classes
            .iter()
            .zip(kept)
            .filter(|(class, kept)| class.active && *kept)
            .map(|(class, _)| class.pass)
            .min()
            .unwrap_or_else(|| {
                self.classes
                    .iter()
                    .map(|class| class.pass)
                    .max()
                    .unwrap_or(0)
            })
    }

    fn activate(&mut self, backlogged: [bool; 4]) {
        let vt = self.virtual_time(backlogged);
        for (class, backlogged) in self.classes.iter_mut().zip(backlogged) {
            if backlogged && !class.active {
                class.pass = class.pass.max(vt);
            }
            class.active = backlogged;
        }
    }
}

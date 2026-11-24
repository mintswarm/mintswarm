//! Learned costs of sizes, design §8.3.
//!
//! #300 learns them from recorded actuals; until then the plane runs with [`NoCostModel`].

use serde::{Deserialize, Serialize};

use crate::ids::AccountName;
use crate::scale::Size;

/// The learned phase 2 cost of a size, in percent of the account's 5 h window.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LearnedCost {
    /// The expected phase 2 consumption.
    pub total: f64,
    /// The expected phase 2 consumption inside one 5 h window.
    pub per_window: f64,
}

/// The learned costs of sizes, per account.
pub trait CostModel: Send + Sync {
    /// The learned cost of `size` on `account`, or `None` while there are too few actuals.
    fn cost(&self, account: &AccountName, size: Size) -> Option<LearnedCost>;
}

/// A cost model that has learned nothing: every cost is `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoCostModel;

impl CostModel for NoCostModel {
    fn cost(&self, _account: &AccountName, _size: Size) -> Option<LearnedCost> {
        None
    }
}

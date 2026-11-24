//! Canonical ordinal scales and the size scale, design §4.1 and §5.
//!
//! A scale is a fixed, ordered list of values. A tracker field maps onto one through a
//! [`FieldMap`](crate::field::FieldMap); nothing here is specific to sizes.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A canonical ordinal scale: a fixed list of values in ascending order.
pub trait Scale: Copy + Ord + 'static {
    /// Every value of the scale, in ascending order.
    const ALL: &'static [Self];

    /// The value's canonical name, as the config and serde spell it.
    ///
    /// A canonical name never contains `-`: [`ScaleRange::parse`] splits `lo-hi` on the
    /// first `-`, so a name with one could not be used in a range.
    fn name(self) -> &'static str;

    /// The value with this canonical name, if any.
    fn parse(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|value| value.name() == name)
    }
}

/// The canonical size of a piece of work, design §5.
///
/// It is an ordinal judgement and states no wall-clock time. It is called "size" because
/// `StageJob.effort` already means reasoning effort.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Size {
    /// `XS`.
    #[serde(rename = "XS")]
    Xs,
    /// `S`.
    #[serde(rename = "S")]
    S,
    /// `M`.
    #[serde(rename = "M")]
    M,
    /// `L`.
    #[serde(rename = "L")]
    L,
    /// `XL`.
    #[serde(rename = "XL")]
    Xl,
    /// `2XL`.
    #[serde(rename = "2XL")]
    Xxl,
    /// `3XL`.
    #[serde(rename = "3XL")]
    Xxxl,
    /// `4XL`.
    #[serde(rename = "4XL")]
    Xxxxl,
}

impl Scale for Size {
    const ALL: &'static [Self] = &[
        Self::Xs,
        Self::S,
        Self::M,
        Self::L,
        Self::Xl,
        Self::Xxl,
        Self::Xxxl,
        Self::Xxxxl,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Xs => "XS",
            Self::S => "S",
            Self::M => "M",
            Self::L => "L",
            Self::Xl => "XL",
            Self::Xxl => "2XL",
            Self::Xxxl => "3XL",
            Self::Xxxxl => "4XL",
        }
    }
}

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A contiguous span `lo-hi` of a scale, bounds included; one value when `lo == hi`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScaleRange<S: Scale> {
    lo: S,
    hi: S,
}

/// Why a range text does not parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScaleRangeError {
    /// The text, or one of its bounds, is empty.
    Empty,
    /// A bound is not a value of the scale.
    Unknown {
        /// The unknown bound.
        name: String,
    },
    /// The lower bound lies above the upper one.
    Reversed,
}

impl fmt::Display for ScaleRangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("the range is empty"),
            Self::Unknown { name } => write!(f, "{name:?} is not a value of the scale"),
            Self::Reversed => f.write_str("the lower bound lies above the upper one"),
        }
    }
}

impl std::error::Error for ScaleRangeError {}

impl<S: Scale> ScaleRange<S> {
    /// The span from `lo` to `hi`, or `None` if `lo` lies above `hi`.
    pub fn new(lo: S, hi: S) -> Option<Self> {
        (lo <= hi).then_some(Self { lo, hi })
    }

    /// The span holding only `value`.
    pub fn single(value: S) -> Self {
        Self {
            lo: value,
            hi: value,
        }
    }

    /// The lower bound.
    pub fn lo(self) -> S {
        self.lo
    }

    /// The upper bound.
    pub fn hi(self) -> S {
        self.hi
    }

    /// Whether `value` lies inside the span.
    pub fn contains(self, value: S) -> bool {
        self.lo <= value && value <= self.hi
    }

    /// Parse `lo-hi`, or one value, by the scale's canonical names.
    pub fn parse(text: &str) -> Result<Self, ScaleRangeError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(ScaleRangeError::Empty);
        }
        let bound = |name: &str| {
            let name = name.trim();
            if name.is_empty() {
                return Err(ScaleRangeError::Empty);
            }
            S::parse(name).ok_or_else(|| ScaleRangeError::Unknown {
                name: name.to_owned(),
            })
        };
        match text.split_once('-') {
            None => bound(text).map(Self::single),
            Some((lo, hi)) => Self::new(bound(lo)?, bound(hi)?).ok_or(ScaleRangeError::Reversed),
        }
    }
}

impl<S: Scale> fmt::Display for ScaleRange<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.lo == self.hi {
            f.write_str(self.lo.name())
        } else {
            write!(f, "{}-{}", self.lo.name(), self.hi.name())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Scale, ScaleRange, ScaleRangeError, Size};

    #[test]
    fn range_parses_spans_and_single_values() {
        let span = ScaleRange::<Size>::parse("XS-S").unwrap();
        assert_eq!((span.lo(), span.hi()), (Size::Xs, Size::S));
        assert!(span.contains(Size::Xs) && span.contains(Size::S) && !span.contains(Size::M));
        assert_eq!(span.to_string(), "XS-S");
        let one = ScaleRange::<Size>::parse("2XL").unwrap();
        assert_eq!(one, ScaleRange::single(Size::Xxl));
        assert_eq!(one.to_string(), "2XL");
    }

    #[test]
    fn size_names_contain_no_range_separator() {
        assert!(Size::ALL.iter().all(|size| !size.name().contains('-')));
    }

    #[test]
    fn range_rejects_empty_unknown_and_reversed_text() {
        assert_eq!(ScaleRange::<Size>::parse(""), Err(ScaleRangeError::Empty));
        assert_eq!(
            ScaleRange::<Size>::parse("XS-"),
            Err(ScaleRangeError::Empty)
        );
        assert_eq!(
            ScaleRange::<Size>::parse("XXL"),
            Err(ScaleRangeError::Unknown { name: "XXL".into() })
        );
        assert_eq!(
            ScaleRange::<Size>::parse("L-S"),
            Err(ScaleRangeError::Reversed)
        );
    }

    #[test]
    fn parse_finds_no_unknown_name() {
        assert_eq!(Size::parse("xs"), None);
    }
}

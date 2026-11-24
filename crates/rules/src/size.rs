//! The lines floor and the effective size of an issue, design §5, §6.3 and §7.2.

use std::fmt;

use mintswarm_domain::field::{FieldFinding, FieldOption};
use mintswarm_domain::scale::Size;

use crate::field::ReadValue;

/// The lines floor bands: the largest production line count each size holds, in ascending order.
///
/// Anything above the last band is 4XL. This table must agree with `BANDS` in
/// `.mintswarm/dogfood/plan-approval/size.py`.
pub const BANDS: [(u64, Size); 7] = [
    (25, Size::Xs),
    (75, Size::S),
    (200, Size::M),
    (500, Size::L),
    (1200, Size::Xl),
    (3000, Size::Xxl),
    (7000, Size::Xxxl),
];

/// The smallest size whose band holds `production_lines`, design §6.3.
pub fn floor(production_lines: u64) -> Size {
    BANDS
        .iter()
        .find(|(bound, _)| production_lines <= *bound)
        .map_or(Size::Xxxxl, |(_, size)| *size)
}

/// The stage that judged a size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JudgingStage {
    /// The analyse stage.
    Analyse,
    /// The plan stage.
    Plan,
}

/// A size a stage judged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Judged {
    /// The stage that judged it.
    pub stage: JudgingStage,
    /// The size it judged.
    pub size: Size,
}

/// Where an effective size came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SizeSource {
    /// The analyse stage's size.
    Analyse,
    /// The plan stage's size, raised to the lines floor when that is larger.
    Plan,
    /// A person's choice on the tracker.
    Person,
    /// The map's default.
    Default,
    /// The map's default, because what was read could not be used.
    Unreadable,
}

impl SizeSource {
    /// The source's name, as it is recorded.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Analyse => "analyse",
            Self::Plan => "plan",
            Self::Person => "person",
            Self::Default => "default",
            Self::Unreadable => "unreadable",
        }
    }
}

impl fmt::Display for SizeSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who a read value is from, design §7.2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reading<'m> {
    /// The issue holds no value.
    Unset,
    /// The issue holds the option mintswarm last wrote.
    Echo(&'m FieldOption<Size>),
    /// The issue holds an option a person chose.
    Person(&'m FieldOption<Size>),
    /// The issue holds something the map cannot interpret.
    Unreadable(FieldFinding),
}

/// Classify a read by the option mintswarm last wrote, design §7.2.
///
/// A mapped option whose name equals `last_written` is an echo. Any other mapped option is a
/// person's, including every option when mintswarm has written nothing yet.
pub fn classify<'m>(read: &ReadValue<'m, Size>, last_written: Option<&str>) -> Reading<'m> {
    match *read {
        ReadValue::Unset => Reading::Unset,
        ReadValue::Unreadable(finding) => Reading::Unreadable(finding),
        ReadValue::Mapped(option) if Some(option.name.as_str()) == last_written => {
            Reading::Echo(option)
        }
        ReadValue::Mapped(option) => Reading::Person(option),
    }
}

/// The fallback state of the scope that holds the issue's value, design §4.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fallback {
    /// Whether the scope is in fallback.
    pub active: bool,
    /// The fallback value, from [`fallback`](crate::field::fallback).
    pub value: Size,
}

/// An issue's effective size, with the judged and floor values it was resolved from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectiveSize {
    /// The size the issue is treated as.
    pub size: Size,
    /// Where it came from.
    pub source: SizeSource,
    /// The size a stage judged, if any.
    pub judged: Option<Size>,
    /// The lines floor, if any.
    pub floor: Option<Size>,
}

/// Resolve an issue's effective size, design §4.5, §6.3 and §7.2.
///
/// The stage size is `max(judged, floor)`: the floor only raises. Only plans have production
/// lines, so callers pass `floor: None` for an analyse estimate. The precedence is:
/// 1. if the scope is in fallback, or the read is unreadable, the fallback value, with source
///    `unreadable` when a person's value or an unreadable value was read, otherwise `default`;
/// 2. otherwise a person's option: the stage size if its range holds it, otherwise the option's
///    `reads_as`, with source `person`;
/// 3. otherwise (nothing read, or an echo) the stage size with source `analyse` or `plan`, or
///    the fallback value with source `default` when no stage has judged a size.
///
/// `judged` and `floor` are always returned as given.
pub fn effective_size(
    judged: Option<Judged>,
    floor: Option<Size>,
    person_option: &ReadValue<'_, Size>,
    last_written: Option<&str>,
    fallback: Fallback,
) -> EffectiveSize {
    let stage_size = judged.map(|judged| floor.map_or(judged.size, |floor| judged.size.max(floor)));
    let reading = classify(person_option, last_written);
    let (size, source) = match reading {
        Reading::Unreadable(_) => (fallback.value, SizeSource::Unreadable),
        Reading::Person(_) if fallback.active => (fallback.value, SizeSource::Unreadable),
        _ if fallback.active => (fallback.value, SizeSource::Default),
        Reading::Person(option) => match stage_size {
            Some(size) if option.range.contains(size) => (size, SizeSource::Person),
            _ => (option.reads_as, SizeSource::Person),
        },
        Reading::Unset | Reading::Echo(_) => match (judged, stage_size) {
            (Some(judged), Some(size)) => (
                size,
                match judged.stage {
                    JudgingStage::Analyse => SizeSource::Analyse,
                    JudgingStage::Plan => SizeSource::Plan,
                },
            ),
            _ => (fallback.value, SizeSource::Default),
        },
    };
    EffectiveSize {
        size,
        source,
        judged: judged.map(|judged| judged.size),
        floor,
    }
}

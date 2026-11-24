//! The size scale, field maps, the tracker's field operations and the cost model.

use std::future::{Future, ready};
use std::mem::discriminant;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use mintswarm_domain::cost::{CostModel, NoCostModel};
use mintswarm_domain::field::{
    FieldError, FieldFinding, FieldMap, FieldMapError, FieldMapSpec, FieldRead, FieldRef,
    FieldValue, LabelPattern, OptionSpec, SourceSpec, read_labels,
};
use mintswarm_domain::ids::{AccountName, IssueNumber, PrNumber};
use mintswarm_domain::scale::{Scale, Size};
use mintswarm_domain::tracker::{
    Artifact, AssignmentReason, Comment, Cursor, Issue, IssueRead, IssueType, Link, Login, SubItem,
    Tracker,
};

fn option(name: &str, range: &str, write: bool, reads_as: Option<&str>) -> OptionSpec {
    OptionSpec {
        name: name.into(),
        range: range.into(),
        write,
        reads_as: reads_as.map(str::to_owned),
    }
}

fn field_spec(default: &str, options: Vec<OptionSpec>) -> FieldMapSpec {
    FieldMapSpec {
        source: SourceSpec::Field {
            name: "Effort".into(),
        },
        default: default.into(),
        options,
    }
}

/// One written option per size.
fn partition() -> Vec<OptionSpec> {
    Size::ALL
        .iter()
        .map(|size| option(size.name(), size.name(), true, None))
        .collect()
}

#[test]
fn size_all_ascending_and_names_round_trip() {
    use Size::*;
    assert_eq!(Size::ALL, [Xs, S, M, L, Xl, Xxl, Xxxl, Xxxxl]);
    assert!(Size::ALL.windows(2).all(|pair| pair[0] < pair[1]));
    let names: Vec<_> = Size::ALL.iter().map(|size| size.name()).collect();
    assert_eq!(names, ["XS", "S", "M", "L", "XL", "2XL", "3XL", "4XL"]);
    for &size in Size::ALL {
        assert_eq!(Size::parse(size.name()), Some(size));
        let json = serde_json::to_string(&size).unwrap();
        assert_eq!(json, format!("\"{}\"", size.name()));
        assert_eq!(serde_json::from_str::<Size>(&json).unwrap(), size);
    }
}

#[test]
fn field_map_rejects_each_invariant() {
    let mut overlap = partition();
    overlap[1] = option("S", "XS-S", true, None);
    let mut uncovered = partition();
    uncovered.remove(2);
    let mut outside = partition();
    outside[0] = option("XS", "XS", true, Some("S"));

    let cases = [
        (field_spec("M", overlap), "overlap"),
        (field_spec("M", uncovered), "uncovered"),
        (field_spec("M", outside), "outside"),
        (field_spec("medium", partition()), "default"),
    ];
    let errors: Vec<FieldMapError> = cases
        .into_iter()
        .map(|(spec, case)| match FieldMap::<Size>::new(spec) {
            Ok(_) => panic!("{case}: the spec is accepted"),
            Err(err) => err,
        })
        .collect();

    assert_eq!(
        errors[0],
        FieldMapError::Overlap {
            value: "XS".into(),
            first: "XS".into(),
            second: "S".into(),
        }
    );
    assert_eq!(errors[1], FieldMapError::Uncovered { value: "M".into() });
    assert!(matches!(
        &errors[2],
        FieldMapError::ReadsAsOutsideRange { option, .. } if option == "XS"
    ));
    assert_eq!(
        errors[3],
        FieldMapError::UnknownDefault {
            default: "medium".into()
        }
    );

    let kinds: Vec<_> = errors.iter().map(discriminant).collect();
    for (i, a) in kinds.iter().enumerate() {
        for b in &kinds[i + 1..] {
            assert_ne!(a, b);
        }
    }
}

#[test]
fn read_only_partial_map_accepted() {
    let spec = field_spec(
        "M",
        vec![
            option("Low", "XS-S", false, None),
            option("High", "XL-4XL", false, None),
        ],
    );
    let map = FieldMap::<Size>::new(spec).unwrap();
    assert!(map.is_read_only());
    assert_eq!(map.option("High").unwrap().reads_as, Size::Xxxxl);
}

#[test]
fn aliases_beside_full_partition_accepted() {
    let mut options = partition();
    options.push(option("Low", "XS-S", false, None));
    options.push(option("Medium", "M-L", false, None));
    options.push(option("High", "XL-4XL", false, Some("2XL")));
    let map = FieldMap::<Size>::new(field_spec("M", options)).unwrap();
    assert!(!map.is_read_only());
    assert_eq!(map.default(), Size::M);
    assert_eq!(map.option("High").unwrap().reads_as, Size::Xxl);
    assert_eq!(map.option("Medium").unwrap().reads_as, Size::L);
    assert_eq!(map.field_ref().options.len(), 11);
}

#[test]
fn labels_mode_requires_one_option_placeholder() {
    for label in ["effort", "effort:{option}:{option}", ""] {
        let spec = FieldMapSpec {
            source: SourceSpec::Labels {
                label: label.into(),
            },
            default: "M".into(),
            options: partition(),
        };
        assert_eq!(
            FieldMap::<Size>::new(spec),
            Err(FieldMapError::LabelPattern {
                label: label.into()
            }),
            "{label:?}"
        );
    }
}

#[test]
fn read_labels_maps_two_matches_to_labels_conflict_for_any_role() {
    for (pattern, first, name, second) in [
        ("effort:{option}", "effort:S", "S", "effort:L"),
        ("{option} priority", "high priority", "high", "low priority"),
    ] {
        let unrelated = ["bug", "mintswarm"];
        assert_eq!(read_labels(pattern, &unrelated), FieldRead::Unset);
        assert_eq!(FieldRead::Unset.finding(), None);

        let one = read_labels(pattern, &["bug", first]);
        let FieldRead::Value(value) = &one else {
            panic!("{pattern}: {one:?}");
        };
        assert_eq!(value.field_id, None);
        assert_eq!(value.option_id, None);
        assert_eq!(value.name, name);
        assert_eq!(one.finding(), None);

        let both = read_labels(pattern, &[first, "bug", second]);
        assert!(
            matches!(&both, FieldRead::Conflict(values) if values.len() == 2),
            "{pattern}: {both:?}"
        );
        assert_eq!(both.finding(), Some(FieldFinding::LabelsConflict));
    }
    assert_eq!(
        read_labels("effort:{option}", &["effort:"]),
        FieldRead::Unset
    );

    // The text around the option compares case-insensitively, as on the GitHub edge, so a
    // mixed-case pair is a conflict, not one value.
    let mixed = read_labels("effort:{option}", &["effort:S", "Effort:L"]);
    assert!(
        matches!(&mixed, FieldRead::Conflict(values)
            if values.iter().map(|value| value.name.as_str()).eq(["S", "L"])),
        "{mixed:?}"
    );
    assert_eq!(mixed.finding(), Some(FieldFinding::LabelsConflict));
}

#[test]
fn label_pattern_is_the_one_labels_matcher() {
    let pattern = LabelPattern::new("effort:{option}").unwrap();
    assert_eq!(pattern.option_of("effort:S"), Some("S"));
    assert_eq!(pattern.option_of("EFFORT:xl"), Some("xl"));
    assert_eq!(pattern.option_of("effort:"), None);
    assert_eq!(pattern.option_of("priority:S"), None);
    assert_eq!(pattern.label("M"), "effort:M");

    let pattern = LabelPattern::new("[{option}] size").unwrap();
    assert_eq!(pattern.option_of("[M] Size"), Some("M"));
    assert_eq!(pattern.option_of("[M] size!"), None);

    assert_eq!(LabelPattern::new("effort"), None);
    assert_eq!(LabelPattern::new("{option}:{option}"), None);
}

#[test]
fn no_cost_model_is_none() {
    let model = NoCostModel;
    for account in ["main", "spare", ""] {
        let account = AccountName(account.into());
        for &size in Size::ALL {
            assert_eq!(model.cost(&account, size), None);
        }
    }
}

/// A tracker that answers every call at once.
struct Stub;

fn unused<T>() -> std::future::Ready<Result<T, FieldError>> {
    ready(Err(FieldError::Transport("unused".into())))
}

impl Tracker for Stub {
    type Error = FieldError;

    fn fetch_ready(&self) -> impl Future<Output = Result<Vec<Issue>, Self::Error>> + Send {
        unused()
    }

    fn post_comment(
        &self,
        _issue: IssueNumber,
        _body: String,
    ) -> impl Future<Output = Result<Comment, Self::Error>> + Send {
        unused()
    }

    fn set_status(
        &self,
        _issue: IssueNumber,
        _status: String,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        unused()
    }

    fn create_sub_item(
        &self,
        _parent: IssueNumber,
        _item: SubItem,
    ) -> impl Future<Output = Result<Issue, Self::Error>> + Send {
        unused()
    }

    fn link_pr(
        &self,
        _issue: IssueNumber,
        _pr: PrNumber,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        unused()
    }

    fn read_links(
        &self,
        _issue: IssueNumber,
    ) -> impl Future<Output = Result<Vec<Link>, Self::Error>> + Send {
        unused()
    }

    fn link_dependency(
        &self,
        _issue: IssueNumber,
        _blocker: IssueNumber,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        unused()
    }

    fn read_issue(
        &self,
        _issue: IssueNumber,
        _since: Option<Cursor>,
    ) -> impl Future<Output = Result<IssueRead, Self::Error>> + Send {
        unused()
    }

    fn publish_artifact(
        &self,
        _issue: IssueNumber,
        _artifact: Artifact,
    ) -> impl Future<Output = Result<String, Self::Error>> + Send {
        unused()
    }

    fn set_issue_type(
        &self,
        _issue: IssueNumber,
        _kind: IssueType,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        unused()
    }

    fn assign(
        &self,
        _issue: IssueNumber,
        _who: Vec<Login>,
        _reason: AssignmentReason,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        unused()
    }

    fn release(&self, _issue: IssueNumber) -> impl Future<Output = Result<(), Self::Error>> + Send {
        unused()
    }

    fn read_field(
        &self,
        _issue: IssueNumber,
        field: &FieldRef,
    ) -> impl Future<Output = Result<FieldRead, FieldError>> + Send {
        ready(Ok(FieldRead::Value(FieldValue {
            field_id: Some(7),
            option_id: Some(70),
            name: field.options[0].clone(),
        })))
    }

    fn set_field(
        &self,
        _issue: IssueNumber,
        _field: &FieldRef,
        option: Option<&str>,
    ) -> impl Future<Output = Result<(), FieldError>> + Send {
        ready(
            option
                .map(|_| ())
                .ok_or(FieldError::Transport("cleared".into())),
        )
    }
}

fn poll_once<F: Future + Send>(future: F) -> F::Output {
    let mut cx = Context::from_waker(Waker::noop());
    match pin!(future).poll(&mut cx) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the stub's future is ready"),
    }
}

#[test]
fn tracker_declares_field_operations() {
    let map = FieldMap::<Size>::new(field_spec("M", partition())).unwrap();
    let field = map.field_ref();
    let tracker = Stub;

    let read: Result<FieldRead, FieldError> = poll_once(tracker.read_field(IssueNumber(1), &field));
    assert!(matches!(read, Ok(FieldRead::Value(FieldValue { ref name, .. })) if name == "XS"));

    let set: Result<(), FieldError> =
        poll_once(tracker.set_field(IssueNumber(1), &field, Some("M")));
    assert_eq!(set, Ok(()));
    let cleared: Result<(), FieldError> =
        poll_once(tracker.set_field(IssueNumber(1), &field, None));
    assert!(cleared.is_err());
}

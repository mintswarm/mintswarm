//! The tracker's capability traits, their composition, and the shapes they read.

use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{
    Artifacts, Assignments, Comments, Dependency, Direction, Fields, Issue, Issues, LinkKind,
    Links, Login, PullLinks, Statuses, Tracker,
};

// Checked at compile time: any type with every capability is a `Tracker`, and a `Tracker` has
// one unambiguous error.
#[expect(dead_code, reason = "compile-time checks, never called")]
const _: () = {
    fn is_tracker<T: Tracker>() {}

    fn composes<
        T: Issues + Links + PullLinks + Statuses + Comments + Artifacts + Assignments + Fields,
    >() {
        is_tracker::<T>()
    }

    fn one_error<T: Tracker>(error: T::Error) -> Box<dyn std::error::Error + Send + Sync> {
        Box::new(error)
    }
};

#[test]
fn link_kind_has_no_dependency_variants() {
    for kind in [LinkKind::Parent, LinkKind::SubItem, LinkKind::PullRequest] {
        let name = match kind {
            LinkKind::Parent => "parent",
            LinkKind::SubItem => "sub_item",
            LinkKind::PullRequest => "pull_request",
        };
        assert_eq!(serde_json::to_value(kind).unwrap(), name);
    }
}

#[test]
fn issue_carries_sub_issue_and_blocked_by_totals() {
    let issue = Issue {
        number: IssueNumber(5),
        title: "Title".into(),
        body: "Body".into(),
        state: "open".into(),
        labels: vec!["mintswarm".into()],
        issue_type: None,
        author: Login("someone".into()),
        url: "https://example.com/owner/name/issues/5".into(),
        sub_issues: 3,
        blocked_by: 1,
    };
    let json = serde_json::to_value(&issue).unwrap();
    assert_eq!(json["sub_issues"], 3);
    assert_eq!(json["blocked_by"], 1);
    let back: Issue = serde_json::from_value(json).unwrap();
    assert_eq!(back, issue);
    assert_eq!((back.sub_issues, back.blocked_by), (3, 1));
}

#[test]
fn dependency_and_direction_round_trip() {
    let dependency = Dependency {
        repository: "owner/name".into(),
        number: IssueNumber(7),
        open: true,
    };
    let json = serde_json::to_value(&dependency).unwrap();
    let back: Dependency = serde_json::from_value(json).unwrap();
    assert_eq!(back, dependency);

    for (direction, name) in [
        (Direction::BlockedBy, "blocked_by"),
        (Direction::Blocking, "blocking"),
    ] {
        assert_eq!(serde_json::to_value(direction).unwrap(), name);
        assert_eq!(
            serde_json::from_value::<Direction>(name.into()).unwrap(),
            direction
        );
    }
}

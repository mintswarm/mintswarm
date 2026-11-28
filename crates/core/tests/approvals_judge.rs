//! The approval judge: the judging cases of `watch-plan-approvals.py`'s selftest, ported with
//! `/approve-plan` as `approve` and the dogfood markers as `render::decision` markers; one case
//! per check of design #548 §6.3, their order, the binding per package kind, a widened
//! footprint, relayed steers, and when the approver is needed. Outcome classes only.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

use mintswarm_config::{
    AWAITING_APPROVAL, Action, ActionTable, ActionTrust, People, Trust, TrustMatch,
};
use mintswarm_core::approvals::judge::{
    Approver, ApproverNotRead, Binding, Facts, Judged, Refusal, Verdict, judge, latest_package,
    needs_approver,
};
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{Comment, Issue, IssueType, Login, RepoRole};
use mintswarm_rules::command::PackageKind;
use mintswarm_rules::render::decision::{Marker, Ruling};

const BOT: &str = "mintswarm-dev-keeper[bot]";
const OWNER: &str = "zmij";
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn url(id: u64) -> String {
    format!("https://github.com/mintswarm/mintswarm/issues/81#issuecomment-{id}")
}

fn at(minute: u32) -> String {
    format!("2026-11-24T10:{minute:02}:00Z")
}

/// A comment by a person, posted at `minute` and never edited.
fn comment(id: u64, login: &str, minute: u32, body: &str) -> Comment {
    Comment {
        id,
        author: Login(login.to_owned()),
        author_is_bot: false,
        body: body.to_owned(),
        created_at: at(minute),
        updated_at: at(minute),
        url: url(id),
    }
}

/// A comment by an account the tracker flags as a bot.
fn bot_comment(id: u64, login: &str, minute: u32, body: &str) -> Comment {
    Comment {
        author_is_bot: true,
        ..comment(id, login, minute, body)
    }
}

fn package_body(kind: PackageKind, sha256: &str) -> String {
    format!(
        "{}\n{}\nPlease approve.",
        kind.marker(),
        kind.digest_line(sha256)
    )
}

/// A plan package by the plane.
fn package(id: u64, minute: u32) -> Comment {
    bot_comment(id, BOT, minute, &package_body(PackageKind::Plan, SHA))
}

/// A record by the plane answering `answer`.
fn record(id: u64, minute: u32, answer: u64) -> Comment {
    let marker = Marker::Decision {
        kind: PackageKind::Plan,
        ruling: Ruling::Approved,
        answer: Some(answer),
    };
    bot_comment(id, BOT, minute, &marker.lines())
}

/// An answer by the plane to `answer`, with `text` after the marker.
fn answer(id: u64, minute: u32, answer: u64, text: &str) -> Comment {
    let body = format!("{}{text}", Marker::Answer(answer).lines());
    bot_comment(id, BOT, minute, &body)
}

fn issue(author: &str) -> Issue {
    Issue {
        number: IssueNumber(81),
        title: "An issue".to_owned(),
        body: String::new(),
        state: "open".to_owned(),
        labels: Vec::new(),
        issue_type: Some(IssueType::Implementation),
        author: Login(author.to_owned()),
        author_is_bot: false,
        url: "https://github.com/mintswarm/mintswarm/issues/81".to_owned(),
        sub_issues: 0,
        blocked_by: 0,
    }
}

fn owner_rows() -> ActionTable {
    let owner = ActionTrust {
        people: People::IssueOwner,
        apps: Vec::new(),
    };
    ActionTable::default()
        .with(Action::ApprovePlan, owner.clone())
        .with(Action::ApproveDesign, owner.clone())
        .with(Action::ApproveDecomposition, owner)
}

/// Everything a case may change, with the dogfood selftest's defaults: issue 81 by `zmij`,
/// trusted, owned by `zmij` with write access, awaiting approval.
struct Case {
    comments: Vec<Comment>,
    issue: Issue,
    trust: Trust,
    owner: Login,
    approver: Option<Approver<'static>>,
    role: RepoRole,
    state: String,
    actions: ActionTable,
    self_logins: Vec<Login>,
}

fn case(comments: Vec<Comment>) -> Case {
    Case {
        comments,
        issue: issue(OWNER),
        trust: Trust {
            labels: Vec::new(),
            authors: vec![Login(OWNER.to_owned())],
            r#match: TrustMatch::Any,
        },
        owner: Login(OWNER.to_owned()),
        approver: None,
        role: RepoRole::Write,
        state: AWAITING_APPROVAL.to_owned(),
        actions: owner_rows(),
        self_logins: vec![Login(BOT.to_owned())],
    }
}

impl Case {
    fn run(&self) -> Result<Vec<Judged>, ApproverNotRead> {
        let approver = self.approver.unwrap_or(Approver::Owner {
            login: &self.owner,
            role: self.role,
        });
        judge(&Facts {
            issue: &self.issue,
            comments: &self.comments,
            trust: &self.trust,
            approver,
            state: &self.state,
            actions: &self.actions,
            self_logins: &self.self_logins,
        })
    }

    /// Each outcome's comment and class.
    fn classes(&self) -> Vec<(u64, Class)> {
        self.run()
            .unwrap()
            .into_iter()
            .map(|judged| (judged.comment, Class::of(&judged.verdict)))
            .collect()
    }
}

/// An outcome's class.
#[derive(Debug, PartialEq, Eq)]
enum Class {
    Approve,
    Reject,
    Check(u8),
    Help(Option<PackageKind>),
    Hint(Option<PackageKind>),
}

impl Class {
    fn of(verdict: &Verdict) -> Self {
        match verdict {
            Verdict::Approve { .. } => Class::Approve,
            Verdict::Reject { .. } => Class::Reject,
            Verdict::Refused(refusal) => Class::Check(refusal.check()),
            Verdict::Help { context } => Class::Help(*context),
            Verdict::Hint { context, .. } => Class::Hint(*context),
        }
    }
}

fn refusal_of(case: &Case, id: u64) -> Refusal {
    let judged = case.run().unwrap();
    let found = judged.iter().find(|judged| judged.comment == id).unwrap();
    match &found.verdict {
        Verdict::Refused(refusal) => refusal.clone(),
        other => panic!("{other:?}"),
    }
}

// The 14 judging checks of the selftest's `watch-plan-approvals.py` section.

#[test]
fn a_trusted_command_after_the_package_approves() {
    let case = case(vec![package(10, 0), comment(11, OWNER, 5, "approve")]);
    assert_eq!(case.classes(), [(11, Class::Approve)]);
}

#[test]
fn a_note_crlf_and_the_login_in_another_case_approve() {
    let case = case(vec![
        package(10, 0),
        comment(11, "ZMIJ", 5, "Approve \r\nlooks good"),
    ]);
    assert_eq!(case.classes(), [(11, Class::Approve)]);
    let judged = case.run().unwrap();
    assert!(matches!(&judged[0].verdict, Verdict::Approve { note, .. } if note == "looks good"));
}

#[test]
fn a_login_that_is_not_the_owner_is_refused() {
    let case = case(vec![package(10, 0), comment(11, "mallory", 5, "approve")]);
    assert_eq!(case.classes(), [(11, Class::Check(3))]);
    assert_eq!(refusal_of(&case, 11), Refusal::NotOwner);
}

#[test]
fn a_bot_login_is_refused() {
    let case = case(vec![
        package(10, 0),
        bot_comment(11, "helper[bot]", 5, "approve"),
    ]);
    assert_eq!(case.classes(), [(11, Class::Check(1))]);
}

#[test]
fn an_account_of_type_bot_is_refused() {
    let case = case(vec![package(10, 0), bot_comment(11, OWNER, 5, "approve")]);
    assert_eq!(case.classes(), [(11, Class::Check(1))]);
}

#[test]
fn an_issue_not_awaiting_approval_is_refused() {
    let mut case = case(vec![package(10, 0), comment(11, OWNER, 5, "approve")]);
    case.state = "implementing".to_owned();
    assert_eq!(case.classes(), [(11, Class::Check(5))]);
}

#[test]
fn an_issue_with_no_marked_package_is_refused() {
    let case = case(vec![
        bot_comment(10, BOT, 0, "An unmarked package"),
        comment(11, OWNER, 5, "approve"),
    ]);
    assert_eq!(case.classes(), [(11, Class::Check(6))]);
}

#[test]
fn a_command_before_the_latest_package_is_refused() {
    let case = case(vec![
        package(9, 0),
        comment(11, OWNER, 5, "approve"),
        package(12, 10),
    ]);
    assert_eq!(case.classes(), [(11, Class::Check(7))]);
}

#[test]
fn an_edited_command_is_refused() {
    let mut edited = comment(11, OWNER, 5, "approve");
    edited.updated_at = at(7);
    let case = case(vec![package(10, 0), edited]);
    assert_eq!(case.classes(), [(11, Class::Check(8))]);
}

#[test]
fn a_second_valid_command_is_already_decided() {
    let case = case(vec![
        package(10, 0),
        comment(11, OWNER, 5, "approve"),
        comment(12, OWNER, 6, "approve"),
    ]);
    assert_eq!(
        case.classes(),
        [(11, Class::Approve), (12, Class::Check(9))]
    );
}

#[test]
fn an_answered_command_gives_nothing_and_a_later_one_is_already_decided() {
    let case = case(vec![
        package(10, 0),
        comment(11, OWNER, 5, "approve"),
        record(13, 6, 11),
        comment(14, OWNER, 7, "approve"),
    ]);
    assert_eq!(case.classes(), [(14, Class::Check(9))]);
}

#[test]
fn a_command_answered_with_a_refusal_is_not_judged_again() {
    let case = case(vec![
        package(10, 0),
        comment(11, OWNER, 5, "approve"),
        answer(
            13,
            6,
            11,
            "Not done: `the issue is not awaiting approval`\n",
        ),
        comment(14, OWNER, 7, "approve"),
    ]);
    assert_eq!(case.classes(), [(14, Class::Approve)]);
}

#[test]
fn an_approval_of_an_earlier_package_leaves_the_current_one_open() {
    let case = case(vec![
        package(9, 0),
        comment(11, OWNER, 1, "approve"),
        record(12, 2, 11),
        package(10, 3),
        comment(14, OWNER, 5, "approve"),
    ]);
    assert_eq!(case.classes(), [(14, Class::Approve)]);
}

#[test]
fn an_answer_marker_from_a_person_does_not_answer_and_the_planes_command_is_a_bots() {
    let case = case(vec![
        package(10, 0),
        comment(11, "mallory", 5, "approve"),
        comment(12, "mallory", 6, &Marker::Answer(11).lines()),
        bot_comment(13, BOT, 7, "approve"),
    ]);
    assert_eq!(
        case.classes(),
        [(11, Class::Check(3)), (13, Class::Check(1))]
    );
}

// The help and hint cases.

#[test]
fn help_with_a_plan_package_carries_plan() {
    let case = case(vec![package(10, 0), comment(11, OWNER, 5, "help\napprove")]);
    assert_eq!(case.classes(), [(11, Class::Help(Some(PackageKind::Plan)))]);
}

#[test]
fn help_after_a_decomposition_package_carries_decomposition() {
    let decomposition = bot_comment(
        10,
        BOT,
        1,
        &format!(
            "{}\npackage: none\nPlease approve.",
            PackageKind::Decomposition.marker()
        ),
    );
    let case = case(vec![
        package(9, 0),
        decomposition,
        comment(11, OWNER, 5, "help"),
    ]);
    assert_eq!(
        case.classes(),
        [(11, Class::Help(Some(PackageKind::Decomposition)))]
    );
}

#[test]
fn help_after_a_design_package_carries_design() {
    let design = bot_comment(
        10,
        BOT,
        0,
        &format!("{}\nPlease approve.", PackageKind::Design.marker()),
    );
    let case = case(vec![design, comment(11, OWNER, 5, "help")]);
    assert_eq!(
        case.classes(),
        [(11, Class::Help(Some(PackageKind::Design)))]
    );
}

#[test]
fn help_while_implementing_carries_none() {
    let mut case = case(vec![package(10, 0), comment(11, OWNER, 5, "help")]);
    case.state = "implementing".to_owned();
    assert_eq!(case.classes(), [(11, Class::Help(None))]);
}

#[test]
fn a_help_and_a_hint_answered_by_the_plane_give_nothing() {
    let case = case(vec![
        package(10, 0),
        comment(11, OWNER, 5, "help"),
        comment(12, OWNER, 6, "aprove"),
        answer(13, 7, 11, "Commands"),
        answer(14, 8, 12, "Did you mean"),
    ]);
    assert_eq!(case.classes(), []);
}

#[test]
fn an_answer_marker_from_a_person_does_not_answer_a_help() {
    let case = case(vec![
        package(10, 0),
        comment(11, OWNER, 5, "help"),
        comment(12, "mallory", 6, &Marker::Answer(11).lines()),
    ]);
    assert_eq!(case.classes(), [(11, Class::Help(Some(PackageKind::Plan)))]);
}

#[test]
fn a_help_and_a_near_miss_from_another_bot_give_nothing() {
    let case = case(vec![
        package(10, 0),
        bot_comment(11, "helper[bot]", 5, "help"),
        bot_comment(12, "helper[bot]", 6, "aprove"),
    ]);
    assert_eq!(case.classes(), []);
}

#[test]
fn hint_help_and_approve_come_out_in_comment_order() {
    let case = case(vec![
        package(10, 0),
        comment(11, OWNER, 5, "aprove"),
        comment(12, OWNER, 6, "help"),
        comment(13, OWNER, 7, "approve"),
    ]);
    let plan = Some(PackageKind::Plan);
    assert_eq!(
        case.classes(),
        [
            (11, Class::Hint(plan)),
            (12, Class::Help(plan)),
            (13, Class::Approve)
        ]
    );
}

// One case per check, and their order.

#[test]
fn each_check_has_its_own_refusal() {
    let approve = || comment(11, OWNER, 5, "approve");

    let flagged = case(vec![package(10, 0), bot_comment(11, OWNER, 5, "approve")]);
    assert_eq!(refusal_of(&flagged, 11), Refusal::Bot);

    let mut untrusted = case(vec![package(10, 0), approve()]);
    untrusted.trust.authors.clear();
    assert_eq!(refusal_of(&untrusted, 11), Refusal::Untrusted);

    let mut no_row = case(vec![package(10, 0), approve()]);
    no_row.actions = ActionTable::default();
    assert_eq!(
        refusal_of(&no_row, 11),
        Refusal::NotTrustedWith(Action::ApprovePlan)
    );

    let mut no_owner = case(vec![package(10, 0), approve()]);
    no_owner.approver = Some(Approver::NoOwner);
    assert_eq!(refusal_of(&no_owner, 11), Refusal::NoOwner);

    let not_owner = case(vec![package(10, 0), comment(11, "mallory", 5, "approve")]);
    assert_eq!(refusal_of(&not_owner, 11), Refusal::NotOwner);

    let mut read_only = case(vec![package(10, 0), approve()]);
    read_only.role = RepoRole::Read;
    assert_eq!(
        refusal_of(&read_only, 11),
        Refusal::NoWriteAccess(RepoRole::Read)
    );

    let mut planning = case(vec![package(10, 0), approve()]);
    planning.state = "planning".to_owned();
    assert_eq!(
        refusal_of(&planning, 11),
        Refusal::NotAwaiting("planning".to_owned())
    );

    let no_package = case(vec![approve()]);
    assert_eq!(refusal_of(&no_package, 11), Refusal::NoPackage);

    // A newer package without a well-formed digest line leaves no current package.
    let not_current = case(vec![
        package(9, 0),
        bot_comment(10, BOT, 1, PackageKind::Plan.marker()),
        approve(),
    ]);
    assert_eq!(refusal_of(&not_current, 11), Refusal::NoPackage);

    let predates = case(vec![approve(), package(12, 10)]);
    assert_eq!(refusal_of(&predates, 11), Refusal::Predates);

    // The same time breaks by comment id.
    let same_time = case(vec![package(12, 5), approve()]);
    assert_eq!(refusal_of(&same_time, 11), Refusal::Predates);

    let mut edited = approve();
    edited.updated_at = at(6);
    let edited = case(vec![package(10, 0), edited]);
    assert_eq!(refusal_of(&edited, 11), Refusal::Edited);

    let recorded = case(vec![
        package(10, 0),
        bot_comment(
            12,
            BOT,
            3,
            &Marker::Decision {
                kind: PackageKind::Plan,
                ruling: Ruling::Rejected,
                answer: None,
            }
            .lines(),
        ),
        approve(),
    ]);
    assert_eq!(refusal_of(&recorded, 11), Refusal::AlreadyDecided);

    let checks: Vec<u8> = [
        Refusal::Bot,
        Refusal::Untrusted,
        Refusal::NotTrustedWith(Action::ApprovePlan),
        Refusal::NoOwner,
        Refusal::NotOwner,
        Refusal::NoWriteAccess(RepoRole::Read),
        Refusal::NotAwaiting("planning".to_owned()),
        Refusal::NoPackage,
        Refusal::Predates,
        Refusal::Edited,
        Refusal::AlreadyDecided,
    ]
    .iter()
    .map(Refusal::check)
    .collect();
    assert_eq!(checks, [1, 2, 3, 3, 3, 4, 5, 6, 7, 8, 9]);
}

#[test]
fn a_people_none_row_refuses_at_check_three_for_every_kind() {
    for (kind, action) in [
        (PackageKind::Plan, Action::ApprovePlan),
        (PackageKind::Design, Action::ApproveDesign),
        (PackageKind::Decomposition, Action::ApproveDecomposition),
    ] {
        let mut case = case(vec![
            bot_comment(10, BOT, 0, &package_body(kind, SHA)),
            comment(11, OWNER, 5, "approve"),
        ]);
        case.actions = owner_rows().with(action, ActionTrust::default());
        assert_eq!(refusal_of(&case, 11), Refusal::NotTrustedWith(action));
    }
}

#[test]
fn the_first_failing_check_is_reported() {
    let mut bot_edited = bot_comment(11, "helper[bot]", 5, "approve");
    bot_edited.updated_at = at(6);
    let mut several = case(vec![package(10, 0), bot_edited]);
    several.state = "planning".to_owned();
    assert_eq!(several.classes(), [(11, Class::Check(1))]);

    let mut predates_edited = comment(11, OWNER, 5, "approve");
    predates_edited.updated_at = at(20);
    let case = case(vec![predates_edited, package(12, 10)]);
    assert_eq!(case.classes(), [(11, Class::Check(7))]);
}

#[test]
fn a_passing_command_binds_its_package_of_every_kind() {
    for kind in PackageKind::ALL {
        let case = case(vec![
            bot_comment(10, BOT, 0, &package_body(kind, SHA)),
            comment(11, OWNER, 5, "approve"),
        ]);
        let judged = case.run().unwrap();
        let expected = Binding {
            kind,
            package_url: url(10),
            package_id: 10,
            sha256: SHA.to_owned(),
        };
        assert!(
            matches!(&judged[..], [Judged { verdict: Verdict::Approve { binding, .. }, .. }] if *binding == expected),
            "{kind:?}: {judged:?}"
        );
    }
}

#[test]
fn a_passing_reject_binds_the_package() {
    let case = case(vec![
        package(10, 0),
        comment(11, OWNER, 5, "reject\nNot yet."),
    ]);
    assert_eq!(case.classes(), [(11, Class::Reject)]);
    let judged = case.run().unwrap();
    assert!(matches!(
        &judged[0].verdict,
        Verdict::Reject { binding, note } if binding.sha256 == SHA && note == "Not yet."
    ));
}

#[test]
fn a_widened_footprint_binds_a_later_approve_to_the_new_digest() {
    let widened = bot_comment(12, BOT, 10, &package_body(PackageKind::Plan, SHA_B));
    let case = case(vec![
        package(10, 0),
        comment(11, OWNER, 5, "approve"),
        widened,
        comment(13, OWNER, 15, "approve"),
    ]);
    assert_eq!(
        case.classes(),
        [(11, Class::Check(7)), (13, Class::Approve)]
    );
    let judged = case.run().unwrap();
    assert!(matches!(
        &judged[1].verdict,
        Verdict::Approve { binding, .. } if binding.sha256 == SHA_B && binding.package_url == url(12)
    ));
}

#[test]
fn comments_are_judged_in_time_order_whatever_the_read_order() {
    let case = case(vec![
        comment(12, OWNER, 6, "approve"),
        comment(11, OWNER, 5, "approve"),
        package(10, 0),
    ]);
    assert_eq!(
        case.classes(),
        [(11, Class::Approve), (12, Class::Check(9))]
    );
}

// Relayed approval steers are no approval form: the plane's own comment with an approve line
// is a bot's, whatever markers and digest it carries.

fn steer(sha256: &str) -> String {
    format!(
        "<!-- mintswarm: steer -->\n<!-- mintswarm: approval steer -->\n\nApprove\n\n{}\n",
        PackageKind::Plan.digest_line(sha256)
    )
}

#[test]
fn a_relayed_steer_with_the_current_digest_does_not_approve() {
    let case = case(vec![package(10, 0), bot_comment(11, BOT, 5, &steer(SHA))]);
    assert_eq!(case.classes(), [(11, Class::Check(1))]);
}

#[test]
fn a_relayed_steer_with_a_stale_digest_is_refused() {
    let case = case(vec![package(10, 0), bot_comment(11, BOT, 5, &steer(SHA_B))]);
    assert_eq!(case.classes(), [(11, Class::Check(1))]);
}

#[test]
fn a_bot_approve_without_steer_markers_is_refused_as_a_bot() {
    let mut unflagged = comment(13, BOT, 7, "Approve");
    unflagged.author_is_bot = false;
    let case = case(vec![
        package(10, 0),
        bot_comment(11, "helper[bot]", 5, "Approve"),
        bot_comment(12, "relay", 6, "Approve"),
        unflagged,
    ]);
    assert_eq!(
        case.classes(),
        [
            (11, Class::Check(1)),
            (12, Class::Check(1)),
            (13, Class::Check(1))
        ]
    );
}

// When the approver is needed.

#[test]
fn only_an_open_approve_or_reject_by_a_person_needs_the_approver() {
    let own = [Login(BOT.to_owned())];
    let help = [package(10, 0), comment(11, OWNER, 5, "help")];
    assert!(!needs_approver(&help, &own));
    let approve = [package(10, 0), comment(11, OWNER, 5, "approve")];
    assert!(needs_approver(&approve, &own));
    let reject = [package(10, 0), comment(11, "mallory", 5, "reject")];
    assert!(needs_approver(&reject, &own));
    let by_bot = [package(10, 0), bot_comment(11, "helper[bot]", 5, "approve")];
    assert!(!needs_approver(&by_bot, &own));
    let answered = [
        package(10, 0),
        comment(11, OWNER, 5, "approve"),
        record(12, 6, 11),
    ];
    assert!(!needs_approver(&answered, &own));
}

#[test]
fn an_approve_without_the_approver_read_is_an_error() {
    let mut not_read = case(vec![package(10, 0), comment(11, OWNER, 5, "approve")]);
    not_read.approver = Some(Approver::NotRead);
    assert_eq!(not_read.run(), Err(ApproverNotRead));

    // Help needs no approver, nor does an untrusted issue's command.
    let mut help = case(vec![package(10, 0), comment(11, OWNER, 5, "help")]);
    help.approver = Some(Approver::NotRead);
    assert!(help.run().is_ok());
    let mut untrusted = case(vec![package(10, 0), comment(11, OWNER, 5, "approve")]);
    untrusted.approver = Some(Approver::NotRead);
    untrusted.trust.authors.clear();
    assert_eq!(untrusted.classes(), [(11, Class::Check(2))]);
}

#[test]
fn the_latest_package_is_the_planes_own() {
    let own = [Login(BOT.to_owned())];
    let comments = [
        package(10, 0),
        comment(11, "mallory", 5, &package_body(PackageKind::Design, SHA_B)),
    ];
    let latest = latest_package(&comments, &own).unwrap();
    assert_eq!(latest.comment.id, 10);
    assert_eq!(latest.kind, PackageKind::Plan);
    assert_eq!(latest.sha256.as_deref(), Some(SHA));
}

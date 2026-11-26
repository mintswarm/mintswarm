//! The pull request labelling rule: component labels mirrored from the diff, the band label from
//! the band, and the labels to create. Errors are checked by variant, never by message.

use mintswarm_domain::records::Band;
use mintswarm_rules::exit::Unusable;
use mintswarm_rules::pr_labels::{
    COMPONENT_COLOUR, COMPONENT_DESCRIPTION, Changes, Create, Error, MAX_DESCRIPTION, MAX_NAME,
    VERDICT_COLOUR, VERDICT_DESCRIPTION, VERDICT_LABEL, View, changes, view,
};

const REPOSITORY: &[u8] = include_bytes!("../../../.mintswarm/config.yaml");

const NONE: [&str; 0] = [];

/// The sections every config needs besides `tracker`, `components` and `shared`.
const PIPELINE: &str = "pipeline:
  plan_approval:
    mode: always
    limits: { files: 10, lines: 500, new_files: 3 }
  landing:
    mode: always
    footprint: { lines_factor: 1.5 }
";

/// A config with only the given `components`, holding one component `name` over `x/**`.
fn one_component(name: &str) -> Vec<u8> {
    format!("components:\n  {name}:\n    paths: [\"x/**\"]\n{PIPELINE}").into_bytes()
}

fn repository() -> View {
    view(REPOSITORY).expect("the repository config gives a view")
}

/// A small config with components `core` and `web`, `shared` docs, and the given
/// `tracker.components` block.
fn config(components: &str) -> Vec<u8> {
    format!(
        "tracker:\n  components:\n{components}\
         components:\n  \
           core:\n    paths: [\"crates/core/**\", \"docs/**\"]\n  \
           web:\n    paths: [\"web/**\"]\n\
         shared: [\"docs/**\"]\n\
         {PIPELINE}"
    )
    .into_bytes()
}

fn small(components: &str) -> View {
    view(&config(components)).expect("the small config gives a view")
}

fn run(view: &View, paths: &[&str], band: Option<Band>, pr: &[&str]) -> Changes {
    changes(view, paths, band, pr, &NONE).expect("changes")
}

const ALL_BANDS: [(Band, &str); 5] = [
    (Band::Safe, "safe"),
    (Band::LikelySafe, "likely-safe"),
    (Band::NeedsReview, "needs-review"),
    (Band::Uncertain, "uncertain"),
    (Band::Blocked, "blocked"),
];

#[test]
fn adds_the_component_label_from_the_pattern() {
    let view = repository();
    assert_eq!(view.label, "component:{name}");
    let out = run(&view, &["crates/runner/src/x.rs"], None, &[]);
    assert_eq!(out.add, ["component:runner"]);
    assert!(out.remove.is_empty());

    let out = run(
        &view,
        &["crates/runner/src/x.rs", "crates/core/src/y.rs"],
        None,
        &[],
    );
    assert_eq!(out.add, ["component:core", "component:runner"]);

    let custom = small("    source: labels\n    label: \"area/{name}\"\n");
    let out = run(&custom, &["web/app.tsx", "crates/core/lib.rs"], None, &[]);
    assert_eq!(out.add, ["area/core", "area/web"]);
}

#[test]
fn shared_and_stray_paths_add_no_label() {
    let view = repository();
    let out = run(
        &view,
        &["docs/architecture/repository-layout.md", "nowhere/x.txt"],
        None,
        &[],
    );
    assert_eq!(out, Changes::default());

    // `docs/**` is both shared and in `core` here.
    let view = small("    source: field\n");
    let out = run(&view, &["docs/a.md"], None, &[]);
    assert_eq!(out, Changes::default());
}

#[test]
fn field_mode_names_labels_component_name() {
    let view = small("    source: field\n    label: \"area/{name}\"\n");
    assert_eq!(view.label, "component:{name}");
    let out = run(&view, &["web/app.tsx"], None, &[]);
    assert_eq!(out.add, ["component:web"]);

    // No `tracker.components` at all reads as field mode.
    let view = view_of(&one_component("web"));
    assert_eq!(run(&view, &["x/a"], None, &[]).add, ["component:web"]);
}

fn view_of(bytes: &[u8]) -> View {
    view(bytes).expect("a view")
}

#[test]
fn untouched_component_labels_are_removed() {
    let view = repository();
    let out = run(
        &view,
        &["crates/runner/src/x.rs"],
        None,
        &["component:web", "component:gone", "component:runner"],
    );
    assert!(out.add.is_empty());
    assert_eq!(out.remove, ["component:gone", "component:web"]);
}

#[test]
fn only_family_labels_are_removed() {
    let view = repository();
    let others = ["mintswarm", "bug", "msw-plan:safe", "msw-state:done"];
    for band in [None, Some(Band::Safe)] {
        let mut pr = others.to_vec();
        pr.extend(["component:web", "msw-verdict:blocked"]);
        let out = run(&view, &["nowhere/x"], band, &pr);
        for other in others {
            assert!(!out.remove.iter().any(|r| r == other), "{other} removed");
        }
        assert!(out.remove.iter().any(|r| r == "component:web"));
        assert_eq!(
            out.remove.iter().any(|r| r == "msw-verdict:blocked"),
            band.is_some()
        );
    }
    // A bare `{name}` pattern, which would remove `bug`, is refused: see
    // `a_pattern_that_would_match_every_label_is_refused`.
}

#[test]
fn a_pattern_that_would_match_every_label_is_refused() {
    for label in ["{name}", "{name}{name}", "component", ""] {
        let yaml = config(&format!("    source: labels\n    label: \"{label}\"\n"));
        assert!(
            matches!(view(&yaml), Err(Error::Pattern { .. })),
            "{label:?}"
        );
    }
    let yaml = config("    source: labels\n");
    assert!(matches!(view(&yaml), Err(Error::Pattern { .. })));

    let by_hand = View {
        label: "{name}".to_owned(),
        ..repository()
    };
    assert!(matches!(
        changes(&by_hand, &["x"], None, &["bug"], &NONE),
        Err(Error::Pattern { .. })
    ));
}

#[test]
fn the_band_label_is_mirrored() {
    let view = repository();
    for (band, id) in ALL_BANDS {
        let label = format!("msw-verdict:{id}");
        let out = run(&view, &[], Some(band), &[]);
        assert_eq!(out.add, [label.as_str()]);
        assert!(out.remove.is_empty());

        // The same band again gives no change.
        let out = run(&view, &[], Some(band), &[&label]);
        assert_eq!(out, Changes::default());

        // Every other band label goes.
        let mut stale: Vec<String> = ALL_BANDS
            .iter()
            .filter(|(other, _)| *other != band)
            .map(|(_, other)| format!("msw-verdict:{other}"))
            .collect();
        let pr: Vec<&str> = stale.iter().map(String::as_str).collect();
        let out = run(&view, &[], Some(band), &pr);
        stale.sort();
        assert_eq!(out.add, [label.as_str()]);
        assert_eq!(out.remove, stale);
    }

    // Two stale bands and the current one.
    let out = run(
        &view,
        &[],
        Some(Band::Safe),
        &[
            "msw-verdict:blocked",
            "msw-verdict:safe",
            "msw-verdict:uncertain",
        ],
    );
    assert!(out.add.is_empty());
    assert_eq!(out.remove, ["msw-verdict:blocked", "msw-verdict:uncertain"]);

    // Without a band, band labels are neither added nor removed.
    let out = run(
        &view,
        &[],
        None,
        &["msw-verdict:blocked", "msw-verdict:safe"],
    );
    assert_eq!(out, Changes::default());
}

#[test]
fn names_compare_case_insensitively() {
    let view = repository();
    let out = run(
        &view,
        &["crates/runner/src/x.rs"],
        Some(Band::Safe),
        &["Component:Runner", "MSW-Verdict:Safe"],
    );
    assert_eq!(out, Changes::default());

    // Of two labels differing only in case, the one as built stays.
    let out = run(
        &view,
        &["crates/runner/src/x.rs"],
        Some(Band::Safe),
        &[
            "Component:Runner",
            "component:runner",
            "MSW-Verdict:Safe",
            "msw-verdict:SAFE",
            "msw-verdict:safe",
        ],
    );
    assert!(out.add.is_empty());
    assert_eq!(
        out.remove,
        ["Component:Runner", "MSW-Verdict:Safe", "msw-verdict:SAFE"]
    );

    // Without the built name, the first in byte order stays.
    let out = run(
        &view,
        &["crates/runner/src/x.rs"],
        None,
        &["component:RUNNER", "Component:Runner"],
    );
    assert_eq!(out.remove, ["component:RUNNER"]);

    // Repository labels in mixed case count as present.
    let out = changes(
        &view,
        &["crates/runner/src/x.rs"],
        Some(Band::LikelySafe),
        &NONE,
        &["COMPONENT:runner", "Msw-Verdict:Likely-Safe"],
    )
    .expect("changes");
    assert_eq!(out.add, ["component:runner", "msw-verdict:likely-safe"]);
    assert!(out.create.is_empty());
}

#[test]
fn create_holds_only_missing_labels() {
    assert_eq!(COMPONENT_COLOUR, "1d76db");
    assert_eq!(COMPONENT_DESCRIPTION, "mintswarm component");
    assert_eq!(VERDICT_LABEL, "msw-verdict:{value}");
    assert_eq!(VERDICT_COLOUR, "ededed");
    assert_eq!(VERDICT_DESCRIPTION, "mintswarm verdict");

    let view = repository();
    let out = changes(
        &view,
        &["crates/runner/src/x.rs", "crates/core/src/y.rs"],
        Some(Band::NeedsReview),
        &NONE,
        &["component:core", "bug"],
    )
    .expect("changes");
    assert_eq!(
        out.add,
        [
            "component:core",
            "component:runner",
            "msw-verdict:needs-review"
        ]
    );
    assert_eq!(
        out.create,
        [
            Create {
                name: "component:runner".to_owned(),
                colour: "1d76db".to_owned(),
                description: "mintswarm component".to_owned(),
            },
            Create {
                name: "msw-verdict:needs-review".to_owned(),
                colour: "ededed".to_owned(),
                description: "mintswarm verdict".to_owned(),
            },
        ]
    );

    // A label already on the PR is neither added nor created.
    let out = changes(
        &view,
        &["crates/runner/src/x.rs"],
        None,
        &["component:runner"],
        &NONE,
    )
    .expect("changes");
    assert_eq!(out, Changes::default());

    let json = serde_json::to_value(out_with_create()).expect("json");
    let create = &json["create"][0];
    assert_eq!(create["color"], "1d76db");
    assert!(create.get("colour").is_none());
}

fn out_with_create() -> Changes {
    run(&repository(), &["crates/runner/src/x.rs"], None, &[])
}

#[test]
fn over_long_names_and_descriptions_are_rejected() {
    let fifty = "n".repeat(MAX_NAME);
    let hundred = "d".repeat(MAX_DESCRIPTION);
    assert_eq!(MAX_NAME, 50);
    assert_eq!(MAX_DESCRIPTION, 100);
    assert!(Create::new(fifty.clone(), "ededed", hundred.clone()).is_ok());
    assert!(matches!(
        Create::new(format!("{fifty}n"), "ededed", "d"),
        Err(Error::NameTooLong { .. })
    ));
    assert!(matches!(
        Create::new("n", "ededed", format!("{hundred}d")),
        Err(Error::DescriptionTooLong { .. })
    ));
    // Characters, not bytes.
    assert!(Create::new("é".repeat(MAX_NAME), "ededed", "é".repeat(MAX_DESCRIPTION)).is_ok());

    // `component:` is 10 characters, so a 41-character name gives a 51-character label.
    let long = "c".repeat(41);
    let view = view_of(&one_component(&long));
    for repo in [&[][..], &["unrelated"][..]] {
        assert!(matches!(
            changes(&view, &["x/a"], None, &NONE, repo),
            Err(Error::NameTooLong { .. })
        ));
    }
    let fits = "c".repeat(40);
    let out = run(&view_of(&one_component(&fits)), &["x/a"], None, &[]);
    assert_eq!(out.add, [format!("component:{fits}")]);
}

#[test]
fn output_is_sorted_deduplicated_and_stable() {
    let view = repository();
    let paths_a = [
        "web/src/a.tsx",
        "crates/runner/x.rs",
        "crates/core/y.rs",
        "crates/runner/x.rs",
        "api/z.proto",
    ];
    let mut paths_b = paths_a;
    paths_b.reverse();
    let pr_a = [
        "msw-verdict:safe",
        "component:gone",
        "bug",
        "component:agents",
        "component:gone",
        "msw-verdict:blocked",
    ];
    let mut pr_b = pr_a;
    pr_b.reverse();
    let repo_a = ["component:core", "bug", "component:core"];
    let mut repo_b = repo_a;
    repo_b.reverse();

    let a = changes(&view, &paths_a, Some(Band::Uncertain), &pr_a, &repo_a).expect("a");
    let b = changes(&view, &paths_b, Some(Band::Uncertain), &pr_b, &repo_b).expect("b");
    let bytes_a = serde_json::to_string(&a).expect("json");
    let bytes_b = serde_json::to_string(&b).expect("json");
    assert_eq!(bytes_a, bytes_b);

    let ascending = |names: &[String]| names.windows(2).all(|pair| pair[0] < pair[1]);
    assert!(ascending(&a.add), "{a:?}");
    assert!(ascending(&a.remove), "{a:?}");
    let created: Vec<String> = a.create.iter().map(|c| c.name.clone()).collect();
    assert!(ascending(&created), "{a:?}");
    assert!(!a.add.is_empty() && !a.remove.is_empty() && !a.create.is_empty());

    let back: Changes = serde_json::from_str(&bytes_a).expect("round trip");
    assert_eq!(back, a);
}

#[test]
fn a_config_that_does_not_load_is_a_config_error() {
    assert!(matches!(view(b"a: [1\n"), Err(Error::Config(_))));
    assert!(matches!(view(b"\xff"), Err(Error::Config(_))));
    let yaml = config("    source: issues\n");
    assert!(matches!(view(&yaml), Err(Error::Config(_))));

    // The future tool reports every error as unusable input, exit 2.
    let error = view(b"a: [1\n").expect_err("does not load");
    let Error::Config(inner) = error.clone() else {
        unreachable!()
    };
    assert_eq!(Unusable::from(error), inner);
    let pattern = Unusable::from(Error::Pattern {
        pattern: "{name}".to_owned(),
    });
    assert!(!pattern.message().is_empty());
}

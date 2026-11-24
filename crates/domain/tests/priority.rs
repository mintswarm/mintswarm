//! The priority scale, a job's own level, the setup preset and the stride selector.

use mintswarm_domain::field::{
    FieldMap, FieldMapError, FieldMapSpec, FieldRead, FieldValue, OptionSpec, SourceSpec,
};
use mintswarm_domain::ids::JobId;
use mintswarm_domain::priority::{
    ClassState, EntryCandidate, Priority, PrioritySource, STRIDE_SCALE, ShareError, ShareState,
    own_level, setup_preset,
};
use mintswarm_domain::scale::{Scale, ScaleRange, Size};

fn option(name: &str, range: &str, reads_as: Option<&str>) -> OptionSpec {
    OptionSpec {
        name: name.into(),
        range: range.into(),
        write: false,
        reads_as: reads_as.map(str::to_owned),
    }
}

/// A read-only map with default `medium` over these options.
fn priority_map(options: Vec<OptionSpec>) -> FieldMap<Priority> {
    FieldMap::new(FieldMapSpec {
        source: SourceSpec::Field {
            name: "Priority".into(),
        },
        default: "medium".into(),
        options,
    })
    .expect("the priority map is valid")
}

fn value(name: &str) -> FieldValue {
    FieldValue {
        field_id: Some(7),
        option_id: None,
        name: name.into(),
    }
}

fn candidate(job: &str, level: Priority, ready_at: u64) -> EntryCandidate {
    EntryCandidate {
        job: JobId(job.into()),
        level,
        ready_at,
        size: Size::M,
    }
}

/// `per_level` candidates for each of `levels`, with ids that sort in ready order.
fn backlog(levels: &[Priority], per_level: usize) -> Vec<EntryCandidate> {
    levels
        .iter()
        .flat_map(|&level| {
            (0..per_level).map(move |n| candidate(&format!("{level}-{n:04}"), level, n as u64))
        })
        .collect()
}

/// One claim: observe, rank, charge the first; the admitted candidate leaves `candidates`.
fn admit(state: &mut ShareState, candidates: &mut Vec<EntryCandidate>) -> Priority {
    state.observe(candidates);
    let first = state.ranked(candidates, |_| 1)[0].clone();
    let at = candidates
        .iter()
        .position(|c| c.job == first)
        .expect("the ranked job is a candidate");
    let admitted = candidates.remove(at);
    state.charge(admitted.level, 1);
    admitted.level
}

#[test]
fn priority_scale_orders_and_round_trips() {
    use Priority::*;
    assert_eq!(<Priority as Scale>::ALL, [Low, Medium, High, Urgent]);
    assert_eq!(Priority::ALL, <Priority as Scale>::ALL);
    assert!(Urgent > High && High > Medium && Medium > Low);
    for (&level, name) in Priority::ALL
        .iter()
        .zip(["low", "medium", "high", "urgent"])
    {
        assert_eq!(level.name(), name);
        assert_eq!(<Priority as Scale>::parse(name), Some(level));
        assert_eq!(level.to_string(), name);
        let json = serde_json::to_string(&level).unwrap();
        assert_eq!(json, format!("\"{name}\""));
        assert_eq!(serde_json::from_str::<Priority>(&json).unwrap(), level);
    }
    let span = ScaleRange::<Priority>::parse("medium-high").unwrap();
    assert_eq!((span.lo(), span.hi()), (Medium, High));
    assert_eq!(
        serde_json::to_string(&PrioritySource::Unreadable).unwrap(),
        "\"unreadable\""
    );
}

#[test]
fn own_level_maps_inherits_and_falls_back() {
    let map = priority_map(vec![
        option("P0", "urgent", None),
        option("P1", "high", None),
        option("P2", "low-medium", None),
    ]);
    let parent = Some(Priority::Urgent);
    assert_eq!(
        own_level(&map, &FieldRead::Value(value("P1")), parent),
        (Priority::High, PrioritySource::Tracker)
    );
    assert_eq!(
        own_level(&map, &FieldRead::Value(value("P1")), None),
        (Priority::High, PrioritySource::Tracker)
    );
    assert_eq!(
        own_level(&map, &FieldRead::Unset, parent),
        (Priority::Urgent, PrioritySource::Parent)
    );
    assert_eq!(
        own_level(&map, &FieldRead::Unset, None),
        (Priority::Medium, PrioritySource::Unset)
    );
    assert_eq!(
        own_level(&map, &FieldRead::Value(value("P9")), parent),
        (Priority::Medium, PrioritySource::Default)
    );
    assert_eq!(
        own_level(
            &map,
            &FieldRead::Conflict(vec![value("P0"), value("P1")]),
            parent
        ),
        (Priority::Medium, PrioritySource::Default)
    );
}

#[test]
fn spanning_option_reads_as_upper_level() {
    let upper = priority_map(vec![option("Soon", "medium-high", None)]);
    assert_eq!(
        own_level(&upper, &FieldRead::Value(value("Soon")), None),
        (Priority::High, PrioritySource::Tracker)
    );
    let stated = priority_map(vec![option("Soon", "medium-high", Some("medium"))]);
    assert_eq!(
        own_level(&stated, &FieldRead::Value(value("Soon")), None),
        (Priority::Medium, PrioritySource::Tracker)
    );
}

#[test]
fn setup_preset_matches_level_names_ignoring_case() {
    let specs = setup_preset(["Urgent", "high", "MEDIUM", "Low", "P3"]);
    assert_eq!(
        specs,
        [
            option("Urgent", "urgent", None),
            option("high", "high", None),
            option("MEDIUM", "medium", None),
            option("Low", "low", None),
            option("P3", "", None),
        ]
    );
    let map = priority_map(specs[..4].to_vec());
    assert!(map.is_read_only());
    assert_eq!(map.option("MEDIUM").unwrap().reads_as, Priority::Medium);
    let refused = FieldMap::<Priority>::new(FieldMapSpec {
        source: SourceSpec::Labels {
            label: "priority:{option}".into(),
        },
        default: "medium".into(),
        options: specs,
    })
    .unwrap_err();
    assert!(
        matches!(&refused, FieldMapError::BadRange { option, .. } if option == "P3"),
        "{refused}"
    );
}

#[test]
fn shares_are_validated_and_strides_exact() {
    let state = ShareState::default();
    let shares: Vec<_> = Priority::ALL.iter().map(|&l| state.share(l)).collect();
    assert_eq!(shares, [1, 2, 4, 8]);
    for &level in Priority::ALL {
        assert_eq!(
            state.stride(level) * u64::from(state.share(level)),
            STRIDE_SCALE
        );
        assert_eq!(state.class(level), ClassState::default());
    }
    assert_eq!(ShareState::new([1, 2, 4, 8]).unwrap(), state);
    assert_eq!(
        ShareState::new([1, 0, 4, 8]),
        Err(ShareError::OutOfRange {
            level: Priority::Medium,
            share: 0
        })
    );
    assert_eq!(
        ShareState::new([1, 2, 4, 17]),
        Err(ShareError::OutOfRange {
            level: Priority::Urgent,
            share: 17
        })
    );
    assert!(ShareState::new([16, 16, 16, 16]).is_ok());
}

#[test]
fn stride_split_over_150_rounds() {
    let mut state = ShareState::default();
    let mut candidates = backlog(Priority::ALL, 200);
    let admitted: Vec<_> = (0..150)
        .map(|_| admit(&mut state, &mut candidates))
        .collect();
    let counts: Vec<_> = Priority::ALL
        .iter()
        .map(|level| admitted.iter().filter(|a| *a == level).count())
        .collect();
    assert_eq!(counts, [10, 20, 40, 80]);
    assert!(
        admitted
            .windows(15)
            .all(|window| window.contains(&Priority::Low)),
        "{admitted:?}"
    );
}

#[test]
fn zero_cost_counts_as_one() {
    let mut zero = ShareState::default();
    zero.charge(Priority::High, 0);
    let mut one = ShareState::default();
    one.charge(Priority::High, 1);
    assert_eq!(zero, one);
    assert_eq!(zero.class(Priority::High).pass, zero.stride(Priority::High));
}

#[test]
fn lone_urgent_goes_first() {
    let lone = [candidate("u", Priority::Urgent, 5)];
    let idle = |pass| ClassState {
        pass,
        active: false,
    };
    for idle_pass in [0, u64::MAX / 2] {
        for urgent in [
            ClassState {
                pass: 0,
                active: true,
            },
            ClassState {
                pass: u64::MAX / 4,
                active: true,
            },
            idle(0),
            idle(u64::MAX / 4),
        ] {
            let state = ShareState::from_parts(
                [1, 2, 4, 8],
                [idle(idle_pass), idle(idle_pass), idle(idle_pass), urgent],
            )
            .unwrap();
            assert_eq!(state.ranked(&lone, |_| 1), [JobId("u".into())]);
        }
    }
}

#[test]
fn reactivated_class_cannot_bank_credit() {
    let mut state = ShareState::default();
    let mut candidates = backlog(&[Priority::High, Priority::Urgent], 200);
    for _ in 0..100 {
        admit(&mut state, &mut candidates);
    }
    assert_eq!(state.class(Priority::Low).pass, 0);
    candidates.extend(backlog(&[Priority::Low], 50));

    let mut observed = state.clone();
    observed.observe(&candidates);
    let active_min = [Priority::High, Priority::Urgent]
        .iter()
        .map(|&level| state.class(level).pass)
        .min()
        .unwrap();
    assert_eq!(
        observed.class(Priority::Low),
        ClassState {
            pass: active_min,
            active: true
        }
    );

    let ranked = state.ranked(&candidates, |_| 1);
    let ranked_low = ranked[..13]
        .iter()
        .filter(|job| job.0.starts_with("low-"))
        .count();
    assert!(ranked_low <= 1, "{ranked:?}");

    let admitted: Vec<_> = (0..13)
        .map(|_| admit(&mut state, &mut candidates))
        .collect();
    let admitted_low = admitted.iter().filter(|&&l| l == Priority::Low).count();
    assert!(admitted_low <= 1, "{admitted:?}");
}

#[test]
fn ranked_is_deterministic_total_and_pure() {
    let mut state = ShareState::default();
    state.charge(Priority::Urgent, 3);
    state.charge(Priority::Medium, 1);
    let mut candidates = vec![
        candidate("b", Priority::High, 10),
        candidate("a", Priority::High, 10),
        candidate("c", Priority::High, 5),
        candidate("d", Priority::Urgent, 1),
        candidate("e", Priority::Low, 0),
        candidate("f", Priority::Medium, 3),
        candidate("g", Priority::Urgent, 0),
    ];
    let before = state.clone();
    let first = state.ranked(&candidates, |_| 1);
    let second = state.ranked(&candidates, |_| 1);
    candidates.reverse();
    let reversed = state.ranked(&candidates, |_| 1);
    assert_eq!(first, second);
    assert_eq!(first, reversed);
    assert_eq!(state, before);

    let mut sorted = first.clone();
    sorted.sort();
    sorted.dedup();
    let mut ids: Vec<_> = candidates.iter().map(|c| c.job.clone()).collect();
    ids.sort();
    assert_eq!(sorted, ids);
    assert_eq!(first.len(), candidates.len());

    let high: Vec<_> = first
        .iter()
        .filter(|job| ["a", "b", "c"].contains(&job.0.as_str()))
        .map(|job| job.0.as_str())
        .collect();
    assert_eq!(high, ["c", "a", "b"]);
    let urgent: Vec<_> = first
        .iter()
        .filter(|job| ["d", "g"].contains(&job.0.as_str()))
        .map(|job| job.0.as_str())
        .collect();
    assert_eq!(urgent, ["g", "d"]);
}

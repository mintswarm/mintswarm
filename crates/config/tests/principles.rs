//! `principles`: the principle packs the repository uses, how each is pinned and its overrides.
//! The variants are built from yq's JSON for the repository config snapshot.

#![cfg(test)]

use mintswarm_config::{Override, PackSource, Pin, RepoConfig, Strength, load_repo_config};
use serde_json::{Value, json};

const REPO_JSON: &str = include_str!("conformance/repo-config.json");

/// The snapshot config with `principles` set to `principles`.
fn with_principles(principles: Value) -> Value {
    let mut config: Value = serde_json::from_str(REPO_JSON).expect("repo-config.json is JSON");
    config
        .as_object_mut()
        .expect("the config is a mapping")
        .insert("principles".to_owned(), principles);
    config
}

fn load(config: Value) -> Result<RepoConfig, mintswarm_config::Error> {
    let text = serde_json::to_string_pretty(&config).expect("a JSON value serialises");
    load_repo_config(&text)
}

/// The error path for `principles` holding `sources: [source]`.
fn refused(source: Value) -> String {
    match load(with_principles(json!({ "sources": [source] }))) {
        Ok(_) => panic!("the config loads"),
        Err(error) => error.path().to_owned(),
    }
}

fn tenets() -> Value {
    json!({
        "pack": "@mintswarm/tenets:rust",
        "from": "git+https://github.com/mintswarm/tenets",
        "tag": "rust/v0.1.0",
    })
}

/// `tenets()` with `change` applied to its mapping.
fn source(change: impl FnOnce(&mut serde_json::Map<String, Value>)) -> Value {
    let mut source = tenets();
    change(source.as_object_mut().expect("the source is a mapping"));
    source
}

#[test]
fn absent_means_no_packs() {
    let mut config: Value = serde_json::from_str(REPO_JSON).expect("repo-config.json is JSON");
    config
        .as_object_mut()
        .expect("the config is a mapping")
        .remove("principles");
    assert!(
        load(config)
            .expect("the config loads")
            .principles()
            .is_empty()
    );
}

#[test]
fn sources_load_typed() {
    let config = with_principles(json!({ "sources": [
        source(|s| {
            s.insert("bias".to_owned(), json!(0.8));
            s.insert("parameters".to_owned(), json!({ "max_width": 100, "edition": "2024" }));
            s.insert("principles".to_owned(), json!({
                "clippy-clean": { "strength": "must" },
                "public-items-documented": { "strength": "may", "reason": "internal crates" },
                "api-guidelines": { "skip": true, "reason": "generated bindings" },
            }));
        }),
        {
            "pack": "@acme/packs:misra-c",
            "from": "git+ssh://git.acme.internal/packs.git",
            "hash": "0123456789abcdef0123456789abcdef01234567",
            "enabled": false,
        },
        { "pack": "@mintswarm/mintswarm:rust", "from": "repo:.mintswarm/principles/rust" },
    ]}));
    let loaded = load(config).expect("the config loads");
    let [rust, misra, local] = loaded.principles() else {
        panic!("three sources load");
    };
    assert_eq!(rust.pack, "@mintswarm/tenets:rust");
    assert_eq!(
        rust.source,
        PackSource::External {
            url: "git+https://github.com/mintswarm/tenets".to_owned(),
            pin: Pin::Tag("rust/v0.1.0".to_owned()),
        }
    );
    assert!(rust.enabled);
    assert_eq!(rust.bias, Some(0.8));
    assert_eq!(rust.parameters.get("max_width"), Some(&json!(100)));
    assert_eq!(
        rust.overrides.get("clippy-clean"),
        Some(&Override::Strength {
            strength: Strength::Must,
            reason: None
        })
    );
    assert_eq!(
        rust.overrides.get("public-items-documented"),
        Some(&Override::Strength {
            strength: Strength::May,
            reason: Some("internal crates".to_owned())
        })
    );
    assert_eq!(
        rust.overrides.get("api-guidelines"),
        Some(&Override::Skip {
            reason: "generated bindings".to_owned()
        })
    );
    assert!(!misra.enabled);
    assert!(matches!(
        &misra.source,
        PackSource::External {
            pin: Pin::Hash(_),
            ..
        }
    ));
    assert_eq!(
        local.source,
        PackSource::Local {
            path: ".mintswarm/principles/rust".to_owned()
        }
    );
}

#[test]
fn malformed_sources_are_refused_at_their_key() {
    let cases: Vec<(Value, &str)> = vec![
        (json!("rust"), "principles.sources[0]"),
        (
            source(|s| {
                s.remove("pack");
            }),
            "principles.sources[0].pack",
        ),
        (
            source(|s| {
                s.insert("pack".to_owned(), json!("mintswarm/tenets:rust"));
            }),
            "principles.sources[0].pack",
        ),
        (
            source(|s| {
                s.insert("from".to_owned(), json!("http://example.com/p.tgz"));
            }),
            "principles.sources[0].from",
        ),
        (
            source(|s| {
                s.insert("extra".to_owned(), json!(1));
            }),
            "principles.sources[0].extra",
        ),
        (
            source(|s| {
                s.remove("tag");
            }),
            "principles.sources[0]",
        ),
        (
            source(|s| {
                s.insert("hash".to_owned(), json!("a".repeat(40)));
            }),
            "principles.sources[0]",
        ),
        (
            source(|s| {
                s.remove("tag");
                s.insert("hash".to_owned(), json!("abc"));
            }),
            "principles.sources[0].hash",
        ),
        (
            source(|s| {
                s.insert("tag".to_owned(), json!(""));
            }),
            "principles.sources[0].tag",
        ),
        (
            json!({ "pack": "@mintswarm/mintswarm:rust", "from": "repo:.mintswarm/principles/rust", "tag": "v1" }),
            "principles.sources[0]",
        ),
        (
            json!({ "pack": "@mintswarm/mintswarm:rust", "from": "repo:../elsewhere" }),
            "principles.sources[0].from",
        ),
        (
            json!({ "pack": "@mintswarm/mintswarm:rust", "from": "repo:" }),
            "principles.sources[0].from",
        ),
        (
            source(|s| {
                s.insert("enabled".to_owned(), json!("yes"));
            }),
            "principles.sources[0].enabled",
        ),
        (
            source(|s| {
                s.insert("bias".to_owned(), json!(1.5));
            }),
            "principles.sources[0].bias",
        ),
        (
            source(|s| {
                s.insert("parameters".to_owned(), json!({ "Max": 1 }));
            }),
            "principles.sources[0].parameters.Max",
        ),
        (
            source(|s| {
                s.insert("parameters".to_owned(), json!({ "max_width": [100] }));
            }),
            "principles.sources[0].parameters.max_width",
        ),
    ];
    for (source, path) in cases {
        assert_eq!(refused(source.clone()), path, "{source}");
    }
}

#[test]
fn malformed_overrides_are_refused_at_their_key() {
    let with = |overrides: Value| {
        source(|s| {
            s.insert("principles".to_owned(), overrides);
        })
    };
    let cases: Vec<(Value, &str)> = vec![
        (
            json!({ "Clippy": { "strength": "must" } }),
            "principles.sources[0].principles.Clippy",
        ),
        (
            json!({ "clippy-clean": { "strength": "always" } }),
            "principles.sources[0].principles.clippy-clean.strength",
        ),
        (
            json!({ "clippy-clean": {} }),
            "principles.sources[0].principles.clippy-clean",
        ),
        (
            json!({ "clippy-clean": { "skip": true } }),
            "principles.sources[0].principles.clippy-clean.reason",
        ),
        (
            json!({ "clippy-clean": { "skip": false, "reason": "x" } }),
            "principles.sources[0].principles.clippy-clean.skip",
        ),
        (
            json!({ "clippy-clean": { "skip": true, "strength": "may", "reason": "x" } }),
            "principles.sources[0].principles.clippy-clean",
        ),
        (
            json!({ "clippy-clean": { "strength": "may", "reason": "" } }),
            "principles.sources[0].principles.clippy-clean.reason",
        ),
        (
            json!({ "clippy-clean": { "strength": "must", "why": "x" } }),
            "principles.sources[0].principles.clippy-clean.why",
        ),
    ];
    for (overrides, path) in cases {
        assert_eq!(refused(with(overrides.clone())), path, "{overrides}");
    }
}

#[test]
fn a_pack_is_listed_once() {
    let config = with_principles(json!({ "sources": [tenets(), tenets()] }));
    let Err(error) = load(config) else {
        panic!("the config loads");
    };
    assert_eq!(error.path(), "principles.sources[1].pack");
}

#[test]
fn principles_holds_only_sources() {
    let Err(error) = load(with_principles(json!({ "packs": [] }))) else {
        panic!("the config loads");
    };
    assert_eq!(error.path(), "principles.packs");
}

//! The migration tree's check on in-memory fixture trees, the embedded list, and minting.

#![cfg(test)]

use std::fs;
use std::path::{Path, PathBuf};

use mintswarm_core::migrations::{
    Down, MintError, TreeError, Warning, embedded, embedded_tree, is_valid_version, mint,
    read_tree, validate, version_of,
};

const DIR: &str = "mintswarm/migrations";

fn path(file_name: &str) -> String {
    format!("{DIR}/{file_name}")
}

fn check(files: &[(&str, &str)]) -> Result<mintswarm_core::migrations::Report, Vec<TreeError>> {
    let pairs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(file, text)| (*file, text.as_bytes()))
        .collect();
    validate(&pairs)
}

fn errors(files: &[(&str, &str)]) -> Vec<TreeError> {
    check(files).expect_err("the tree is refused")
}

#[test]
fn a_down_file_without_its_up_is_refused() {
    let down = path("20260101000000_a.down.sql");
    assert_eq!(
        errors(&[(&down, "")]),
        vec![TreeError::DownWithoutUp { file: down }]
    );
}

#[test]
fn a_down_file_with_another_name_than_its_up_is_refused() {
    let up = path("20260101000000_a.up.sql");
    let down = path("20260101000000_b.down.sql");
    assert_eq!(
        errors(&[(&up, ""), (&down, "")]),
        vec![TreeError::DownWithoutUp { file: down }]
    );
}

#[test]
fn a_name_with_another_character_is_refused() {
    for file_name in [
        "20260101000000_bad-name.up.sql",
        "20260101000000_bad.name.up.sql",
        "20260101000000_.up.sql",
        "20260101000000_späť.up.sql",
    ] {
        let file = path(file_name);
        assert_eq!(
            errors(&[(&file, "")]),
            vec![TreeError::BadName { file: file.clone() }],
            "{file}"
        );
    }
}

#[test]
fn a_version_that_is_not_14_digits_of_a_real_second_is_refused() {
    for version in [
        "2026010100000",
        "202601010000000",
        "2026010100000x",
        "20261301000000",
        "20250229000000",
        "20260431000000",
        "20260101240000",
        "20260101006000",
        "20260101000060",
        "20260100000000",
    ] {
        let file = path(&format!("{version}_a.up.sql"));
        assert_eq!(
            errors(&[(&file, "")]),
            vec![TreeError::BadVersion { file: file.clone() }],
            "{file}"
        );
    }
}

#[test]
fn leap_days_are_real_dates() {
    assert!(is_valid_version("20240229235959"));
    assert!(is_valid_version("20000229000000"));
    assert!(!is_valid_version("19000229000000"));
}

#[test]
fn two_files_with_one_version_are_refused() {
    let first = path("20260101000000_a.up.sql");
    let second = path("20260101000000_b.up.sql");
    assert_eq!(
        errors(&[(&second, ""), (&first, "")]),
        vec![TreeError::DuplicateVersion { file: second }]
    );
}

#[test]
fn a_file_name_without_a_separator_is_refused() {
    let file = path("20260101000000.up.sql");
    assert_eq!(
        errors(&[(&file, "")]),
        vec![TreeError::BadFileName { file }]
    );
}

#[test]
fn paths_outside_the_layout_are_refused() {
    for file in [
        "other/migrations/20260101000000_a.up.sql",
        "mintswarm/backfills/20260101000000_a.sql",
        "mintswarm/20260101000000_a.up.sql",
        "mintswarm/migrations/README.md",
        "mintswarm/migrations/20260101000000_a.sql",
        "mintswarm/migrations/nested/20260101000000_a.up.sql",
        ".DS_Store",
    ] {
        assert_eq!(
            errors(&[(file, "")]),
            vec![TreeError::UnexpectedPath {
                file: file.to_owned()
            }],
            "{file}"
        );
    }
}

#[test]
fn a_file_that_is_not_utf8_is_refused() {
    let file = path("20260101000000_a.up.sql");
    let bytes: &[u8] = &[0xff, 0xfe];
    assert_eq!(
        validate(&[(&file, bytes)]),
        Err(vec![TreeError::NotUtf8 { file }])
    );
}

#[test]
fn every_error_is_given_at_once() {
    let bad_name = path("20260101000000_bad-name.up.sql");
    let orphan = path("20260102000000_a.down.sql");
    assert_eq!(
        errors(&[(&orphan, ""), (&bad_name, "")]),
        vec![
            TreeError::BadName { file: bad_name },
            TreeError::DownWithoutUp { file: orphan },
        ]
    );
}

#[test]
fn a_migration_without_a_down_gives_exactly_one_warning() {
    let up = path("20260101000000_a.up.sql");
    let report = check(&[(&up, "CREATE TABLE a ();\n")]).expect("the tree is valid");
    assert_eq!(report.warnings, vec![Warning::MissingDown { file: up }]);
    assert_eq!(report.migrations.len(), 1);
}

#[test]
fn a_migration_with_its_down_gives_no_warning() {
    let up = path("20260101000000_a.up.sql");
    let down = path("20260101000000_a.down.sql");
    let report = check(&[(&down, "DROP TABLE a;\n"), (&up, "CREATE TABLE a ();\n")])
        .expect("the tree is valid");
    assert!(report.warnings.is_empty());
    let [migration] = report.migrations.as_slice() else {
        panic!("one migration: {:?}", report.migrations);
    };
    assert_eq!(migration.schema, "mintswarm");
    assert_eq!(migration.version, "20260101000000");
    assert_eq!(migration.name, "a");
    assert_eq!(migration.up, up);
    assert_eq!(
        migration.down,
        Some(Down {
            file: down,
            no_transaction: false
        })
    );
}

#[test]
fn migrations_are_sorted_by_version() {
    let later = path("20260102000000_z.up.sql");
    let earlier = path("20260101000000_y.up.sql");
    let report = check(&[(&later, ""), (&earlier, "")]).expect("the tree is valid");
    let ups: Vec<&str> = report.migrations.iter().map(|m| m.up.as_str()).collect();
    assert_eq!(ups, vec![earlier.as_str(), later.as_str()]);
}

#[test]
fn a_header_key_below_the_first_statement_is_refused() {
    let file = path("20260101000000_a.up.sql");
    let text = "-- A table.\n\nCREATE TABLE a ();\n  -- migrate: no-transaction\n";
    assert_eq!(
        errors(&[(&file, text)]),
        vec![TreeError::KeyBelowStatement {
            file,
            key: "no-transaction".to_owned()
        }]
    );
}

#[test]
fn a_header_key_below_the_first_statement_of_a_down_file_is_refused() {
    let up = path("20260101000000_a.up.sql");
    let down = path("20260101000000_a.down.sql");
    let text = "DROP TABLE a;\n-- migrate: depends-on 20250101000000_b\n";
    assert_eq!(
        errors(&[(&up, ""), (&down, text)]),
        vec![TreeError::KeyBelowStatement {
            file: down,
            key: "depends-on".to_owned()
        }]
    );
}

#[test]
fn keys_not_implemented_yet_are_refused_on_an_up_file() {
    for (key, line) in [
        (
            "depends-on",
            "-- migrate: depends-on mintswarm/20250101000000_b",
        ),
        (
            "irreversible-enum",
            "-- migrate: irreversible-enum mintswarm.kind.new",
        ),
        ("code-floor", "-- migrate: code-floor 450"),
        ("replaces", "-- migrate: replaces 20250101000000"),
        ("pile-hash", "-- migrate: pile-hash abc"),
        ("pile-commit", "-- migrate: pile-commit abc"),
        ("declared-difference", "-- migrate: declared-difference x"),
        ("no-transaction", "-- migrate: no-transaction"),
    ] {
        let file = path("20260101000000_a.up.sql");
        let text = format!("-- A table.\n{line}\nCREATE TABLE a ();\n");
        assert_eq!(
            errors(&[(&file, &text)]),
            vec![TreeError::Unsupported {
                file,
                key: key.to_owned()
            }],
            "{key}"
        );
    }
}

#[test]
fn a_down_file_carrying_another_key_than_no_transaction_is_refused() {
    let up = path("20260101000000_a.up.sql");
    let down = path("20260101000000_a.down.sql");
    let text = "-- migrate: depends-on mintswarm/20250101000000_b\nDROP TABLE a;\n";
    assert_eq!(
        errors(&[(&up, ""), (&down, text)]),
        vec![TreeError::NotOnDown {
            file: down,
            key: "depends-on".to_owned()
        }]
    );
}

#[test]
fn a_down_file_may_run_outside_a_transaction() {
    let up = path("20260101000000_a.up.sql");
    let down = path("20260101000000_a.down.sql");
    let text = "-- Concurrent.\n-- migrate: no-transaction\n\nDROP INDEX CONCURRENTLY a_idx;\n";
    let report = check(&[(&up, ""), (&down, text)]).expect("the tree is valid");
    let [migration] = report.migrations.as_slice() else {
        panic!("one migration: {:?}", report.migrations);
    };
    assert_eq!(
        migration.down,
        Some(Down {
            file: down,
            no_transaction: true
        })
    );
}

#[test]
fn an_unknown_key_is_refused() {
    let file = path("20260101000000_a.up.sql");
    assert_eq!(
        errors(&[(&file, "-- migrate: no-transactions\n")]),
        vec![TreeError::UnknownKey {
            file,
            key: "no-transactions".to_owned()
        }]
    );
}

#[test]
fn a_malformed_header_is_refused() {
    let up = path("20260101000000_a.up.sql");
    let down = path("20260101000000_a.down.sql");
    for (text, line) in [
        ("-- migrate:\n", 1),
        ("--migrate: no-transaction\n", 1),
        ("--  migrate: no-transaction\n", 1),
        ("-- Note.\n-- migrate: no-transaction yes\n", 2),
        (
            "-- migrate: no-transaction\n-- migrate: no-transaction\n",
            2,
        ),
    ] {
        assert_eq!(
            errors(&[(&up, ""), (&down, text)]),
            vec![TreeError::MalformedHeader {
                file: down.clone(),
                line
            }],
            "{text:?}"
        );
    }
}

#[test]
fn ordinary_comments_are_not_headers() {
    let up = path("20260101000000_a.up.sql");
    let text =
        "-- The migrate step creates a.\n-- migrate it later: no.\nCREATE TABLE a ();\n-- done\n";
    assert!(check(&[(&up, text)]).is_ok());
}

#[test]
fn a_missing_tree_reads_as_empty() {
    let root = scratch("a_missing_tree_reads_as_empty").join("absent");
    assert_eq!(read_tree(&root), Ok(Vec::new()));
}

#[test]
fn a_tree_reads_sorted_with_relative_paths() {
    let root = scratch("a_tree_reads_sorted_with_relative_paths");
    let dir = root.join("mintswarm/migrations");
    fs::create_dir_all(&dir).expect("the tree folder is created");
    fs::write(dir.join("20260102000000_b.up.sql"), "B").expect("a file is written");
    fs::write(dir.join("20260101000000_a.up.sql"), "A").expect("a file is written");
    assert_eq!(
        read_tree(&root),
        Ok(vec![
            (path("20260101000000_a.up.sql"), b"A".to_vec()),
            (path("20260102000000_b.up.sql"), b"B".to_vec()),
        ])
    );
}

#[test]
fn embedded_list_matches_the_directory() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let on_disk = read_tree(&root).expect("the tree reads");
    let listed: Vec<(String, Vec<u8>)> = embedded()
        .iter()
        .map(|(file, bytes)| ((*file).to_owned(), bytes.to_vec()))
        .collect();
    println!("embedded entries: {}", listed.len());
    assert_eq!(listed, on_disk);
    if !root.exists() {
        assert!(embedded().is_empty(), "an absent tree embeds nothing");
    }
    assert!(embedded_tree().is_ok(), "the build refuses an invalid tree");
}

#[test]
fn versions_format_known_seconds() {
    assert_eq!(version_of(0), "19700101000000");
    assert_eq!(version_of(951_782_400), "20000229000000");
    assert_eq!(version_of(1_795_689_045), "20261126103045");
    assert_eq!(version_of(86_399), "19700101235959");
}

/// A fresh folder for one test under Cargo's per-target temporary folder, which is under
/// `build/`.
fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("core-migrations")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("the scratch folder is created");
    dir
}

/// A fixed clock: 2026-11-26 10:30:45 UTC.
const NOW: u64 = 1_795_689_045;

fn minted_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .expect("a minted file has a UTF-8 name")
        .to_owned()
}

#[test]
fn mint_names_the_file_for_a_free_second() {
    let root = scratch("mint_names_the_file_for_a_free_second");
    let created = mint(&root, "add_jobs", NOW).expect("the file is minted");
    assert_eq!(
        created.parent(),
        Some(root.join("mintswarm/migrations").as_path())
    );
    assert_eq!(
        minted_name(&created),
        format!("{}_add_jobs.up.sql", version_of(NOW))
    );
    assert_eq!(fs::read(&created).expect("the file reads"), b"");
}

#[test]
fn mint_moves_to_the_next_free_second() {
    let root = scratch("mint_moves_to_the_next_free_second");
    let first = mint(&root, "a", NOW).expect("the first file is minted");
    let second = mint(&root, "b", NOW).expect("the second file is minted");
    assert_eq!(minted_name(&first), format!("{}_a.up.sql", version_of(NOW)));
    assert_eq!(
        minted_name(&second),
        format!("{}_b.up.sql", version_of(NOW + 1))
    );
}

#[test]
fn mint_skips_every_taken_second() {
    let root = scratch("mint_skips_every_taken_second");
    let dir = root.join("mintswarm/migrations");
    fs::create_dir_all(&dir).expect("the tree folder is created");
    for (offset, file) in [(0, "x.up.sql"), (1, "y.down.sql"), (2, "z.up.sql")] {
        fs::write(dir.join(format!("{}_{file}", version_of(NOW + offset))), "")
            .expect("a taken file is written");
    }
    let created = mint(&root, "next", NOW).expect("the file is minted");
    assert_eq!(
        minted_name(&created),
        format!("{}_next.up.sql", version_of(NOW + 3))
    );
}

#[test]
fn mint_never_overwrites_a_file() {
    let root = scratch("mint_never_overwrites_a_file");
    let dir = root.join("mintswarm/migrations");
    fs::create_dir_all(&dir).expect("the tree folder is created");
    let existing = dir.join(format!("{}_a.up.sql", version_of(NOW)));
    fs::write(&existing, "CREATE TABLE a ();\n").expect("the existing file is written");
    let created = mint(&root, "a", NOW).expect("the file is minted");
    assert_ne!(created, existing);
    assert_eq!(
        fs::read(&existing).expect("the existing file reads"),
        b"CREATE TABLE a ();\n"
    );
}

#[test]
fn mint_refuses_an_invalid_name_and_creates_nothing() {
    let root = scratch("mint_refuses_an_invalid_name_and_creates_nothing");
    for name in ["bad-name", "", "a b", "../a"] {
        assert_eq!(
            mint(&root, name, NOW),
            Err(MintError::InvalidName {
                name: name.to_owned()
            })
        );
    }
    assert!(!root.join("mintswarm").exists(), "nothing is created");
}

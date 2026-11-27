//! The lexical SQL reading, the narrowing detector and the interim rule, on in-memory SQL.

#![cfg(test)]

use mintswarm_core::migrations::narrowing::{
    Advisory, Name, Narrowing, Object, Reading, Refusal, check, read,
};
use mintswarm_core::migrations::sql::{Kind, LexError, Statement, Token, classify, split};

const FIRST: &str = "mintswarm/migrations/20260101000000_first.up.sql";
const SECOND: &str = "mintswarm/migrations/20260102000000_second.up.sql";

fn statements(text: &str) -> Vec<Statement> {
    split(text).expect("the text splits")
}

fn one(text: &str) -> Statement {
    let mut statements = statements(text);
    assert_eq!(statements.len(), 1, "one statement in {text:?}");
    statements.remove(0)
}

fn reading(text: &str) -> Reading {
    read(&one(text))
}

fn narrowings(text: &str) -> Vec<(Narrowing, Object)> {
    reading(text).narrowings
}

fn name(name: &str) -> Name {
    Name::unqualified(name)
}

fn relation(table: &str) -> Object {
    Object::Relation(name(table))
}

fn column(table: &str, column: &str) -> Object {
    Object::Column(name(table), column.to_owned())
}

fn lex_error(text: &str) -> LexError {
    split(text).expect_err("the text is refused")
}

// The lexer.

#[test]
fn statements_split_on_semicolons_outside_everything_else() {
    let all = statements(
        "-- migrate: no-transaction\nCREATE TABLE a (x text DEFAULT ';');\n;\n\
         /* ; */ SELECT \";\" FROM a; DO $$ BEGIN; END $$;\nSELECT 1",
    );
    let texts: Vec<&str> = all
        .iter()
        .map(|statement| statement.text.as_str())
        .collect();
    assert_eq!(
        texts,
        [
            "CREATE TABLE a (x text DEFAULT ';')",
            "SELECT \";\" FROM a",
            "DO $$ BEGIN; END $$",
            "SELECT 1",
        ]
    );
    let lines: Vec<usize> = all.iter().map(|statement| statement.line).collect();
    assert_eq!(lines, [2, 4, 4, 5]);
}

#[test]
fn the_display_text_drops_comments_and_collapses_whitespace() {
    let statement = one("ALTER   TABLE\n\tjobs -- trailing\n  DROP /* inner */ COLUMN x");
    assert_eq!(statement.text, "ALTER TABLE jobs DROP COLUMN x");
    assert_eq!(statement.line, 1);
}

#[test]
fn block_comments_nest() {
    let statement = one("/* outer /* inner */ DROP TABLE a; */ SELECT 1");
    assert_eq!(classify(&statement), Kind::Data);
    assert_eq!(
        lex_error("SELECT 1; /* outer /* inner */ still open"),
        LexError::UnterminatedComment { line: 1 }
    );
}

#[test]
fn literal_forms_become_one_blanked_token() {
    for text in [
        "SELECT 'it''s'",
        "SELECT E'it\\'s'",
        "SELECT e'\\\\'",
        "SELECT U&'d\\0061t'",
        "SELECT B'1010'",
        "SELECT X'1F'",
        "SELECT N'x'",
    ] {
        assert_eq!(
            one(text).tokens,
            [Token::Word("select".to_owned()), Token::Literal],
            "{text}"
        );
    }
}

#[test]
fn quoted_identifiers_are_kept_as_written() {
    assert_eq!(
        one("SELECT \"My \"\"Table\"\"\", U&\"x\"").tokens,
        [
            Token::Word("select".to_owned()),
            Token::Quoted("My \"Table\"".to_owned()),
            Token::Symbol(','),
            Token::Quoted("x".to_owned()),
        ]
    );
}

#[test]
fn unquoted_words_fold_to_lower_case_and_keep_dollars() {
    assert_eq!(
        one("SeLeCt a$b").tokens,
        [
            Token::Word("select".to_owned()),
            Token::Word("a$b".to_owned())
        ]
    );
}

#[test]
fn a_positional_parameter_is_not_a_dollar_quote() {
    assert_eq!(
        one("SELECT $1, $2").tokens,
        [
            Token::Word("select".to_owned()),
            Token::Param("1".to_owned()),
            Token::Symbol(','),
            Token::Param("2".to_owned()),
        ]
    );
    assert_eq!(
        one("SELECT $fn$ ; $1 $$ $fn$").tokens,
        [Token::Word("select".to_owned()), Token::DollarBody]
    );
}

#[test]
fn unterminated_forms_are_refused_with_their_line() {
    assert_eq!(
        lex_error("SELECT 1;\nSELECT 'open"),
        LexError::UnterminatedLiteral { line: 2 }
    );
    assert_eq!(
        lex_error("SELECT E'open\\'"),
        LexError::UnterminatedLiteral { line: 1 }
    );
    assert_eq!(
        lex_error("\n\nSELECT \"open"),
        LexError::UnterminatedQuotedIdentifier { line: 3 }
    );
    assert_eq!(
        lex_error("DO $body$ BEGIN END $other$"),
        LexError::UnterminatedDollarQuote { line: 1 }
    );
    assert_eq!(
        lex_error("/* open"),
        LexError::UnterminatedComment { line: 1 }
    );
}

#[test]
fn a_begin_atomic_body_is_refused() {
    let error = lex_error(
        "CREATE FUNCTION f() RETURNS int LANGUAGE sql\nBEGIN /* gap */ ATOMIC SELECT 1; END",
    );
    assert_eq!(error, LexError::AtomicBody { line: 2 });
    assert_eq!(error.line(), 2);
    assert!(split("BEGIN; SELECT atomic FROM t").is_ok());
}

// The classifier.

#[test]
fn each_statement_is_classified() {
    for text in [
        "INSERT INTO t VALUES (1)",
        "UPDATE t SET x = 1",
        "DELETE FROM t",
        "MERGE INTO t USING s ON true WHEN MATCHED THEN DELETE",
        "COPY t FROM STDIN",
        "SELECT 1",
        "WITH a AS (SELECT 1) SELECT * FROM a",
        "DO $$ BEGIN END $$",
        "CALL p()",
    ] {
        assert_eq!(classify(&one(text)), Kind::Data, "{text}");
    }
    for text in [
        "GRANT SELECT ON t TO r",
        "REVOKE SELECT ON t FROM r",
        "ALTER DEFAULT PRIVILEGES GRANT SELECT ON TABLES TO r",
    ] {
        assert_eq!(classify(&one(text)), Kind::GrantOrRevoke, "{text}");
    }
    for text in [
        "CREATE TABLE t (x int)",
        "ALTER TABLE t ADD COLUMN y int",
        "DROP TABLE t",
        "TRUNCATE t",
        "SET lock_timeout = 0",
        "FROBNICATE t",
        "ALTER DEFAULT x",
    ] {
        assert_eq!(classify(&one(text)), Kind::Ddl, "{text}");
    }
}

// The detector.

#[test]
fn each_narrowing_form_is_reported_with_its_object() {
    use Narrowing::*;
    let ty = |name: &str| Object::Type(Name::unqualified(name));
    let function = |name: &str| Object::Function(Name::unqualified(name));
    let cases: Vec<(&str, Vec<(Narrowing, Object)>)> = vec![
        ("DROP TABLE jobs", vec![(Drop, relation("jobs"))]),
        (
            "DROP TABLE IF EXISTS a, other.b CASCADE",
            vec![
                (Drop, relation("a")),
                (Drop, Object::Relation(Name::new("other", "b"))),
            ],
        ),
        ("DROP VIEW v", vec![(Drop, relation("v"))]),
        ("DROP MATERIALIZED VIEW m", vec![(Drop, relation("m"))]),
        ("DROP SEQUENCE s", vec![(Drop, relation("s"))]),
        ("DROP TYPE t", vec![(Drop, ty("t"))]),
        ("DROP DOMAIN d", vec![(Drop, ty("d"))]),
        (
            "DROP FUNCTION f(int, text), g",
            vec![(Drop, function("f")), (Drop, function("g"))],
        ),
        ("DROP PROCEDURE p()", vec![(Drop, function("p"))]),
        (
            "DROP SCHEMA IF EXISTS s RESTRICT",
            vec![(Drop, Object::Schema("s".to_owned()))],
        ),
        (
            "ALTER TABLE jobs DROP COLUMN IF EXISTS x",
            vec![(Drop, column("jobs", "x"))],
        ),
        ("ALTER TABLE jobs DROP x", vec![(Drop, column("jobs", "x"))]),
        (
            "ALTER TABLE IF EXISTS ONLY jobs ALTER COLUMN x SET NOT NULL",
            vec![(SetNotNull, column("jobs", "x"))],
        ),
        (
            "ALTER TABLE jobs ALTER x TYPE bigint",
            vec![(ColumnType, column("jobs", "x"))],
        ),
        (
            "ALTER TABLE jobs ALTER COLUMN x SET DATA TYPE bigint USING x::bigint",
            vec![(ColumnType, column("jobs", "x"))],
        ),
        (
            "ALTER TABLE jobs ADD COLUMN y int NOT NULL",
            vec![(NotNullColumnWithoutDefault, column("jobs", "y"))],
        ),
        (
            "ALTER TABLE jobs ADD y int UNIQUE",
            vec![(UniqueConstraint, relation("jobs"))],
        ),
        (
            "ALTER TABLE jobs ADD COLUMN y int PRIMARY KEY",
            vec![(UniqueConstraint, relation("jobs"))],
        ),
        (
            "ALTER TABLE jobs ADD CONSTRAINT u UNIQUE (x)",
            vec![(UniqueConstraint, relation("jobs"))],
        ),
        (
            "ALTER TABLE jobs ADD PRIMARY KEY (x)",
            vec![(UniqueConstraint, relation("jobs"))],
        ),
        (
            "ALTER TABLE jobs ADD EXCLUDE USING gist (r WITH &&)",
            vec![(UniqueConstraint, relation("jobs"))],
        ),
        (
            "ALTER TABLE jobs RENAME TO tasks",
            vec![(Rename, relation("jobs"))],
        ),
        (
            "ALTER TABLE jobs RENAME COLUMN x TO y",
            vec![(Rename, column("jobs", "x"))],
        ),
        (
            "ALTER TABLE jobs RENAME x TO y",
            vec![(Rename, column("jobs", "x"))],
        ),
        (
            "ALTER TABLE jobs RENAME CONSTRAINT c TO d",
            vec![(Rename, Object::Constraint(name("jobs"), "c".to_owned()))],
        ),
        (
            "ALTER TABLE jobs SET SCHEMA other",
            vec![(SetSchema, relation("jobs"))],
        ),
        ("ALTER VIEW v RENAME TO w", vec![(Rename, relation("v"))]),
        (
            "ALTER MATERIALIZED VIEW IF EXISTS m RENAME COLUMN a TO b",
            vec![(Rename, column("m", "a"))],
        ),
        (
            "ALTER SEQUENCE s SET SCHEMA other",
            vec![(SetSchema, relation("s"))],
        ),
        ("ALTER TYPE t RENAME TO u", vec![(Rename, ty("t"))]),
        ("ALTER TYPE t SET SCHEMA other", vec![(SetSchema, ty("t"))]),
        (
            "ALTER TYPE mood RENAME VALUE 'sad' TO 'blue'",
            vec![(EnumValueRename, ty("mood"))],
        ),
        (
            "ALTER TYPE pair ADD ATTRIBUTE c int, DROP ATTRIBUTE IF EXISTS d",
            vec![(AttributeChange, ty("pair")), (AttributeChange, ty("pair"))],
        ),
        (
            "ALTER TYPE pair ALTER ATTRIBUTE a TYPE bigint",
            vec![(AttributeChange, ty("pair"))],
        ),
        (
            "ALTER TYPE pair RENAME ATTRIBUTE a TO b",
            vec![(AttributeChange, ty("pair"))],
        ),
        (
            "ALTER DOMAIN d ADD CONSTRAINT positive CHECK (VALUE > 0)",
            vec![(DomainConstraint, ty("d"))],
        ),
        (
            "ALTER DOMAIN d SET NOT NULL",
            vec![(DomainConstraint, ty("d"))],
        ),
        (
            "ALTER DOMAIN d RENAME CONSTRAINT a TO b",
            vec![(Rename, Object::Constraint(name("d"), "a".to_owned()))],
        ),
        ("ALTER DOMAIN d RENAME TO e", vec![(Rename, ty("d"))]),
        (
            "ALTER DOMAIN d SET SCHEMA other",
            vec![(SetSchema, ty("d"))],
        ),
        (
            "ALTER FUNCTION f(int) RENAME TO g",
            vec![(Rename, function("f"))],
        ),
        (
            "ALTER PROCEDURE p SET SCHEMA other",
            vec![(SetSchema, function("p"))],
        ),
        (
            "ALTER SCHEMA s RENAME TO t",
            vec![(Rename, Object::Schema("s".to_owned()))],
        ),
        (
            "CREATE UNIQUE INDEX jobs_x ON jobs (x)",
            vec![(UniqueIndex, relation("jobs"))],
        ),
        (
            "CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS ON ONLY other.jobs (x)",
            vec![(UniqueIndex, Object::Relation(Name::new("other", "jobs")))],
        ),
        (
            "ALTER TABLE jobs ADD COLUMN y int, ALTER COLUMN x SET NOT NULL, DROP COLUMN z",
            vec![
                (SetNotNull, column("jobs", "x")),
                (Drop, column("jobs", "z")),
            ],
        ),
        (
            "ALTER TABLE \"Jobs\" DROP COLUMN \"X\"",
            vec![(
                Drop,
                Object::Column(Name::unqualified("Jobs"), "X".to_owned()),
            )],
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(narrowings(text), expected, "{text}");
    }
}

#[test]
fn not_narrowing_forms_report_nothing() {
    for text in [
        "ALTER TYPE mood ADD VALUE 'happy'",
        "ALTER TABLE jobs DROP CONSTRAINT c",
        "DROP INDEX jobs_x",
        "DROP TRIGGER t ON jobs",
        "ALTER TABLE jobs ALTER COLUMN x DROP NOT NULL",
        "ALTER TABLE jobs ALTER COLUMN x SET DEFAULT 0",
        "ALTER TABLE jobs ALTER COLUMN x DROP DEFAULT",
        "ALTER TABLE jobs ALTER CONSTRAINT c DEFERRABLE",
        "ALTER TABLE jobs SET (fillfactor = 70)",
        "GRANT SELECT ON jobs TO r",
        "REVOKE SELECT ON jobs FROM r",
        "COMMENT ON TABLE jobs IS 'DROP TABLE jobs'",
        "ALTER TRIGGER t ON jobs RENAME TO u",
        "ALTER TABLE jobs ADD COLUMN y int",
        "ALTER TABLE jobs ADD COLUMN y int NOT NULL DEFAULT 0",
        "ALTER TABLE jobs ADD COLUMN y bigint NOT NULL GENERATED ALWAYS AS IDENTITY",
        "ALTER TABLE jobs ADD COLUMN y int CHECK (y IS NOT NULL)",
        "CREATE INDEX jobs_x ON jobs (x)",
        "ALTER DOMAIN d DROP CONSTRAINT c",
        "ALTER DOMAIN d DROP NOT NULL",
        "ALTER DOMAIN d SET DEFAULT 0",
        "ALTER DOMAIN d VALIDATE CONSTRAINT c",
        "CREATE TABLE jobs (x int NOT NULL UNIQUE)",
        "UPDATE jobs SET x = 1",
        "DELETE FROM jobs",
    ] {
        assert_eq!(narrowings(text), [], "{text}");
    }
}

#[test]
fn narrowing_inside_comments_literals_and_dollar_bodies_is_not_reported() {
    for text in [
        "SELECT 1 -- ; DROP TABLE jobs\n",
        "SELECT 1 /* a /* ; DROP TABLE jobs */ ; ALTER TABLE jobs DROP COLUMN x */",
        "SELECT '; DROP TABLE jobs'",
        "SELECT E'\\'; DROP TABLE jobs'",
        "SELECT $$; DROP TABLE jobs$$",
        "CREATE FUNCTION f() RETURNS void LANGUAGE sql AS $fn$ ; DROP TABLE jobs $fn$",
        "DO $$ BEGIN DROP TABLE jobs; ALTER TABLE jobs DROP COLUMN x; END $$",
    ] {
        for statement in statements(text) {
            assert_eq!(read(&statement).narrowings, [], "{text}");
        }
    }
}

#[test]
fn check_and_foreign_key_are_advisory() {
    for (text, advisory) in [
        (
            "ALTER TABLE jobs ADD CONSTRAINT positive CHECK (x > 0)",
            Advisory::Check,
        ),
        ("ALTER TABLE jobs ADD CHECK (x > 0)", Advisory::Check),
        (
            "ALTER TABLE jobs ADD FOREIGN KEY (r) REFERENCES runs (id)",
            Advisory::ForeignKey,
        ),
        (
            "ALTER TABLE jobs ADD COLUMN r int REFERENCES runs (id)",
            Advisory::ForeignKey,
        ),
    ] {
        let found = reading(text);
        assert_eq!(found.narrowings, [], "{text}");
        assert_eq!(found.advisories, [(advisory, relation("jobs"))], "{text}");
        let verdict = check(&[(FIRST, text)]);
        assert!(verdict.is_accepted(), "{text}");
        assert_eq!(verdict.advisories.len(), 1, "{text}");
        assert_eq!(verdict.advisories[0].advisory, advisory);
        assert_eq!(verdict.advisories[0].object, relation("jobs"));
        assert_eq!(verdict.advisories[0].file, FIRST);
    }
}

#[test]
fn creations_are_reported_and_conditional_creates_create_nothing() {
    let cases: Vec<(&str, Vec<Object>)> = vec![
        ("CREATE TABLE jobs (x int)", vec![relation("jobs")]),
        (
            "CREATE UNLOGGED TABLE other.jobs (x int)",
            vec![Object::Relation(Name::new("other", "jobs"))],
        ),
        ("CREATE TEMP VIEW v AS SELECT 1", vec![]),
        (
            "CREATE MATERIALIZED VIEW m AS SELECT 1",
            vec![relation("m")],
        ),
        ("CREATE SEQUENCE s", vec![relation("s")]),
        (
            "CREATE TYPE mood AS ENUM ('a')",
            vec![Object::Type(name("mood"))],
        ),
        ("CREATE DOMAIN d AS int", vec![Object::Type(name("d"))]),
        (
            "CREATE FUNCTION f(a int) RETURNS int LANGUAGE sql AS $$ SELECT a $$",
            vec![Object::Function(name("f"))],
        ),
        (
            "CREATE PROCEDURE p() LANGUAGE sql AS $$ $$",
            vec![Object::Function(name("p"))],
        ),
        ("CREATE SCHEMA s", vec![Object::Schema("s".to_owned())]),
        (
            "ALTER TABLE jobs ADD COLUMN y int, ADD CONSTRAINT c CHECK (y > 0)",
            vec![
                column("jobs", "y"),
                Object::Constraint(name("jobs"), "c".to_owned()),
            ],
        ),
        ("CREATE TABLE IF NOT EXISTS jobs (x int)", vec![]),
        ("CREATE OR REPLACE VIEW v AS SELECT 1", vec![]),
        (
            "CREATE OR REPLACE FUNCTION f() RETURNS int LANGUAGE sql AS $$ SELECT 1 $$",
            vec![],
        ),
        ("CREATE SCHEMA IF NOT EXISTS s", vec![]),
        ("ALTER TABLE jobs ADD COLUMN IF NOT EXISTS y int", vec![]),
        ("CREATE INDEX i ON jobs (x)", vec![relation("i")]),
    ];
    for (text, expected) in cases {
        assert_eq!(reading(text).creates, expected, "{text}");
    }
}

// The interim rule.

#[test]
fn a_narrowing_on_an_existing_object_is_refused() {
    let verdict = check(&[(FIRST, "SELECT 1;\n\nALTER TABLE jobs\n  DROP COLUMN x;")]);
    assert!(!verdict.is_accepted());
    let [refusal] = verdict.refusals.as_slice() else {
        panic!("one refusal: {:?}", verdict.refusals);
    };
    assert_eq!(
        refusal,
        &Refusal::Narrowing {
            file: FIRST.to_owned(),
            line: 3,
            statement: "ALTER TABLE jobs DROP COLUMN x".to_owned(),
            narrowing: Narrowing::Drop,
            object: column("jobs", "x"),
        }
    );
    let shown = refusal.to_string();
    assert!(shown.contains(FIRST), "{shown}");
    assert!(shown.contains("ALTER TABLE jobs DROP COLUMN x"), "{shown}");
    assert!(shown.contains("narrowing waits for floors"), "{shown}");
}

#[test]
fn a_creation_only_in_a_later_file_or_statement_does_not_exempt() {
    let verdict = check(&[
        (FIRST, "DROP TABLE jobs"),
        (SECOND, "CREATE TABLE jobs (x int)"),
    ]);
    assert_eq!(verdict.refusals.len(), 1);
    let verdict = check(&[(FIRST, "DROP TABLE jobs; CREATE TABLE jobs (x int)")]);
    assert_eq!(verdict.refusals.len(), 1);
}

#[test]
fn a_conditional_create_does_not_exempt() {
    let verdict = check(&[
        (FIRST, "CREATE TABLE IF NOT EXISTS jobs (x int)"),
        (SECOND, "ALTER TABLE jobs ALTER COLUMN x SET NOT NULL"),
    ]);
    assert!(matches!(
        verdict.refusals.as_slice(),
        [Refusal::Narrowing { file, narrowing: Narrowing::SetNotNull, .. }] if file == SECOND
    ));
}

#[test]
fn a_statement_does_not_exempt_its_own_later_actions() {
    let verdict = check(&[(
        FIRST,
        "ALTER TABLE jobs ADD COLUMN y int, ALTER COLUMN y SET NOT NULL",
    )]);
    assert_eq!(verdict.refusals.len(), 1);
}

#[test]
fn a_new_not_null_column_without_default_is_refused_on_an_existing_table() {
    let verdict = check(&[(FIRST, "ALTER TABLE jobs ADD COLUMN y int NOT NULL")]);
    assert!(matches!(
        verdict.refusals.as_slice(),
        [Refusal::Narrowing {
            narrowing: Narrowing::NotNullColumnWithoutDefault,
            ..
        }]
    ));
}

#[test]
fn unique_index_and_set_not_null_on_a_table_an_earlier_file_creates_are_accepted() {
    let verdict = check(&[
        (FIRST, "CREATE TABLE jobs (x int, y int)"),
        (
            SECOND,
            "CREATE UNIQUE INDEX jobs_x ON jobs (x);\n\
             ALTER TABLE mintswarm.jobs ALTER COLUMN y SET NOT NULL;\n\
             ALTER TABLE jobs ADD CONSTRAINT u UNIQUE (y), ADD CHECK (y > 0);\n\
             ALTER TABLE jobs ADD COLUMN z int NOT NULL",
        ),
    ]);
    assert_eq!(verdict.refusals, []);
    assert!(verdict.is_accepted());
    assert_eq!(verdict.advisories, []);
}

#[test]
fn set_not_null_on_a_column_an_earlier_file_adds_is_accepted() {
    let verdict = check(&[
        (FIRST, "ALTER TABLE jobs ADD COLUMN y int"),
        (SECOND, "ALTER TABLE jobs ALTER COLUMN y SET NOT NULL"),
    ]);
    assert!(verdict.is_accepted(), "{:?}", verdict.refusals);
    let verdict = check(&[(
        FIRST,
        "ALTER TABLE jobs ADD COLUMN y int;\nALTER TABLE jobs ALTER COLUMN y SET NOT NULL",
    )]);
    assert!(verdict.is_accepted(), "{:?}", verdict.refusals);
    let verdict = check(&[
        (FIRST, "ALTER TABLE jobs ADD COLUMN y int"),
        (SECOND, "ALTER TABLE jobs ALTER COLUMN z SET NOT NULL"),
    ]);
    assert_eq!(verdict.refusals.len(), 1);
}

#[test]
fn a_rename_does_not_carry_the_exemption() {
    let verdict = check(&[
        (FIRST, "CREATE TABLE jobs (x int)"),
        (SECOND, "ALTER TABLE jobs RENAME TO tasks; DROP TABLE tasks"),
    ]);
    assert!(matches!(
        verdict.refusals.as_slice(),
        [Refusal::Narrowing { narrowing: Narrowing::Drop, object, .. }] if *object == relation("tasks")
    ));
}

#[test]
fn a_constraint_on_a_created_domain_is_exempt() {
    let verdict = check(&[
        (FIRST, "CREATE DOMAIN d AS int"),
        (
            SECOND,
            "ALTER DOMAIN d ADD CONSTRAINT p CHECK (VALUE > 0); ALTER DOMAIN d RENAME CONSTRAINT p TO q",
        ),
    ]);
    assert!(verdict.is_accepted(), "{:?}", verdict.refusals);
}

#[test]
fn a_file_that_cannot_be_split_is_refused_and_creates_nothing() {
    let verdict = check(&[
        (FIRST, "CREATE TABLE jobs (x int); SELECT 'open"),
        (SECOND, "DROP TABLE jobs"),
    ]);
    assert_eq!(verdict.refusals.len(), 2);
    assert_eq!(
        verdict.refusals[0],
        Refusal::Lex {
            file: FIRST.to_owned(),
            error: LexError::UnterminatedLiteral { line: 1 },
        }
    );
    assert!(verdict.refusals[0].to_string().contains(FIRST));
    assert!(matches!(&verdict.refusals[1], Refusal::Narrowing { file, .. } if file == SECOND));
}

// Review round 1.

#[test]
fn a_set_default_referential_action_is_not_a_column_default() {
    for action in ["ON DELETE SET DEFAULT", "ON UPDATE SET DEFAULT"] {
        let text = format!("ALTER TABLE jobs ADD COLUMN y int NOT NULL REFERENCES runs {action}");
        assert_eq!(
            narrowings(&text),
            [(Narrowing::NotNullColumnWithoutDefault, column("jobs", "y"))],
            "{text}"
        );
        assert!(!check(&[(FIRST, &text)]).is_accepted(), "{text}");
    }
    for text in [
        "ALTER TABLE jobs ADD COLUMN y int NOT NULL DEFAULT 0 REFERENCES runs ON DELETE SET DEFAULT",
        "ALTER TABLE jobs ADD COLUMN y int GENERATED BY DEFAULT AS IDENTITY NOT NULL",
    ] {
        assert!(narrowings(text).is_empty(), "{text}");
    }
}

#[test]
fn create_schema_with_a_grant_or_revoke_element_is_a_grant() {
    for text in [
        "CREATE SCHEMA s GRANT SELECT ON TABLE t TO r",
        "CREATE SCHEMA IF NOT EXISTS s AUTHORIZATION r CREATE TABLE t (x int) GRANT ALL ON t TO r",
        "CREATE SCHEMA s REVOKE SELECT ON TABLE t FROM r",
    ] {
        assert_eq!(classify(&one(text)), Kind::GrantOrRevoke, "{text}");
    }
    for text in [
        "CREATE SCHEMA s",
        "CREATE SCHEMA \"grant\" CREATE TABLE t (x int)",
    ] {
        assert_eq!(classify(&one(text)), Kind::Ddl, "{text}");
    }
}

#[test]
fn select_into_creates_a_table_so_it_is_ddl() {
    for text in [
        "SELECT * INTO newt FROM jobs",
        "SELECT x INTO TEMP newt FROM jobs",
        "WITH a AS (SELECT 1) SELECT * INTO newt FROM a",
    ] {
        assert_eq!(classify(&one(text)), Kind::Ddl, "{text}");
    }
    for text in [
        "WITH a AS (SELECT 1) INSERT INTO t SELECT * FROM a",
        "WITH a AS (SELECT 1) MERGE INTO t USING a ON true WHEN MATCHED THEN DELETE",
        "SELECT 'INTO' FROM jobs",
    ] {
        assert_eq!(classify(&one(text)), Kind::Data, "{text}");
    }
}

#[test]
fn routines_aggregates_and_foreign_tables_are_read() {
    use Narrowing::*;
    let function = |name: &str| Object::Function(Name::unqualified(name));
    let cases: Vec<(&str, Vec<(Narrowing, Object)>)> = vec![
        ("DROP ROUTINE f", vec![(Drop, function("f"))]),
        (
            "DROP ROUTINE IF EXISTS f(int), g",
            vec![(Drop, function("f")), (Drop, function("g"))],
        ),
        (
            "ALTER ROUTINE f(int) RENAME TO g",
            vec![(Rename, function("f"))],
        ),
        (
            "ALTER ROUTINE f SET SCHEMA other",
            vec![(SetSchema, function("f"))],
        ),
        ("DROP AGGREGATE total(int)", vec![(Drop, function("total"))]),
        (
            "ALTER AGGREGATE total(*) RENAME TO sum_all",
            vec![(Rename, function("total"))],
        ),
        (
            "ALTER AGGREGATE total(int) SET SCHEMA other",
            vec![(SetSchema, function("total"))],
        ),
        ("DROP FOREIGN TABLE f", vec![(Drop, relation("f"))]),
        (
            "DROP FOREIGN TABLE IF EXISTS f, g",
            vec![(Drop, relation("f")), (Drop, relation("g"))],
        ),
        (
            "ALTER FOREIGN TABLE f DROP COLUMN x",
            vec![(Drop, column("f", "x"))],
        ),
        (
            "ALTER FOREIGN TABLE IF EXISTS ONLY f RENAME TO g",
            vec![(Rename, relation("f"))],
        ),
        (
            "ALTER FOREIGN TABLE f ALTER COLUMN x TYPE text",
            vec![(ColumnType, column("f", "x"))],
        ),
        (
            "ALTER FOREIGN TABLE f ALTER x SET NOT NULL",
            vec![(SetNotNull, column("f", "x"))],
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(narrowings(text), expected, "{text}");
    }
    for text in [
        "DROP FOREIGN DATA WRAPPER w",
        "ALTER ROUTINE f(int) IMMUTABLE",
        "ALTER FOREIGN TABLE f ADD COLUMN y int",
    ] {
        assert!(narrowings(text).is_empty(), "{text}");
    }
    assert_eq!(
        reading("CREATE FOREIGN TABLE f (x int) SERVER s").creates,
        [relation("f")]
    );
    assert_eq!(
        reading("CREATE AGGREGATE total(int) (SFUNC = int4pl, STYPE = int)").creates,
        [function("total")]
    );
    assert!(
        check(&[(
            FIRST,
            "CREATE FOREIGN TABLE f (x int) SERVER s; DROP FOREIGN TABLE f"
        )])
        .is_accepted()
    );
}

#[test]
fn a_semicolon_inside_parentheses_is_refused() {
    assert_eq!(
        lex_error(
            "SELECT 1;\nCREATE RULE r AS ON INSERT TO t DO ALSO (DELETE FROM u;\nDROP TABLE v)"
        ),
        LexError::SemicolonInParentheses { line: 2 }
    );
    let refused = check(&[(
        FIRST,
        "CREATE RULE r AS ON INSERT TO t DO ALSO (DELETE FROM u; DROP TABLE v)",
    )]);
    assert!(matches!(
        refused.refusals.as_slice(),
        [Refusal::Lex {
            error: LexError::SemicolonInParentheses { .. },
            ..
        }]
    ));
    assert_eq!(
        statements("CREATE TABLE a (x text DEFAULT ';'); SELECT (1)").len(),
        2
    );
}

#[test]
fn whitespace_and_identifier_characters_are_postgres_classes() {
    for text in [
        "SELECT x\u{2022}e'\\'; DROP TABLE jobs; --'",
        "SELECT \u{a0}e'\\'; DROP TABLE jobs; --'",
    ] {
        let all = statements(text);
        assert_eq!(all.len(), 2, "{text}");
        assert_eq!(
            narrowings(&all[1].text),
            [(Narrowing::Drop, relation("jobs"))],
            "{text}"
        );
    }
    assert_eq!(
        one("SELECT x\u{2022}e FROM \u{e9}t\u{e9}").tokens,
        [
            Token::Word("select".to_owned()),
            Token::Word("x\u{2022}e".to_owned()),
            Token::Word("from".to_owned()),
            Token::Word("\u{e9}t\u{e9}".to_owned()),
        ]
    );
    assert_eq!(
        one("SELECT\x0b1\x0c,\u{2003}2").tokens,
        [
            Token::Word("select".to_owned()),
            Token::Word("1".to_owned()),
            Token::Symbol(','),
            Token::Word("\u{2003}2".to_owned()),
        ]
    );
    assert_eq!(
        one("DO $\u{e9}t\u{e9}1$ ; $\u{e9}t\u{e9}1$").tokens,
        [Token::Word("do".to_owned()), Token::DollarBody]
    );
}

#[test]
fn naming_standard_conforming_strings_is_refused() {
    for text in [
        "SET standard_conforming_strings = off",
        "SET SESSION standard_conforming_strings TO off",
        "SET LOCAL \"Standard_Conforming_Strings\" = off",
        "RESET standard_conforming_strings",
        "ALTER DATABASE d SET STANDARD_CONFORMING_STRINGS = off",
    ] {
        assert_eq!(
            lex_error(&format!(
                "SELECT 1;\n{text}; SELECT '\\'; DROP TABLE jobs; --'"
            )),
            LexError::SessionSetting { line: 2 },
            "{text}"
        );
    }
    assert!(split("SELECT 'standard_conforming_strings'").is_ok());
}

#[test]
fn naming_set_config_or_pg_settings_is_refused() {
    for text in [
        "SELECT set_config('standard_conforming_strings', 'off', false)",
        "SELECT pg_catalog.SET_CONFIG('x', 'y', true)",
        "UPDATE pg_settings SET setting = 'off' WHERE name = 'standard_conforming_strings'",
    ] {
        assert_eq!(
            lex_error(&format!("SELECT 1;\n{text};")),
            LexError::SessionSetting { line: 2 },
            "{text}"
        );
    }
    for text in [
        "SELECT U&\"set\\005fconfig\"('standard_conforming_strings','off',false); SELECT '\\'; SELECT $$'; DROP TABLE jobs; SELECT $$",
        "SET U&\"standard\\005fconforming\\005fstrings\" TO off",
        "UPDATE U&\"pg\\005Fsettings\" SET setting = 'off'",
        "UPDATE U&\"pg!005Fsettings\" /* c */ UESCAPE '!' SET setting = 'off'",
    ] {
        assert_eq!(
            lex_error(&format!("SELECT 1;\n{text};")),
            LexError::UnicodeEscapes { line: 2 },
            "{text}"
        );
    }
    let plain = statements("SELECT U&\"a\" FROM t; SELECT U&\"b\" uescaped");
    assert_eq!(plain[1].tokens[1], Token::Quoted("b".to_owned()));
}

#[test]
fn a_select_with_any_into_is_ddl() {
    for text in [
        "SELECT 1 AS insert INTO newtab",
        "SELECT x insert INTO newtab FROM foo",
        "WITH a AS (SELECT 1) SELECT 1 AS merge INTO t FROM a",
        "WITH a AS (SELECT 1) SELECT (1) insert INTO t FROM a",
        "WITH a AS (SELECT x insert INTO t FROM u) SELECT 1",
        "WITH RECURSIVE a(n) AS (SELECT 1) SEARCH DEPTH FIRST BY n SET o SELECT count(*) insert INTO newt FROM a",
        "WITH RECURSIVE a(n) AS (SELECT 1) SEARCH DEPTH FIRST BY n SET o SELECT (n) merge INTO newt FROM a",
        "WITH RECURSIVE a(insert) AS (SELECT 1) CYCLE insert SET c USING p SELECT DISTINCT ON (insert) insert INTO newt FROM a",
        "WITH RECURSIVE a(n) AS (SELECT 1) CYCLE n SET c USING p INSERT INTO t SELECT n FROM a",
    ] {
        assert_eq!(classify(&one(text)), Kind::Ddl, "{text}");
    }
    for text in [
        "WITH a AS (INSERT INTO t VALUES (1) RETURNING x) SELECT * FROM a",
        "WITH a(x) AS NOT MATERIALIZED (SELECT 1), b AS (SELECT 2) INSERT INTO t SELECT x FROM a",
    ] {
        assert_eq!(classify(&one(text)), Kind::Data, "{text}");
    }
}

#[test]
fn role_statements_are_grants() {
    for text in [
        "CREATE ROLE r IN ROLE admin",
        "CREATE ROLE r",
        "CREATE USER u",
        "CREATE GROUP g USER u",
        "ALTER GROUP g ADD USER u",
        "ALTER GROUP \"G\" DROP USER u, v",
    ] {
        assert_eq!(classify(&one(text)), Kind::GrantOrRevoke, "{text}");
    }
    assert_eq!(classify(&one("ALTER GROUP g RENAME TO h")), Kind::Ddl);
}

#[test]
fn a_number_ends_before_a_dollar() {
    let all = statements("SELECT 1$$ $$; DROP TABLE jobs; -- $$");
    assert_eq!(all.len(), 2);
    assert_eq!(
        all[0].tokens,
        [
            Token::Word("select".to_owned()),
            Token::Word("1".to_owned()),
            Token::DollarBody,
        ]
    );
    assert_eq!(
        narrowings(&all[1].text),
        [(Narrowing::Drop, relation("jobs"))]
    );
    assert_eq!(one("SELECT a$1").tokens[1], Token::Word("a$1".to_owned()));
}

#[test]
fn drops_that_reach_unnamed_objects_are_narrowings() {
    use Narrowing::*;
    let extension = |name: &str| Object::Extension(name.to_owned());
    let dependants = |drop: &str| Object::Dependants(drop.to_owned());
    let cases: Vec<(&str, Vec<(Narrowing, Object)>)> = vec![
        (
            "DROP EXTENSION pgcrypto",
            vec![(Drop, extension("pgcrypto"))],
        ),
        (
            "DROP EXTENSION IF EXISTS a, b CASCADE",
            vec![(Drop, extension("a")), (Drop, extension("b"))],
        ),
        (
            "ALTER EXTENSION pgcrypto SET SCHEMA other",
            vec![(SetSchema, extension("pgcrypto"))],
        ),
        ("DROP OWNED BY app", vec![(Drop, dependants("DROP OWNED"))]),
        (
            "DROP SERVER s CASCADE",
            vec![(Drop, dependants("DROP SERVER"))],
        ),
        (
            "DROP TEXT SEARCH CONFIGURATION c CASCADE",
            vec![(Drop, dependants("DROP TEXT"))],
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(narrowings(text), expected, "{text}");
    }
    assert!(narrowings("DROP SERVER s").is_empty());
    assert!(narrowings("ALTER EXTENSION pgcrypto UPDATE").is_empty());
    assert_eq!(
        reading("CREATE EXTENSION pgcrypto SCHEMA s").creates,
        [extension("pgcrypto")]
    );
    assert!(
        reading("CREATE EXTENSION IF NOT EXISTS pgcrypto")
            .creates
            .is_empty()
    );
    assert!(
        check(&[
            (FIRST, "CREATE EXTENSION pgcrypto"),
            (
                SECOND,
                "ALTER EXTENSION pgcrypto SET SCHEMA s; DROP EXTENSION pgcrypto"
            ),
        ])
        .is_accepted()
    );
    let refused = check(&[(
        FIRST,
        "CREATE SERVER s FOREIGN DATA WRAPPER w; DROP OWNED BY app; DROP SERVER s CASCADE",
    )]);
    assert_eq!(refused.refusals.len(), 2);
    assert!(
        refused.refusals[0]
            .to_string()
            .contains("the objects DROP OWNED reaches")
    );
}

#[test]
fn forms_that_could_hide_a_narrowing_are_refused() {
    for text in [
        "SELECT 1; -- note\rDROP TABLE jobs;\nSELECT 2",
        "-- only a comment\rALTER TABLE jobs ALTER email SET NOT NULL",
        "SET client_encoding = 'SJIS'",
        "SET NAMES 'SJIS'",
        "SET search_path TO scratch; CREATE TABLE jobs (id int); RESET search_path; DROP TABLE jobs",
        "SET SCHEMA 'scratch'",
        "RESET ALL",
        "DISCARD ALL",
        "ALTER INDEX jobs RENAME TO jobs_old",
        "ALTER INDEX IF EXISTS mintswarm.jobs RENAME TO jobs_old",
        "CREATE TEMP TABLE jobs (id int); DROP TABLE mintswarm.jobs",
        "CREATE TEMPORARY TABLE jobs (id int); DROP TABLE jobs; DROP TABLE jobs",
        "ALTER TABLE jobs ADD COLUMN flag boolean NOT NULL DEFAULT NULL",
        "ALTER TABLE jobs ADD CONSTRAINT nn NOT NULL email",
        "ALTER TABLE jobs ADD NOT NULL email",
        "UPDATE pg_catalog.pg_attribute SET attnotnull = true WHERE attname = 'email'",
    ] {
        assert!(!check(&[(FIRST, text)]).is_accepted(), "{text}");
    }
    let accepted = "CREATE INDEX i ON jobs (x); ALTER INDEX i RENAME TO j; SET LOCAL lock_timeout = 0; CREATE FUNCTION f() RETURNS int SET search_path = x AS $$ SELECT 1 $$";
    assert!(check(&[(FIRST, accepted)]).is_accepted());
    let lone_cr = statements("SELECT 1; -- c\rSELECT 2;\r\nSELECT 3");
    assert_eq!(lone_cr[2].line, 3);
    let catalog = one("UPDATE pg_attribute SET attnotnull = true");
    assert_eq!(classify(&catalog), Kind::Ddl);
}

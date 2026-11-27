//! Narrowing changes found lexically in migration SQL, and the interim rule over them.
//!
//! [`read`] reports, for one statement, each narrowing with the object it narrows, each check or
//! foreign key it adds as an advisory, and each object it creates. A column added with
//! `ADD COLUMN` counts as created. Matching is on leading keyword paths, and a multi-action
//! `ALTER TABLE` is read one action at a time. Names fold as Postgres folds them: unquoted
//! words to lower case, quoted ones kept, and an unqualified name is in [`SCHEMA`].
//!
//! [`check`] is the rule that holds until floors exist: over an ordered list of files, a
//! narrowing is allowed only on an object that an earlier statement creates, in the same file or
//! an earlier one. `CREATE … IF NOT EXISTS` and `CREATE OR REPLACE` create nothing for it, and a
//! statement's own creations count only for later statements.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use super::SCHEMA;
use super::sql::{LexError, Statement, Token, split};

/// A schema-qualified name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Name {
    /// The schema, [`SCHEMA`] when the name is unqualified.
    pub schema: String,
    /// The name within the schema.
    pub name: String,
}

impl Name {
    /// A name in `schema`.
    pub fn new(schema: &str, name: &str) -> Self {
        Self {
            schema: schema.to_owned(),
            name: name.to_owned(),
        }
    }

    /// An unqualified name, so in [`SCHEMA`].
    pub fn unqualified(name: &str) -> Self {
        Self::new(SCHEMA, name)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.schema, self.name)
    }
}

/// An object a statement narrows or creates.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Object {
    /// A table, view, materialised view or sequence.
    Relation(Name),
    /// A column of a relation.
    Column(Name, String),
    /// A constraint of a table, or of a domain.
    Constraint(Name, String),
    /// A type or a domain.
    Type(Name),
    /// A function or procedure, by name only: overloads are not told apart.
    Function(Name),
    /// A schema.
    Schema(String),
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Relation(name) => write!(f, "relation {name}"),
            Self::Column(name, column) => write!(f, "column {name}.{column}"),
            Self::Constraint(name, constraint) => write!(f, "constraint {constraint} on {name}"),
            Self::Type(name) => write!(f, "type {name}"),
            Self::Function(name) => write!(f, "function {name}"),
            Self::Schema(name) => write!(f, "schema {name}"),
        }
    }
}

/// A kind of narrowing change from the migration procedure's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Narrowing {
    /// A drop of a relation, type, function, schema or column.
    Drop,
    /// A rename of a relation, column, type, function, constraint or schema.
    Rename,
    /// A move to another schema.
    SetSchema,
    /// `SET NOT NULL` on a column.
    SetNotNull,
    /// `ADD COLUMN … NOT NULL` without a default.
    NotNullColumnWithoutDefault,
    /// A change of a column's type.
    ColumnType,
    /// An added unique, primary key or exclusion constraint.
    UniqueConstraint,
    /// `CREATE UNIQUE INDEX`.
    UniqueIndex,
    /// An added domain constraint, or `SET NOT NULL` on a domain.
    DomainConstraint,
    /// `ALTER TYPE … RENAME VALUE`.
    EnumValueRename,
    /// An added, dropped, altered or renamed attribute of a composite type.
    AttributeChange,
}

impl fmt::Display for Narrowing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Drop => "drop",
            Self::Rename => "rename",
            Self::SetSchema => "set schema",
            Self::SetNotNull => "set not null",
            Self::NotNullColumnWithoutDefault => "not-null column without a default",
            Self::ColumnType => "column type change",
            Self::UniqueConstraint => "unique constraint",
            Self::UniqueIndex => "unique index",
            Self::DomainConstraint => "domain constraint",
            Self::EnumValueRename => "enum value rename",
            Self::AttributeChange => "attribute change",
        })
    }
}

/// A change that is advisory, not narrowing: old writers may break on it, but it does not wait
/// for floors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advisory {
    /// An added check constraint.
    Check,
    /// An added foreign key.
    ForeignKey,
}

impl fmt::Display for Advisory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Check => "check",
            Self::ForeignKey => "foreign key",
        })
    }
}

/// What one statement narrows, advises and creates, each in statement order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    /// Each narrowing with the object it narrows.
    pub narrowings: Vec<(Narrowing, Object)>,
    /// Each advisory with the table it is added to.
    pub advisories: Vec<(Advisory, Object)>,
    /// Each object the statement creates.
    pub creates: Vec<Object>,
}

impl Reading {
    fn narrows(&mut self, narrowing: Narrowing, object: Object) {
        self.narrowings.push((narrowing, object));
    }

    fn advises(&mut self, advisory: Advisory, table: &Name) {
        self.advisories
            .push((advisory, Object::Relation(table.clone())));
    }
}

/// Reads one statement. Data statements, grants, comments and forms off the procedure's list
/// give nothing.
pub fn read(statement: &Statement) -> Reading {
    let mut reading = Reading::default();
    let mut cursor = Cursor::new(&statement.tokens);
    if cursor.eat("drop") {
        read_drop(&mut cursor, &mut reading);
    } else if cursor.eat("alter") {
        read_alter(&mut cursor, &mut reading);
    } else if cursor.eat("create") {
        read_create(&mut cursor, &mut reading);
    }
    reading
}

/// The class of object a `DROP` or `CREATE` names.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Relation,
    Type,
    Function,
    Schema,
}

/// Reads the class keyword of a `DROP` or `CREATE`.
fn class(cursor: &mut Cursor<'_>) -> Option<Class> {
    if cursor.eat("table")
        || cursor.eat("view")
        || cursor.eat("sequence")
        || cursor.eat_all(&["materialized", "view"])
    {
        Some(Class::Relation)
    } else if cursor.eat("type") || cursor.eat("domain") {
        Some(Class::Type)
    } else if cursor.eat("function") || cursor.eat("procedure") {
        Some(Class::Function)
    } else if cursor.eat("schema") {
        Some(Class::Schema)
    } else {
        None
    }
}

/// Reads the object a `DROP` or `CREATE` of `class` names, skipping a function's arguments.
fn target(cursor: &mut Cursor<'_>, class: Class) -> Option<Object> {
    match class {
        Class::Relation => cursor.name().map(Object::Relation),
        Class::Type => cursor.name().map(Object::Type),
        Class::Function => {
            let name = cursor.name()?;
            cursor.skip_parens();
            Some(Object::Function(name))
        }
        Class::Schema => cursor.ident().map(Object::Schema),
    }
}

/// `DROP <class> [IF EXISTS] a, b …`; other drops give nothing.
fn read_drop(cursor: &mut Cursor<'_>, reading: &mut Reading) {
    let Some(class) = class(cursor) else {
        return;
    };
    cursor.eat_all(&["if", "exists"]);
    while let Some(object) = target(cursor, class) {
        reading.narrows(Narrowing::Drop, object);
        if !cursor.eat_symbol(',') {
            break;
        }
    }
}

/// `ALTER TABLE`, `VIEW`, `MATERIALIZED VIEW`, `SEQUENCE`, `TYPE`, `DOMAIN`, `FUNCTION`,
/// `PROCEDURE` or `SCHEMA`; other alters give nothing.
fn read_alter(cursor: &mut Cursor<'_>, reading: &mut Reading) {
    if cursor.eat("table") {
        cursor.eat_all(&["if", "exists"]);
        cursor.eat("only");
        let Some(table) = cursor.name() else {
            return;
        };
        cursor.eat_symbol('*');
        for action in actions(cursor.rest()) {
            table_action(&table, &mut Cursor::new(action), reading);
        }
    } else if cursor.eat("view")
        || cursor.eat("sequence")
        || cursor.eat_all(&["materialized", "view"])
    {
        cursor.eat_all(&["if", "exists"]);
        if let Some(relation) = cursor.name() {
            rename_or_move(&relation, cursor, reading);
        }
    } else if cursor.eat("type") {
        let Some(name) = cursor.name() else {
            return;
        };
        for action in actions(cursor.rest()) {
            type_action(&name, &mut Cursor::new(action), reading);
        }
    } else if cursor.eat("domain") {
        if let Some(name) = cursor.name() {
            domain_action(&name, cursor, reading);
        }
    } else if cursor.eat("function") || cursor.eat("procedure") {
        let Some(name) = cursor.name() else {
            return;
        };
        cursor.skip_parens();
        if cursor.eat_all(&["rename", "to"]) {
            reading.narrows(Narrowing::Rename, Object::Function(name));
        } else if cursor.eat_all(&["set", "schema"]) {
            reading.narrows(Narrowing::SetSchema, Object::Function(name));
        }
    } else if cursor.eat("schema")
        && let Some(name) = cursor.ident()
        && cursor.eat_all(&["rename", "to"])
    {
        reading.narrows(Narrowing::Rename, Object::Schema(name));
    }
}

/// One action of `ALTER TABLE`.
fn table_action(table: &Name, cursor: &mut Cursor<'_>, reading: &mut Reading) {
    let column = |name: String| Object::Column(table.clone(), name);
    if cursor.eat("add") {
        add_action(table, cursor, reading);
    } else if cursor.eat("drop") {
        if cursor.is("constraint") {
            return;
        }
        cursor.eat("column");
        cursor.eat_all(&["if", "exists"]);
        if let Some(name) = cursor.ident() {
            reading.narrows(Narrowing::Drop, column(name));
        }
    } else if cursor.eat("alter") {
        if cursor.is("constraint") {
            return;
        }
        cursor.eat("column");
        let Some(name) = cursor.ident() else {
            return;
        };
        if cursor.eat_all(&["set", "not", "null"]) {
            reading.narrows(Narrowing::SetNotNull, column(name));
        } else if cursor.eat("type") || cursor.eat_all(&["set", "data", "type"]) {
            reading.narrows(Narrowing::ColumnType, column(name));
        }
    } else {
        rename_or_move(table, cursor, reading);
    }
}

/// `RENAME TO`, `RENAME CONSTRAINT`, `RENAME [COLUMN]` or `SET SCHEMA` on a relation.
fn rename_or_move(relation: &Name, cursor: &mut Cursor<'_>, reading: &mut Reading) {
    if cursor.eat("rename") {
        if cursor.eat("to") {
            reading.narrows(Narrowing::Rename, Object::Relation(relation.clone()));
        } else if cursor.eat("constraint") {
            if let Some(name) = cursor.ident() {
                let object = Object::Constraint(relation.clone(), name);
                reading.narrows(Narrowing::Rename, object);
            }
        } else {
            cursor.eat("column");
            if let Some(name) = cursor.ident() {
                let object = Object::Column(relation.clone(), name);
                reading.narrows(Narrowing::Rename, object);
            }
        }
    } else if cursor.eat_all(&["set", "schema"]) {
        reading.narrows(Narrowing::SetSchema, Object::Relation(relation.clone()));
    }
}

/// The keywords that start a table constraint.
const CONSTRAINT_STARTS: &[&str] = &["unique", "primary", "exclude", "check", "foreign"];

/// `ALTER TABLE … ADD`, after `ADD`: a table constraint or a column.
fn add_action(table: &Name, cursor: &mut Cursor<'_>, reading: &mut Reading) {
    if cursor.eat("constraint") {
        let name = cursor.ident();
        table_constraint(table, cursor, reading);
        if let Some(name) = name {
            reading
                .creates
                .push(Object::Constraint(table.clone(), name));
        }
        return;
    }
    if CONSTRAINT_STARTS.iter().any(|word| cursor.is(word)) {
        table_constraint(table, cursor, reading);
        return;
    }
    cursor.eat("column");
    let if_not_exists = cursor.eat_all(&["if", "not", "exists"]);
    let Some(name) = cursor.ident() else {
        return;
    };
    let mut not_null = false;
    let mut default = false;
    let mut depth = 0usize;
    let rest = cursor.rest();
    for (at, token) in rest.iter().enumerate() {
        match token {
            Token::Symbol('(') => depth += 1,
            Token::Symbol(')') => depth = depth.saturating_sub(1),
            Token::Word(word) if depth == 0 => match word.as_str() {
                "not" if matches!(rest.get(at + 1), Some(Token::Word(next)) if next == "null") => {
                    not_null = true;
                }
                "default" | "generated" => default = true,
                "unique" | "primary" => {
                    reading.narrows(Narrowing::UniqueConstraint, Object::Relation(table.clone()));
                }
                "references" => reading.advises(Advisory::ForeignKey, table),
                "check" => reading.advises(Advisory::Check, table),
                _ => {}
            },
            _ => {}
        }
    }
    let column = Object::Column(table.clone(), name);
    if not_null && !default {
        reading.narrows(Narrowing::NotNullColumnWithoutDefault, column.clone());
    }
    if !if_not_exists {
        reading.creates.push(column);
    }
}

/// A table constraint after `ADD [CONSTRAINT n]`.
fn table_constraint(table: &Name, cursor: &mut Cursor<'_>, reading: &mut Reading) {
    if cursor.is("unique") || cursor.is("primary") || cursor.is("exclude") {
        reading.narrows(Narrowing::UniqueConstraint, Object::Relation(table.clone()));
    } else if cursor.is("check") {
        reading.advises(Advisory::Check, table);
    } else if cursor.is("foreign") {
        reading.advises(Advisory::ForeignKey, table);
    }
}

/// One action of `ALTER TYPE`.
fn type_action(name: &Name, cursor: &mut Cursor<'_>, reading: &mut Reading) {
    let object = || Object::Type(name.clone());
    if cursor.eat_all(&["rename", "value"]) {
        reading.narrows(Narrowing::EnumValueRename, object());
    } else if cursor.eat_all(&["rename", "attribute"]) {
        reading.narrows(Narrowing::AttributeChange, object());
    } else if cursor.eat_all(&["rename", "to"]) {
        reading.narrows(Narrowing::Rename, object());
    } else if cursor.eat_all(&["set", "schema"]) {
        reading.narrows(Narrowing::SetSchema, object());
    } else if (cursor.eat("add") || cursor.eat("drop") || cursor.eat("alter"))
        && cursor.is("attribute")
    {
        reading.narrows(Narrowing::AttributeChange, object());
    }
}

/// The action of `ALTER DOMAIN`.
fn domain_action(name: &Name, cursor: &mut Cursor<'_>, reading: &mut Reading) {
    let object = || Object::Type(name.clone());
    if cursor.eat("add") || cursor.eat_all(&["set", "not", "null"]) {
        reading.narrows(Narrowing::DomainConstraint, object());
    } else if cursor.eat_all(&["rename", "constraint"]) {
        if let Some(constraint) = cursor.ident() {
            let constraint = Object::Constraint(name.clone(), constraint);
            reading.narrows(Narrowing::Rename, constraint);
        }
    } else if cursor.eat_all(&["rename", "to"]) {
        reading.narrows(Narrowing::Rename, object());
    } else if cursor.eat_all(&["set", "schema"]) {
        reading.narrows(Narrowing::SetSchema, object());
    }
}

/// The words that may come between `CREATE` and the class keyword.
const CREATE_MODIFIERS: &[&str] = &[
    "global",
    "local",
    "unlogged",
    "temp",
    "temporary",
    "recursive",
];

/// `CREATE [UNIQUE] INDEX`, or a create of a relation, type, function or schema.
fn read_create(cursor: &mut Cursor<'_>, reading: &mut Reading) {
    let or_replace = cursor.eat_all(&["or", "replace"]);
    let unique = cursor.eat("unique");
    if cursor.eat("index") {
        if !unique {
            return;
        }
        cursor.eat("concurrently");
        cursor.eat_all(&["if", "not", "exists"]);
        if !cursor.eat("on") && (cursor.ident().is_none() || !cursor.eat("on")) {
            return;
        }
        cursor.eat("only");
        if let Some(table) = cursor.name() {
            reading.narrows(Narrowing::UniqueIndex, Object::Relation(table));
        }
        return;
    }
    if unique {
        return;
    }
    while CREATE_MODIFIERS.iter().any(|word| cursor.eat(word)) {}
    let Some(class) = class(cursor) else {
        return;
    };
    let if_not_exists = cursor.eat_all(&["if", "not", "exists"]);
    if class == Class::Schema {
        cursor.eat("authorization");
    }
    if let Some(object) = target(cursor, class)
        && !or_replace
        && !if_not_exists
    {
        reading.creates.push(object);
    }
}

/// Splits tokens at the commas outside parentheses.
fn actions(tokens: &[Token]) -> Vec<&[Token]> {
    let mut actions = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (at, token) in tokens.iter().enumerate() {
        match token {
            Token::Symbol('(') => depth += 1,
            Token::Symbol(')') => depth = depth.saturating_sub(1),
            Token::Symbol(',') if depth == 0 => {
                actions.push(&tokens[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    actions.push(&tokens[start..]);
    actions
}

/// A cursor over a statement's tokens.
struct Cursor<'a> {
    tokens: &'a [Token],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, at: 0 }
    }

    fn peek(&self) -> Option<&'a Token> {
        self.tokens.get(self.at)
    }

    fn rest(&self) -> &'a [Token] {
        self.tokens.get(self.at..).unwrap_or_default()
    }

    /// Whether the next token is the word `word`.
    fn is(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Token::Word(next)) if next == word)
    }

    /// Moves past the word `word` if it is next.
    fn eat(&mut self, word: &str) -> bool {
        let found = self.is(word);
        if found {
            self.at += 1;
        }
        found
    }

    /// Moves past all of `words` if they are next, or past none.
    fn eat_all(&mut self, words: &[&str]) -> bool {
        let start = self.at;
        if words.iter().all(|word| self.eat(word)) {
            return true;
        }
        self.at = start;
        false
    }

    fn eat_symbol(&mut self, symbol: char) -> bool {
        let found = self.peek() == Some(&Token::Symbol(symbol));
        if found {
            self.at += 1;
        }
        found
    }

    /// Reads an identifier: a word, folded, or a quoted identifier, as written.
    fn ident(&mut self) -> Option<String> {
        let ident = match self.peek()? {
            Token::Word(word) | Token::Quoted(word) => word.clone(),
            _ => return None,
        };
        self.at += 1;
        Some(ident)
    }

    /// Reads a dotted name, of which the last two parts are kept.
    fn name(&mut self) -> Option<Name> {
        let mut parts = vec![self.ident()?];
        while self.peek() == Some(&Token::Symbol('.')) {
            let dot = self.at;
            self.at += 1;
            match self.ident() {
                Some(part) => parts.push(part),
                None => {
                    self.at = dot;
                    break;
                }
            }
        }
        let name = parts.pop()?;
        Some(match parts.pop() {
            Some(schema) => Name { schema, name },
            None => Name::unqualified(&name),
        })
    }

    /// Moves past a parenthesised group if one is next.
    fn skip_parens(&mut self) {
        if !self.eat_symbol('(') {
            return;
        }
        let mut depth = 1usize;
        while depth > 0 {
            match self.peek() {
                None => return,
                Some(Token::Symbol('(')) => depth += 1,
                Some(Token::Symbol(')')) => depth -= 1,
                Some(_) => {}
            }
            self.at += 1;
        }
    }
}

/// An advisory the interim rule found on a table no earlier statement creates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// The file, relative to the tree root.
    pub file: String,
    /// The statement's first line, counted from 1.
    pub line: usize,
    /// The statement's text, comments removed and whitespace collapsed.
    pub statement: String,
    /// The advisory.
    pub advisory: Advisory,
    /// The table it is added to.
    pub object: Object,
}

impl fmt::Display for Notice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            file,
            line,
            statement,
            advisory,
            object,
        } = self;
        write!(
            f,
            "{file}:{line}: {statement}: adds a {advisory} to {object}, which old writers may break; advisory, not narrowing"
        )
    }
}

/// Why the interim rule refuses a list of files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A narrowing on an object no earlier statement of the list creates.
    Narrowing {
        /// The file, relative to the tree root.
        file: String,
        /// The statement's first line, counted from 1.
        line: usize,
        /// The statement's text, comments removed and whitespace collapsed.
        statement: String,
        /// The narrowing.
        narrowing: Narrowing,
        /// The object it narrows.
        object: Object,
    },
    /// A file whose text cannot be split safely, so nothing of it is read.
    Lex {
        /// The file, relative to the tree root.
        file: String,
        /// Why.
        error: LexError,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Narrowing {
                file,
                line,
                statement,
                narrowing,
                object,
            } => write!(
                f,
                "{file}:{line}: {statement}: narrows {object} ({narrowing}), which no earlier statement creates; narrowing waits for floors"
            ),
            Self::Lex { file, error } => write!(f, "{file}: {error}"),
        }
    }
}

impl Error for Refusal {}

/// The interim rule's verdict on a list of files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    /// The advisories, in list order.
    pub advisories: Vec<Notice>,
    /// The refusals, in list order.
    pub refusals: Vec<Refusal>,
}

impl Verdict {
    /// Whether the list is accepted: nothing is refused.
    pub fn is_accepted(&self) -> bool {
        self.refusals.is_empty()
    }
}

/// Runs the interim rule over `(path relative to the tree root, text)` pairs in list order.
///
/// A narrowing is exempt when its object, or for a column or constraint the table or type that
/// holds it, is created by an earlier statement of the list. A file that cannot be split is
/// refused, and creates nothing for later files.
pub fn check(files: &[(&str, &str)]) -> Verdict {
    let mut verdict = Verdict::default();
    let mut created = BTreeSet::new();
    for (file, text) in files {
        let statements = match split(text) {
            Ok(statements) => statements,
            Err(error) => {
                verdict.refusals.push(Refusal::Lex {
                    file: (*file).to_owned(),
                    error,
                });
                continue;
            }
        };
        for statement in &statements {
            let reading = read(statement);
            for (narrowing, object) in reading.narrowings {
                if !is_exempt(&created, &object) {
                    verdict.refusals.push(Refusal::Narrowing {
                        file: (*file).to_owned(),
                        line: statement.line,
                        statement: statement.text.clone(),
                        narrowing,
                        object,
                    });
                }
            }
            for (advisory, object) in reading.advisories {
                if !is_exempt(&created, &object) {
                    verdict.advisories.push(Notice {
                        file: (*file).to_owned(),
                        line: statement.line,
                        statement: statement.text.clone(),
                        advisory,
                        object,
                    });
                }
            }
            created.extend(reading.creates);
        }
    }
    verdict
}

/// Whether an earlier statement created the object, or the table or type holding it.
fn is_exempt(created: &BTreeSet<Object>, object: &Object) -> bool {
    if created.contains(object) {
        return true;
    }
    match object {
        Object::Column(holder, _) | Object::Constraint(holder, _) => {
            created.contains(&Object::Relation(holder.clone()))
                || created.contains(&Object::Type(holder.clone()))
        }
        _ => false,
    }
}

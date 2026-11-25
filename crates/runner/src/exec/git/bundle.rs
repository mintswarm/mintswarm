//! The plan floor's input bundle: what `plan-approval/floor.py` reads from git and from files,
//! extracted from the trusted store at the pinned base commit and from the job's documents, each
//! item bound to its sha256.

use std::path::{Path, PathBuf};

use mintswarm_domain::records::BaseRef;
use mintswarm_domain::vcs::{RepoPath, Vcs};
use mintswarm_rules::exit::Unusable;
use mintswarm_rules::plan_floor::{Bundle, CONFIG, Item, Subjects};

use super::{GitError, GitStore, read};

/// The job's documents, each a path relative to the documents root, bound under that path as
/// given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Documents {
    /// The plan.
    pub plan: String,
    /// The issue document, when given.
    pub issue_document: Option<String>,
    /// The analysis, when given.
    pub analysis: Option<String>,
    /// The screen record, when given.
    pub screen: Option<String>,
}

/// Why a bundle cannot be extracted: the inputs on which `floor.py` exits 2. The text is ours
/// alone; a git or file error is only the `source()`.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    /// The base commit is not a commit in the store.
    #[error("the base {reference} ({commit}) is not a commit in the store")]
    Base {
        /// The base ref as given.
        reference: String,
        /// The commit it was pinned to.
        commit: String,
        /// Why it did not resolve.
        #[source]
        source: GitError,
    },
    /// The base commit has no config.
    #[error("no {CONFIG} at {0}")]
    NoConfig(String),
    /// The store could not be read at the base commit.
    #[error("cannot read the {what} at {commit}")]
    Store {
        /// What was being read.
        what: &'static str,
        /// The base commit.
        commit: String,
        /// The adapter's error.
        #[source]
        source: GitError,
    },
    /// A document or result record that is missing, not a regular file, or unreadable.
    #[error("cannot read the {what} {path}")]
    Document {
        /// What the file is.
        what: &'static str,
        /// The path, as given or as named beside its document.
        path: String,
        /// The file error, when there was one.
        #[source]
        source: Option<std::io::Error>,
    },
}

impl BundleError {
    /// The floor's unusable input for this error: exit class 2.
    pub fn unusable(&self) -> Unusable {
        Unusable::new(self.to_string())
    }
}

/// The plan floor's bundle for the plan at `base` and the issue's `components`: the config and the
/// raw subjects from `store` at `base.commit`, and the documents and the result records beside the
/// analysis and the plan read under `root`.
pub fn plan_floor_bundle(
    store: &GitStore,
    base: BaseRef,
    components: &[String],
    root: &Path,
    documents: &Documents,
) -> Result<Bundle, BundleError> {
    let commit = store
        .resolve(&base.commit.0)
        .map_err(|source| BundleError::Base {
            reference: base.reference.clone(),
            commit: base.commit.0.clone(),
            source,
        })?;
    let at = |what: &'static str| {
        let commit = commit.0.clone();
        move |source| BundleError::Store {
            what,
            commit,
            source,
        }
    };
    let config = store
        .read_blob(&commit, &RepoPath(CONFIG.as_bytes().to_vec()))
        .map_err(at("config"))?
        .ok_or_else(|| BundleError::NoConfig(commit.0.clone()))?;
    let plan = document(root, &documents.plan, "plan")?;
    let optional = |path: &Option<String>, what| {
        path.as_deref()
            .map(|path| document(root, path, what))
            .transpose()
    };
    let issue_document = optional(&documents.issue_document, "issue document")?;
    let analysis = optional(&documents.analysis, "analysis")?;
    let screen = optional(&documents.screen, "screen record")?;
    let analysis_record = match &documents.analysis {
        Some(path) => record(root, path)?,
        None => None,
    };
    let plan_record = record(root, &documents.plan)?;
    let subjects = read::subjects(store, &commit).map_err(at("history"))?;
    Ok(Bundle {
        config: Item::new(CONFIG, config),
        subjects: Subjects::new(subjects),
        base: BaseRef {
            reference: base.reference,
            commit,
        },
        components: components.to_vec(),
        plan,
        issue_document,
        analysis,
        screen,
        analysis_record,
        plan_record,
    })
}

/// A document at `path` under `root`, which must be a regular file after following links.
fn document(root: &Path, path: &str, what: &'static str) -> Result<Item, BundleError> {
    let refuse = |source| BundleError::Document {
        what,
        path: path.to_owned(),
        source,
    };
    let file = root.join(path);
    let meta = std::fs::metadata(&file).map_err(|error| refuse(Some(error)))?;
    if !meta.is_file() {
        return Err(refuse(None));
    }
    let bytes = std::fs::read(&file).map_err(|error| refuse(Some(error)))?;
    Ok(Item::new(path, bytes))
}

/// The result record beside the document at `path`, named as `floor.py`'s `record_path` names it:
/// `<name>.md` becomes `<name>.result.json`, any other name gets `.result.json` appended. `None`
/// when there is none; one that exists must be a regular file after following links.
fn record(root: &Path, path: &str) -> Result<Option<Item>, BundleError> {
    let what = "result record";
    let named = record_path(path).ok_or_else(|| BundleError::Document {
        what,
        path: path.to_owned(),
        source: None,
    })?;
    let shown = named.to_string_lossy().into_owned();
    let refuse = |source| BundleError::Document {
        what,
        path: shown.clone(),
        source,
    };
    let file = root.join(&named);
    let meta = match std::fs::metadata(&file) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(refuse(Some(error))),
    };
    if !meta.is_file() {
        return Err(refuse(None));
    }
    let bytes = std::fs::read(&file).map_err(|error| refuse(Some(error)))?;
    Ok(Some(Item::new(shown.clone(), bytes)))
}

/// The path of the result record beside `path`, or `None` when it names no file.
fn record_path(path: &str) -> Option<PathBuf> {
    let document = Path::new(path);
    let name = document.file_name()?.to_str()?;
    let stem = name.strip_suffix(".md").unwrap_or(name);
    Some(document.with_file_name(format!("{stem}.result.json")))
}

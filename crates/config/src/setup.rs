//! The project setup file: the factory's branches, starting with the work branch.
//!
//! While mintswarm is dogfooded the file is `.mintswarm/dogfood/project-setup.yaml`, read from
//! the checkout, and `.mintswarm/dogfood/project-setup.py` is its one reader outside Rust. The
//! case table `tests/conformance/project-setup.json` holds both readers to the same rules. The
//! caller finds and reads the file; this module parses its text.

use mintswarm_domain::setup::ProjectSetup;
use mintswarm_domain::vcs::BranchName;
use serde_json::{Map, Value};

use crate::{Error, Path, parse_yaml};

/// Parse and validate a project setup file.
///
/// The document is a mapping with one key, `branches`, itself a mapping with one key, `work`:
/// the work branch, a string that passes [`BranchName::is_valid`]. Both are required, and any
/// other key is rejected with its path.
pub fn load_project_setup(text: &str) -> Result<ProjectSetup, Error> {
    let value = parse_yaml(text)?;
    let root = Path::root();
    let map = mapping(&value, &root, "the project setup must be a mapping")?;
    only_keys(map, &root, &["branches"])?;
    let branches_path = root.key("branches");
    let branches = map
        .get("branches")
        .ok_or_else(|| Error::schema(&branches_path, "is missing"))?;
    let branches = mapping(branches, &branches_path, "must be a mapping")?;
    only_keys(branches, &branches_path, &["work"])?;
    let work_path = branches_path.key("work");
    let work = match branches.get("work") {
        Some(Value::String(name)) => name,
        Some(_) => return Err(Error::schema(&work_path, "must be a string")),
        None => return Err(Error::schema(&work_path, "is missing")),
    };
    ProjectSetup::new(BranchName(work.clone()))
        .map_err(|_| Error::schema(&work_path, "is not a valid branch name"))
}

fn mapping<'a>(
    value: &'a Value,
    path: &Path,
    message: &str,
) -> Result<&'a Map<String, Value>, Error> {
    value
        .as_object()
        .ok_or_else(|| Error::schema(path, message))
}

fn only_keys(map: &Map<String, Value>, path: &Path, keys: &[&str]) -> Result<(), Error> {
    match map.keys().find(|key| !keys.contains(&key.as_str())) {
        Some(key) => Err(Error::schema(&path.key(key), "unknown key")),
        None => Ok(()),
    }
}

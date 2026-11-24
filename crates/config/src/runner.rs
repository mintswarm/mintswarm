//! The runner config: where a runner keeps its state, how many jobs it runs at once, how often
//! it polls, and the wall-clock limit of each stage.
//!
//! The caller finds and reads the file; this module parses its text.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::{Error, Path, parse_yaml};

/// A stage, as named in `wall_clock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    /// `analyse`.
    Analyse,
    /// `design`.
    Design,
    /// `decompose`.
    Decompose,
    /// `plan`.
    Plan,
    /// `assess`.
    Assess,
    /// `implement`.
    Implement,
    /// `address`.
    Address,
    /// `gates`.
    Gates,
    /// `review`.
    Review,
    /// `landing`.
    Landing,
}

impl Stage {
    /// Every stage, in pipeline order.
    pub const ALL: [Stage; 10] = [
        Stage::Analyse,
        Stage::Design,
        Stage::Decompose,
        Stage::Plan,
        Stage::Assess,
        Stage::Implement,
        Stage::Address,
        Stage::Gates,
        Stage::Review,
        Stage::Landing,
    ];

    /// The stage's name in the config.
    pub fn name(self) -> &'static str {
        match self {
            Stage::Analyse => "analyse",
            Stage::Design => "design",
            Stage::Decompose => "decompose",
            Stage::Plan => "plan",
            Stage::Assess => "assess",
            Stage::Implement => "implement",
            Stage::Address => "address",
            Stage::Gates => "gates",
            Stage::Review => "review",
            Stage::Landing => "landing",
        }
    }

    /// The stage named `name` in the config, if there is one.
    pub(crate) fn from_name(name: &str) -> Option<Stage> {
        Stage::ALL.into_iter().find(|stage| stage.name() == name)
    }
}

/// The wall-clock limit of a stage that `wall_clock` does not name.
pub const DEFAULT_WALL_CLOCK: Duration = Duration::from_secs(3600);

/// The poll interval when the config gives none.
const DEFAULT_POLL_INTERVAL: u64 = 30;
/// The shortest and longest poll interval, in seconds.
const POLL_INTERVAL: (u64, u64) = (10, 60);

/// A validated runner config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerConfig {
    state_dir: PathBuf,
    workers: u64,
    poll_interval: Duration,
    wall_clock: BTreeMap<Stage, Duration>,
}

impl RunnerConfig {
    /// `state_dir`: where the runner keeps its state.
    pub fn state_dir(&self) -> &std::path::Path {
        &self.state_dir
    }

    /// `workers`: how many jobs run at once, 1 when absent.
    pub fn workers(&self) -> u64 {
        self.workers
    }

    /// `poll_interval`, 30 s when absent.
    pub fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    /// The wall-clock limit of `stage`: `wall_clock.<stage>`, or [`DEFAULT_WALL_CLOCK`].
    pub fn wall_clock(&self, stage: Stage) -> Duration {
        self.wall_clock
            .get(&stage)
            .copied()
            .unwrap_or(DEFAULT_WALL_CLOCK)
    }
}

/// Parse and validate a runner config.
///
/// The keys are `state_dir` (a non-empty string, required), `workers` (an integer of 1 or more,
/// default 1), `poll_interval` (whole seconds from 10 to 60, default 30) and `wall_clock` (a
/// positive number of seconds per stage, default 3600). Any other key is rejected.
pub fn load_runner_config(text: &str) -> Result<RunnerConfig, Error> {
    let value = parse_yaml(text)?;
    let root = Path::root();
    let map = value
        .as_object()
        .ok_or_else(|| Error::schema(&root, "the runner config must be a mapping"))?;
    if let Some(key) = map.keys().find(|key| {
        !["state_dir", "workers", "poll_interval", "wall_clock"].contains(&key.as_str())
    }) {
        return Err(Error::schema(&root.key(key), "unknown key"));
    }

    let state_dir = match map.get("state_dir") {
        Some(Value::String(dir)) if !dir.is_empty() => PathBuf::from(dir),
        Some(_) => {
            return Err(Error::schema(
                &root.key("state_dir"),
                "must be a non-empty string",
            ));
        }
        None => return Err(Error::schema(&root.key("state_dir"), "is missing")),
    };

    let workers = match map.get("workers") {
        None => 1,
        Some(value) => value.as_u64().filter(|&n| n >= 1).ok_or_else(|| {
            Error::schema(&root.key("workers"), "must be an integer of 1 or more")
        })?,
    };

    let poll_interval = match map.get("poll_interval") {
        None => DEFAULT_POLL_INTERVAL,
        Some(value) => value
            .as_u64()
            .filter(|n| (POLL_INTERVAL.0..=POLL_INTERVAL.1).contains(n))
            .ok_or_else(|| {
                Error::schema(
                    &root.key("poll_interval"),
                    format!(
                        "must be whole seconds from {} to {}, not {value}",
                        POLL_INTERVAL.0, POLL_INTERVAL.1
                    ),
                )
            })?,
    };

    let mut wall_clock = BTreeMap::new();
    if let Some(value) = map.get("wall_clock") {
        let path = root.key("wall_clock");
        let stages = value
            .as_object()
            .ok_or_else(|| Error::schema(&path, "must be a mapping"))?;
        for (name, seconds) in stages {
            let stage_path = path.key(name);
            let stage =
                Stage::from_name(name).ok_or_else(|| Error::schema(&stage_path, "unknown key"))?;
            let limit = seconds
                .as_f64()
                .filter(|s| s.is_finite() && *s > 0.0)
                .and_then(|s| Duration::try_from_secs_f64(s).ok())
                // A value too small for a nanosecond converts to zero, which is not positive.
                .filter(|limit| !limit.is_zero())
                .ok_or_else(|| {
                    Error::schema(&stage_path, "must be a positive number of seconds")
                })?;
            wall_clock.insert(stage, limit);
        }
    }

    Ok(RunnerConfig {
        state_dir,
        workers,
        poll_interval: Duration::from_secs(poll_interval),
        wall_clock,
    })
}

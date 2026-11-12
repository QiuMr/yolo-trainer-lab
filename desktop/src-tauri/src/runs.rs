//! Discovering finished runs on disk.
//!
//! `/state` only knows about the run *this process* started. After a restart it is `idle` with a
//! null `best`, so an AI asked to "score this image with the model you trained yesterday" has no
//! way to find it: the weights are right there in `<output>/<task>/<run>/exports/`, but nothing
//! advertises them. This module reads those directories and reports what is actually usable.
//!
//! Tauri-free on purpose, like `dataset.rs` and `engine.rs` — the window could use it later for a
//! "history" list without a second implementation.
//!
//! ## Why `results.csv` is parsed by column name
//!
//! The header differs per task: a classification run has `metrics/accuracy_top1` and no
//! `metrics/mAP50(B)`; a segmentation run adds `train/mask_loss`. Reading by position returns the
//! wrong number the day the engine inserts a column, and a silently wrong `final_fitness` is worse
//! than no number at all.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;
use serde_json::Value;

/// Upper bound on what `GET /runs` will report. A machine that has been training for a year has
/// hundreds of directories; the useful answer is the recent ones.
pub const MAX_RUNS: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct RunSummary {
    /// Directory name, e.g. `train-1794318289-00005f40`.
    pub name: String,
    /// `detect` / `segment` / `classify`, taken from the run's own resolved config first and the
    /// parent directory name second. The config is authoritative: moving a run directory must not
    /// change what it claims to be.
    pub task: Option<String>,
    pub dir: String,
    /// Directory mtime, seconds since the Unix epoch. Cheaper and more portable than parsing a
    /// timestamp out of `results.csv`, and it is what "most recent" means to a user.
    pub modified_unix: u64,
    /// Full-precision mtime, used only for ordering. Sorting on the second-resolution field made
    /// runs created in the same second fall back to name order, which puts `train-1` after
    /// `train-9`.
    #[serde(skip)]
    modified_at: std::time::SystemTime,
    /// Highest epoch number present in `results.csv`; 0 when the run died before writing one.
    pub epochs_completed: u32,
    pub final_loss: Option<f64>,
    /// Validation fitness of the last completed epoch. `None` when the run used `--no-val` or
    /// never got far enough to validate.
    pub final_fitness: Option<f64>,
    /// `exports/best.bpk`, when the run exported. Pass it to `POST /predict` as `model`.
    pub best: Option<String>,
    pub last: Option<String>,
    /// `checkpoints/last` has all of `manifest.json` / `model.bin` / `optimizer.bin`, so
    /// `POST /train {"init":{"mode":"resume","checkpoint":"<dir>/checkpoints/last"}}` will accept
    /// it. A missing file here is the difference between "resume works" and "clap error".
    pub resumable: bool,
    pub classes: Vec<String>,
}

/// Every run under `root`, newest first.
///
/// A missing or unreadable `root` is not an error: a fresh install has no `runs/` yet, and the
/// honest answer there is an empty list, not a 500.
pub fn scan(root: &Path, limit: usize) -> Vec<RunSummary> {
    let mut found: Vec<RunSummary> = candidates(root)
        .into_iter()
        .filter_map(|(dir, parent)| describe(&dir, parent))
        .collect();
    found.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| a.name.cmp(&b.name))
    });
    found.truncate(limit.clamp(1, MAX_RUNS));
    found
}

/// Directories that look like a run, at both `<root>/<task>/<run>` (the engine's layout) and
/// `<root>/<run>` (what a hand-made `--project` produces).
fn candidates(root: &Path) -> Vec<(PathBuf, Option<String>)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if looks_like_run(&path) {
            out.push((path, None));
            continue;
        }
        let Ok(children) = std::fs::read_dir(&path) else {
            continue;
        };
        let parent = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        for child in children.filter_map(Result::ok) {
            let child_path = child.path();
            if child_path.is_dir() && looks_like_run(&child_path) {
                out.push((child_path, parent.clone()));
            }
        }
    }
    out
}

/// Any one of these is enough. `results.csv` alone catches a run that died in epoch 1; the
/// directories alone catch one killed before the first write.
fn looks_like_run(dir: &Path) -> bool {
    dir.join("results.csv").is_file()
        || dir.join("exports").is_dir()
        || dir.join("checkpoints").is_dir()
        || dir.join("config.resolved.json").is_file()
}

fn describe(dir: &Path, parent: Option<String>) -> Option<RunSummary> {
    let name = dir.file_name()?.to_string_lossy().into_owned();
    let modified_at = dir
        .metadata()
        .ok()
        .and_then(|meta| meta.modified().ok())
        .unwrap_or(UNIX_EPOCH);
    let modified_unix = modified_at
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0);

    let config = read_config(dir);
    let (epochs_completed, final_loss, final_fitness) = read_results(&dir.join("results.csv"));
    let best = file_or_none(dir.join("exports").join("best.bpk"));
    let last = file_or_none(dir.join("exports").join("last.bpk"));
    let checkpoint = dir.join("checkpoints").join("last");
    let resumable = ["manifest.json", "model.bin", "optimizer.bin"]
        .iter()
        .all(|required| checkpoint.join(required).is_file());

    Some(RunSummary {
        name,
        task: config.task.or(parent),
        dir: dir.to_string_lossy().into_owned(),
        modified_unix,
        modified_at,
        epochs_completed,
        final_loss,
        final_fitness,
        best,
        last,
        resumable,
        classes: config.classes,
    })
}

fn file_or_none(path: PathBuf) -> Option<String> {
    path.is_file()
        .then(|| path.to_string_lossy().into_owned())
}

struct RunConfig {
    task: Option<String>,
    classes: Vec<String>,
}

/// `config.resolved.json` is the run's own record of what it trained, which is why it wins over
/// the directory name. `dataset.json` is the fallback for runs written by an older engine.
fn read_config(dir: &Path) -> RunConfig {
    let from_config = read_json(&dir.join("config.resolved.json")).and_then(|value| {
        let model = value.get("model")?;
        let task = model
            .get("task")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let classes = string_list(model.get("class_names"));
        (task.is_some() || !classes.is_empty()).then_some(RunConfig { task, classes })
    });
    if let Some(config) = from_config {
        return config;
    }
    let classes = read_json(&dir.join("dataset.json"))
        .map(|value| string_list(value.get("class_names")))
        .unwrap_or_default();
    RunConfig {
        task: None,
        classes,
    }
}

fn read_json(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// `(highest epoch, last non-empty loss, last non-empty fitness)`.
///
/// Empty cells are skipped rather than read as `0.0`: `--no-val` leaves the metric columns blank,
/// and reporting `fitness: 0` for "not measured" would make a perfectly good model look useless.
fn read_results(path: &Path) -> (u32, Option<f64>, Option<f64>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (0, None, None);
    };
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let Some(header) = lines.next() else {
        return (0, None, None);
    };
    let columns: Vec<&str> = header.split(',').map(str::trim).collect();
    let column = |name: &str| columns.iter().position(|candidate| *candidate == name);

    let epoch_col = column("epoch");
    let loss_col = column("train/loss");
    let fitness_col = column("fitness");

    let mut epochs = 0u32;
    let mut loss = None;
    let mut fitness = None;
    let mut rows = 0u32;
    for line in lines {
        let cells: Vec<&str> = line.split(',').collect();
        rows += 1;
        if let Some(value) = epoch_col
            .and_then(|col| cells.get(col))
            .and_then(|cell| cell.trim().parse::<f64>().ok())
        {
            epochs = epochs.max(value as u32);
        }
        if let Some(col) = loss_col {
            loss = cells
                .get(col)
                .and_then(|cell| cell.trim().parse::<f64>().ok())
                .or(loss);
        }
        if let Some(col) = fitness_col {
            fitness = cells
                .get(col)
                .and_then(|cell| cell.trim().parse::<f64>().ok())
                .or(fitness);
        }
    }
    // A file with rows but no `epoch` column still says how far the run got.
    if epoch_col.is_none() {
        epochs = rows;
    }
    (epochs, loss, fitness)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "yolo-runs-{}-{}-{tag}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn make_run(root: &Path, task: &str, name: &str, csv: &str, with_exports: bool, checkpoint: bool) {
        let dir = root.join(task).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("results.csv"), csv).unwrap();
        std::fs::write(
            dir.join("config.resolved.json"),
            format!(r#"{{"model":{{"task":"{task}","class_names":["a","b"]}}}}"#),
        )
        .unwrap();
        if with_exports {
            std::fs::create_dir_all(dir.join("exports")).unwrap();
            std::fs::write(dir.join("exports").join("best.bpk"), b"x").unwrap();
        }
        if checkpoint {
            let last = dir.join("checkpoints").join("last");
            std::fs::create_dir_all(&last).unwrap();
            for required in ["manifest.json", "model.bin", "optimizer.bin"] {
                std::fs::write(last.join(required), b"x").unwrap();
            }
        }
    }

    #[test]
    fn finds_runs_and_reads_the_metrics_by_name() {
        let root = temp_root("basic");
        make_run(
            &root,
            "detect",
            "train-1",
            "epoch,train/loss,metrics/mAP50(B),fitness,lr/pg0\n\
             1,4.5,0.01,0.02,0.001\n\
             2,3.1,0.03,0.04,0.001\n",
            true,
            true,
        );
        let found = scan(&root, 50);
        assert_eq!(found.len(), 1);
        let run = &found[0];
        assert_eq!(run.name, "train-1");
        assert_eq!(run.task.as_deref(), Some("detect"));
        assert_eq!(run.epochs_completed, 2);
        assert_eq!(run.final_loss, Some(3.1));
        assert_eq!(run.final_fitness, Some(0.04));
        assert!(run.best.is_some());
        assert!(run.resumable);
        assert_eq!(run.classes, ["a", "b"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The classification header has no mAP columns. A positional read would have picked
    /// `metrics/accuracy_top1` and called it loss.
    #[test]
    fn a_different_header_does_not_shift_the_numbers() {
        let root = temp_root("header");
        make_run(
            &root,
            "classify",
            "train-2",
            "epoch,metrics/accuracy_top1,metrics/accuracy_top5,val/loss,fitness\n\
             1,0.5,0.9,1.2,0.5\n",
            false,
            false,
        );
        let run = &scan(&root, 10)[0];
        assert_eq!(run.final_loss, None, "classification rows have no train/loss");
        assert_eq!(run.final_fitness, Some(0.5));
        assert!(!run.resumable);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `--no-val` leaves the fitness cell blank. Reporting 0.0 would call the model worthless.
    #[test]
    fn blank_metric_cells_stay_none() {
        let root = temp_root("blank");
        make_run(
            &root,
            "detect",
            "train-3",
            "epoch,train/loss,fitness\n1,9.0,\n",
            false,
            false,
        );
        let run = &scan(&root, 10)[0];
        assert_eq!(run.final_loss, Some(9.0));
        assert_eq!(run.final_fitness, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn newest_first_and_limit_applies() {
        let root = temp_root("order");
        // Created oldest-first with a real gap: on Windows a directory's mtime cannot be set
        // through `File`, and the ordering test would otherwise depend on timestamp granularity.
        for name in ["train-a", "train-b", "train-c"] {
            make_run(&root, "detect", name, "epoch,train/loss\n1,1.0\n", false, false);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let all = scan(&root, 10);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].name, "train-c", "newest first");
        assert_eq!(scan(&root, 2).len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A fresh install has no `runs/` at all. That is an empty list, not an error.
    #[test]
    fn a_missing_root_is_empty_not_an_error() {
        assert!(scan(Path::new("C:/definitely/not/here/runs"), 10).is_empty());
    }

    /// `--project` can point straight at a directory of runs with no task level.
    #[test]
    fn runs_directly_under_the_root_are_found() {
        let root = temp_root("flat");
        let dir = root.join("train-flat");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("results.csv"), "epoch,train/loss\n1,2.0\n").unwrap();
        let run = &scan(&root, 10)[0];
        assert_eq!(run.name, "train-flat");
        assert!(run.task.is_none());
        assert_eq!(run.epochs_completed, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Not part of the normal suite: parses whatever this machine actually produced. The fixtures
    /// above encode what I *believed* the engine writes; this one checks the belief.
    ///
    /// ```text
    /// cargo test --lib real_runs_directory -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "reads this machine's real runs/ directory"]
    fn real_runs_directory_parses() {
        let Ok(appdata) = std::env::var("APPDATA") else {
            return;
        };
        let root = Path::new(&appdata)
            .join("com.yolotrainer.desktop")
            .join("runs");
        if !root.is_dir() {
            println!("no runs directory at {}", root.display());
            return;
        }
        for run in scan(&root, 20) {
            println!(
                "{:<28} task={:<9} epochs={:<3} loss={:<10} fitness={:<10} best={:<5} \
                 resumable={:<5} classes={:?}",
                run.name,
                run.task.as_deref().unwrap_or("-"),
                run.epochs_completed,
                run.final_loss.map(|v| format!("{v:.4}")).unwrap_or_else(|| "-".into()),
                run.final_fitness.map(|v| format!("{v:.4}")).unwrap_or_else(|| "-".into()),
                run.best.is_some(),
                run.resumable,
                run.classes,
            );
        }
    }
}

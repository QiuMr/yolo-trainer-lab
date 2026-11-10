//! Dataset discovery: read a `dataset.yaml` and report what is actually on disk.
//!
//! Hand-written rather than pulled from ultralytics because the UI only needs a handful of fields
//! and this must work with no Python anywhere near the machine.
//!
//! Two layouts are understood, matching the two the app can train:
//!
//! - `yolo` — `images/train/*.jpg` with sibling `labels/*.txt` (detect and instance segmentation)
//! - `imagenet-folders` — `train/<class>/*.jpg` with the class name carried by the directory
//!
//! COCO is deliberately rejected with a message rather than half-supported: the engine can read it,
//! this scanner cannot yet, and a dataset page that shows plausible numbers for a layout it does not
//! really parse is worse than one that says no.
//!
//! Also Tauri-free: this is the operation an AI-driven service would call to answer "is this
//! dataset usable before I spend an hour training on it".

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const IMAGE_EXTENSIONS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

#[derive(Debug, Deserialize, Default)]
struct Manifest {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    train: Option<String>,
    #[serde(default)]
    val: Option<String>,
    #[serde(default)]
    names: Option<serde_yaml::Value>,
    /// Optional in every case: omitting it lets the layout be sniffed from the files on disk.
    #[serde(default)]
    format: Option<String>,
}

/// Which on-disk layout the manifest describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Yolo,
    ClassificationFolders,
}

impl Layout {
    /// The string the frontend keys on. Not the same as the YAML spelling, which is what the
    /// engine accepts; this one is a product-level name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yolo => "yolo",
            Self::ClassificationFolders => "imagenet-folders",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DatasetSummary {
    /// Absolute root the splits resolve against, not necessarily the folder the user picked.
    pub root: String,
    /// Absolute path of the manifest the splits were read from.
    ///
    /// `root` cannot stand in for this: when the manifest sets `path:` to another directory, the
    /// yaml lives next to the manifest, not at the data root. Training needs the yaml, so the
    /// engine is handed this field.
    pub manifest: String,
    pub classes: Vec<String>,
    pub train_images: usize,
    pub val_images: usize,
    /// Images without a label file. Always 0 for classification, where the folder is the label.
    pub missing_labels: usize,
    pub annotated: usize,
    /// Training images per class, in `classes` order. Empty for layouts where a per-class count
    /// would mean parsing every label file (YOLO), which this scanner deliberately does not do.
    /// For classification the directory listing makes it free, and imbalance is the first thing
    /// that explains a model that will not learn — so it is worth showing.
    pub class_counts: Vec<usize>,
    pub format: String,
    /// Set when the dataset is usable but has a problem worth showing, e.g. no validation split.
    pub warning: Option<String>,
}

fn is_image(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

fn count_images(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| is_image(&entry.path()))
        .count()
}

/// Count images anywhere under `dir`. Needed for class folders, where a user may nest an extra
/// level (`train/cat/2024/x.jpg`) and the engine's own loader recurses too.
fn count_images_recursive(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            total += count_images_recursive(&path);
        } else if is_image(&path) {
            total += 1;
        }
    }
    total
}

fn subdirectories(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect();
    names.sort();
    names
}

/// Count label files, and how many images have none.
///
/// Ultralytics layout puts `images/<split>/x.jpg` next to `labels/<split>/x.txt`. The sibling is
/// resolved by swapping the `images` component rather than assuming a fixed depth, because real
/// datasets nest differently almost every time.
fn count_labels(image_dir: &Path) -> (usize, usize) {
    let Ok(entries) = std::fs::read_dir(image_dir) else {
        return (0, 0);
    };
    let label_dir = {
        let mut path = image_dir.to_path_buf();
        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        path.pop();
        if path.file_name().map(|v| v.to_string_lossy().into_owned()) == Some("images".to_owned()) {
            path.pop();
            path.join("labels").join(name)
        } else {
            image_dir.join("labels")
        }
    };

    let mut annotated = 0;
    let mut missing = 0;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(stem) = path.file_stem() else {
            continue;
        };
        if label_dir.join(stem).with_extension("txt").is_file() {
            annotated += 1;
        } else {
            missing += 1;
        }
    }
    (annotated, missing)
}

/// Accept both spellings Ultralytics documents: a mapping of index to name, or a plain list.
fn class_names(names: Option<serde_yaml::Value>) -> Vec<String> {
    match names {
        Some(serde_yaml::Value::Sequence(items)) => items
            .into_iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        Some(serde_yaml::Value::Mapping(map)) => {
            let mut pairs: Vec<(u64, String)> = map
                .into_iter()
                .filter_map(|(key, value)| Some((key.as_u64()?, value.as_str()?.to_owned())))
                .collect();
            pairs.sort_by_key(|(index, _)| *index);
            pairs.into_iter().map(|(_, name)| name).collect()
        }
        _ => Vec::new(),
    }
}

/// Fields a layout-specific scanner produces; the caller adds the paths.
struct Counts {
    classes: Vec<String>,
    train_images: usize,
    val_images: usize,
    missing_labels: usize,
    annotated: usize,
    class_counts: Vec<usize>,
    warning: Option<String>,
}

/// Decide the layout from the files when the manifest does not say.
///
/// A YOLO split puts images directly in the split directory; a classification split puts only
/// class folders there. That single difference is enough, and it avoids guessing from `names`.
fn sniff_layout(train_dir: &Path) -> Layout {
    if count_images(train_dir) > 0 {
        return Layout::Yolo;
    }
    let has_class_images = subdirectories(train_dir)
        .iter()
        .any(|name| count_images_recursive(&train_dir.join(name)) > 0);
    if has_class_images {
        Layout::ClassificationFolders
    } else {
        // Nothing to learn from: keep the YOLO path, whose error message is the more actionable
        // one for the far more common case.
        Layout::Yolo
    }
}

fn scan_yolo(train_dir: &Path, val_dir: &Path, names: Option<serde_yaml::Value>) -> Result<Counts, String> {
    let train_images = count_images(train_dir);
    if train_images == 0 {
        return Err(format!(
            "训练集里没有图片：{}。请检查 dataset.yaml 的 train 指向。",
            train_dir.display()
        ));
    }
    let val_images = count_images(val_dir);
    let (annotated, missing_labels) = count_labels(train_dir);
    let classes = class_names(names);

    if classes.is_empty() {
        return Err("dataset.yaml 里没有 names，无法确定类别。".into());
    }

    let warning = if val_images == 0 {
        Some("没有验证集，训练时会跳过 mAP 计算。".to_owned())
    } else if missing_labels > 0 {
        Some(format!("有 {missing_labels} 张图缺标注。"))
    } else {
        None
    };

    Ok(Counts {
        classes,
        train_images,
        val_images,
        missing_labels,
        annotated,
        class_counts: Vec::new(),
        warning,
    })
}

/// ImageNet-style split: `train/<class>/*.jpg`, with the class name stored as a directory name.
///
/// `names` may be omitted — the engine infers the classes from the directory listing — but when it
/// is present it must match the directories exactly, because the engine resolves
/// `split.join(name)`. Checking that here turns a mid-run failure into a page-level message.
fn scan_classification(
    train_dir: &Path,
    val_dir: &Path,
    names: Option<serde_yaml::Value>,
) -> Result<Counts, String> {
    let on_disk = subdirectories(train_dir);
    let declared = class_names(names);
    let classes = if declared.is_empty() {
        on_disk.clone()
    } else {
        declared
    };
    if classes.is_empty() {
        return Err(format!(
            "分类数据集的训练目录下没有类别文件夹：{}。每个类别应是一个文件夹，例如 train/cat/*.jpg。",
            train_dir.display()
        ));
    }

    let mut train_images = 0;
    let mut missing_dirs = Vec::new();
    let mut class_counts = Vec::with_capacity(classes.len());
    for class in &classes {
        let directory = train_dir.join(class);
        if !directory.is_dir() {
            missing_dirs.push(class.clone());
            class_counts.push(0);
            continue;
        }
        let count = count_images_recursive(&directory);
        train_images += count;
        class_counts.push(count);
    }
    if !missing_dirs.is_empty() {
        return Err(format!(
            "这些类别在 {} 下没有对应文件夹：{}。分类数据集的类别名必须与文件夹名完全一致。",
            train_dir.display(),
            missing_dirs.join("、")
        ));
    }
    if train_images == 0 {
        return Err(format!("训练集里没有图片：{}。", train_dir.display()));
    }

    let val_images: usize = classes
        .iter()
        .map(|class| count_images_recursive(&val_dir.join(class)))
        .sum();

    let extra: Vec<String> = on_disk
        .into_iter()
        .filter(|name| !classes.contains(name))
        .collect();
    let warning = if !extra.is_empty() {
        Some(format!(
            "train 下有未列进 names 的文件夹（不会被训练）：{}。",
            extra.join("、")
        ))
    } else if val_images == 0 {
        Some("没有验证集，训练时会跳过准确率统计。".to_owned())
    } else {
        None
    };

    Ok(Counts {
        annotated: train_images,
        missing_labels: 0,
        classes,
        train_images,
        val_images,
        class_counts,
        warning,
    })
}

/// Read `dataset.yaml` (or a directory containing one) and summarise its splits.
///
/// Accepts a directory or the yaml itself, because both a drag-and-drop and a file picker can
/// produce either one.
pub fn scan(path: &str) -> Result<DatasetSummary, String> {
    let root = PathBuf::from(path);
    let manifest_path = if root.is_file() {
        root.clone()
    } else {
        root.join("dataset.yaml")
    };
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("读不到 {}：{error}", manifest_path.display()))?;
    let manifest: Manifest =
        serde_yaml::from_str(&text).map_err(|error| format!("dataset.yaml 格式有误：{error}"))?;

    let base = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.clone());
    let data_root = match manifest.path.as_deref() {
        Some(relative) if Path::new(relative).is_absolute() => PathBuf::from(relative),
        Some(relative) => base.join(relative),
        None => base.clone(),
    };

    let resolve = |split: Option<&str>, fallback: &str| -> PathBuf {
        let value = split.unwrap_or(fallback);
        let candidate = data_root.join(value);
        if candidate.exists() {
            candidate
        } else {
            // Some manifests already point at absolute directories.
            PathBuf::from(value)
        }
    };

    let train_dir = resolve(manifest.train.as_deref(), "images/train");
    let val_dir = resolve(manifest.val.as_deref(), "images/val");

    let layout = match manifest.format.as_deref() {
        Some("yolo") => Layout::Yolo,
        Some("classification-folders") | Some("imagenet-folders") | Some("imagenet") => {
            Layout::ClassificationFolders
        }
        Some("coco") => {
            return Err(
                "COCO 数据集暂不支持：本应用目前只认 YOLO 标签（images/ + labels/）和按类别分文件夹两种结构。"
                    .into(),
            )
        }
        Some(other) => {
            return Err(format!(
                "不认识的 format：{other}。可用 yolo 或 classification-folders，或删掉这行让程序自行判断。"
            ))
        }
        None => sniff_layout(&train_dir),
    };

    let counts = match layout {
        Layout::Yolo => scan_yolo(&train_dir, &val_dir, manifest.names)?,
        Layout::ClassificationFolders => {
            scan_classification(&train_dir, &val_dir, manifest.names)?
        }
    };

    // `std::path::absolute` rather than `fs::canonicalize`: the latter returns a `\\?\` verbatim
    // path on Windows, which would be shown in the UI and handed to the engine as `--data`.
    // `absolute` resolves `.` and `..` without touching the filesystem, which is all that is
    // needed here.
    let absolute = |path: &Path| {
        std::path::absolute(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .into_owned()
    };

    Ok(DatasetSummary {
        root: absolute(&data_root),
        manifest: absolute(&manifest_path),
        classes: counts.classes,
        train_images: counts.train_images,
        val_images: counts.val_images,
        missing_labels: counts.missing_labels,
        annotated: counts.annotated,
        class_counts: counts.class_counts,
        format: layout.as_str().to_owned(),
        warning: counts.warning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Option<PathBuf> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("spikes")
            .join("tiny-dataset");
        root.join("dataset.yaml").is_file().then_some(root)
    }

    fn classify_fixture() -> Option<PathBuf> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("spikes")
            .join("tiny-classify");
        root.join("dataset.yaml").is_file().then_some(root)
    }

    /// The scanner is what the dataset page shows, so a wrong count is a wrong UI.
    #[test]
    fn scans_the_tiny_dataset() {
        let Some(root) = fixture() else {
            return;
        };
        let summary = scan(&root.to_string_lossy()).unwrap();
        assert_eq!(summary.train_images, 4);
        assert_eq!(summary.val_images, 2);
        assert_eq!(summary.classes, vec!["dog", "person", "bike"]);
        assert_eq!(summary.missing_labels, 0);
        assert_eq!(summary.annotated, 4);
        assert_eq!(summary.format, "yolo");
        assert!(summary.warning.is_none());
        assert!(summary.manifest.ends_with("dataset.yaml"));
    }

    /// Classification had no fixture at all before, so the whole layout was untested.
    #[test]
    fn scans_the_classification_fixture() {
        let Some(root) = classify_fixture() else {
            return;
        };
        let summary = scan(&root.to_string_lossy()).unwrap();
        assert_eq!(summary.format, "imagenet-folders");
        assert_eq!(summary.classes, vec!["circle", "square", "triangle"]);
        assert_eq!(summary.train_images, 18);
        assert_eq!(summary.val_images, 6);
        // No label files exist in this layout, so "missing labels" must not leak a non-zero count
        // into the UI — the class folder *is* the label.
        assert_eq!(summary.missing_labels, 0);
        assert_eq!(summary.annotated, 18);
        assert_eq!(summary.class_counts, vec![6, 6, 6]);
        assert!(summary.warning.is_none());
    }

    /// `names` wins over the directory listing, so a name without a folder has to fail loudly:
    /// the engine resolves `split.join(name)` and would otherwise die mid-run.
    #[test]
    fn rejects_names_without_matching_folder() {
        let Some(root) = classify_fixture() else {
            return;
        };
        let temporary = std::env::temp_dir().join("dataset-scan-name-mismatch");
        let _ = std::fs::remove_dir_all(&temporary);
        std::fs::create_dir_all(&temporary).unwrap();
        for class in ["circle", "square"] {
            std::fs::create_dir_all(temporary.join("train").join(class)).unwrap();
            std::fs::write(
                temporary.join("train").join(class).join("a.png"),
                b"not-a-real-png-but-the-name-is-what-matters",
            )
            .unwrap();
        }
        std::fs::write(
            temporary.join("dataset.yaml"),
            "path: .\ntrain: train\nnames:\n  - circle\n  - ghost\n",
        )
        .unwrap();

        let error = scan(&temporary.to_string_lossy()).unwrap_err();
        assert!(error.contains("ghost"), "{error}");
        let _ = std::fs::remove_dir_all(&temporary);
        drop(root);
    }

    /// A directory without `dataset.yaml` must fail with a message, not panic or return zeros.
    #[test]
    fn rejects_a_directory_without_manifest() {
        assert!(scan(&std::env::temp_dir().to_string_lossy()).is_err());
    }

    /// Both YAML shapes are documented by Ultralytics; users copy either one.
    #[test]
    fn reads_class_names_in_both_yaml_shapes() {
        let mapping: serde_yaml::Value = serde_yaml::from_str("0: dog\n1: person").unwrap();
        assert_eq!(class_names(Some(mapping)), vec!["dog", "person"]);

        let list: serde_yaml::Value = serde_yaml::from_str("- cat\n- bird").unwrap();
        assert_eq!(class_names(Some(list)), vec!["cat", "bird"]);
    }
}

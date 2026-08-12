use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use bumper::bump::{TypedChange, VersionBump, preview_typed_change};
use bumper::package::Package;

use crate::Target;
use crate::model::AppResult;
use crate::release_plan::{
    Release, ReleasePlan, package_files, package_label, resolve_known_package_owner,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BumpPreview {
    pub files: BTreeMap<PathBuf, PreviewFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewFile {
    pub version: Option<VersionBump>,
    pub dependencies: BTreeMap<PathBuf, DependencyFileChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyFileChange {
    pub names: BTreeSet<String>,
}

#[derive(Default)]
struct PreviewNode {
    children: BTreeMap<OsString, PreviewNode>,
    file: bool,
}

struct PreviewRenderer<'a> {
    plan: &'a ReleasePlan,
    preview: &'a BumpPreview,
    package_colors: BTreeMap<PathBuf, &'static str>,
    color: bool,
}

pub struct PreviewInput<'a> {
    pub repo_root: &'a Path,
    pub plan: &'a ReleasePlan,
    pub targets: &'a [Target],
    pub known_packages: &'a BTreeMap<PathBuf, Package>,
    pub tracked_files: &'a [PathBuf],
    pub tracked_paths: &'a HashSet<PathBuf>,
}

pub fn collect_bump_preview(input: PreviewInput<'_>) -> AppResult<BumpPreview> {
    let mut preview = BumpPreview::default();

    for release in input.plan.releases.values() {
        for target in input
            .targets
            .iter()
            .filter(|target| target.package_path == release.package.path)
        {
            if target.path.is_file() {
                if preview_file_change(&target.path, release, true)? {
                    add_version_file(&mut preview, input.repo_root, &target.path, release)?;
                }
                continue;
            }

            for file in input
                .tracked_files
                .iter()
                .filter(|file| file.starts_with(&target.path))
            {
                if !file.is_file() {
                    continue;
                }
                let owner =
                    resolve_known_package_owner(input.repo_root, file, input.known_packages)?;
                if owner.path != release.package.path {
                    continue;
                }
                if preview_file_change(file, release, false)? {
                    add_version_file(&mut preview, input.repo_root, file, release)?;
                }
            }
        }

        for file in package_files(&release.package.root, input.tracked_paths) {
            if preview_file_change(&file, release, false)? {
                add_version_file(&mut preview, input.repo_root, &file, release)?;
            }
        }
    }

    for (file, planned) in &input.plan.dependency_files {
        let entry = preview
            .files
            .entry(file.clone())
            .or_insert_with(|| PreviewFile {
                version: None,
                dependencies: BTreeMap::new(),
            });
        for (producer, names) in &planned.dependencies {
            entry.dependencies.insert(
                producer.clone(),
                DependencyFileChange {
                    names: names.clone(),
                },
            );
        }
    }

    Ok(preview)
}

fn preview_file_change(file: &Path, release: &Release, replace_unhandled: bool) -> AppResult<bool> {
    match preview_typed_change(file, &release.old_version, &release.new_version)? {
        TypedChange::Changed => Ok(true),
        TypedChange::Unchanged => Ok(false),
        TypedChange::Unhandled if !replace_unhandled => Ok(false),
        TypedChange::Unhandled => {
            let source = fs::read_to_string(file)
                .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
            if source.contains(&release.old_version) {
                Ok(true)
            } else {
                Err(format!("no occurrences found in {}", file.display()))
            }
        }
    }
}

fn add_version_file(
    preview: &mut BumpPreview,
    repo_root: &Path,
    file: &Path,
    release: &Release,
) -> AppResult<()> {
    let relative = repository_relative(repo_root, file)?;
    let entry = preview
        .files
        .entry(relative)
        .or_insert_with(|| PreviewFile {
            version: None,
            dependencies: BTreeMap::new(),
        });
    entry.version = Some(VersionBump {
        old_version: release.old_version.clone(),
        new_version: release.new_version.clone(),
    });
    Ok(())
}

fn repository_relative(repo_root: &Path, file: &Path) -> AppResult<PathBuf> {
    file.strip_prefix(repo_root)
        .map(Path::to_path_buf)
        .map_err(|_| {
            format!(
                "file '{}' is outside repository root '{}'",
                file.display(),
                repo_root.display()
            )
        })
}

pub fn preview_colors_enabled(
    stdout_is_terminal: bool,
    no_color: Option<&OsStr>,
    term: Option<&OsStr>,
) -> bool {
    cfg!(not(windows))
        && stdout_is_terminal
        && no_color.is_none()
        && term
            .and_then(OsStr::to_str)
            .is_none_or(|term| !term.eq_ignore_ascii_case("dumb"))
}

pub fn render_bump_preview(plan: &ReleasePlan, preview: &BumpPreview, color: bool) -> String {
    const PACKAGE_COLORS: &[&str] = &[
        "\x1b[36m", "\x1b[33m", "\x1b[32m", "\x1b[35m", "\x1b[34m", "\x1b[31m", "\x1b[96m",
        "\x1b[93m", "\x1b[92m", "\x1b[95m", "\x1b[94m", "\x1b[91m",
    ];

    let package_colors = plan
        .releases
        .keys()
        .cloned()
        .enumerate()
        .map(|(index, path)| (path, PACKAGE_COLORS[index % PACKAGE_COLORS.len()]))
        .collect::<BTreeMap<_, _>>();
    let mut root = PreviewNode::default();
    for path in plan.releases.keys() {
        let mut node = &mut root;
        for component in path.components() {
            node = node
                .children
                .entry(component.as_os_str().to_os_string())
                .or_default();
        }
    }
    for path in preview.files.keys() {
        let mut node = &mut root;
        for component in path.components() {
            node = node
                .children
                .entry(component.as_os_str().to_os_string())
                .or_default();
        }
        node.file = true;
    }

    let renderer = PreviewRenderer {
        plan,
        preview,
        package_colors,
        color,
    };
    let mut output = String::from("Release plan:\n");
    let root_label = node_label(".", Path::new(""), None, plan);
    write_node_name(
        &mut output,
        &root_label,
        renderer.package_colors.get(Path::new("")).copied(),
        color,
    );
    output.push('\n');
    renderer.render_node_contents(&root, "", Path::new(""), &mut output);
    output
}

impl PreviewRenderer<'_> {
    fn render_node_contents(
        &self,
        node: &PreviewNode,
        prefix: &str,
        path: &Path,
        output: &mut String,
    ) {
        let reasons = self
            .plan
            .releases
            .get(path)
            .map(release_reason_labels)
            .unwrap_or_default();
        let total = reasons.len() + node.children.len();
        let package_color = nearest_package_color(path, &self.package_colors);

        for (index, reason) in reasons.iter().enumerate() {
            let last = index + 1 == total;
            let connector = if last { "`-- " } else { "|-- " };
            let _ = write!(output, "{prefix}{connector}");
            write_node_name(output, reason, package_color, self.color);
            output.push('\n');
        }

        for (child_index, (name, child)) in node.children.iter().enumerate() {
            let index = reasons.len() + child_index;
            let last = index + 1 == total;
            let connector = if last { "`-- " } else { "|-- " };
            let child_path = path.join(name);
            let child_color = nearest_package_color(&child_path, &self.package_colors);
            let file = child.file.then(|| {
                self.preview
                    .files
                    .get(&child_path)
                    .expect("preview tree file came from preview files")
            });
            let label = node_label(&name.to_string_lossy(), &child_path, file, self.plan);
            let _ = write!(output, "{prefix}{connector}");
            write_node_name(output, &label, child_color, self.color);
            output.push('\n');

            let child_prefix = format!("{prefix}{}", if last { "    " } else { "|   " });
            self.render_node_contents(child, &child_prefix, &child_path, output);
        }
    }
}

fn node_label(name: &str, path: &Path, file: Option<&PreviewFile>, plan: &ReleasePlan) -> String {
    let mut label = name.to_string();
    if let Some(release) = plan.releases.get(path) {
        let _ = write!(
            label,
            " ({} -> {}, {})",
            release.old_version,
            release.new_version,
            release.impact.as_str()
        );
    }
    if let Some(file) = file {
        let changes = file_change_labels(file, plan);
        if !changes.is_empty() {
            let _ = write!(label, " ({})", changes.join("; "));
        }
    }
    label
}

fn file_change_labels(file: &PreviewFile, plan: &ReleasePlan) -> Vec<String> {
    let mut changes = Vec::new();
    if let Some(version) = &file.version {
        changes.push(format!(
            "{} -> {}",
            version.old_version, version.new_version
        ));
    }
    for dependency in file.dependencies.values() {
        for name in &dependency.names {
            let bump = &plan
                .named_bumps
                .get(name)
                .expect("preview dependency name came from named bumps")
                .version;
            changes.push(format!(
                "{name} {} -> {}",
                bump.old_version, bump.new_version
            ));
        }
    }
    changes
}

fn release_reason_labels(release: &Release) -> Vec<String> {
    let mut labels = Vec::new();
    if let Some(impact) = release.reasons.direct {
        labels.push(format!("direct {}", impact.as_str()));
    }
    if release.reasons.forced {
        labels.push("forced patch (--force)".to_string());
    }
    labels.extend(
        release
            .reasons
            .descendants
            .iter()
            .map(|path| format!("propagated from child {}", package_label(path))),
    );
    labels.extend(
        release
            .reasons
            .dependencies
            .iter()
            .map(|(producer, cause)| {
                let names = cause.names.iter().cloned().collect::<Vec<_>>().join(", ");
                let manifests = cause
                    .manifests
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "propagated from dependency {} ({names} via {manifests})",
                    package_label(producer)
                )
            }),
    );
    labels
}

fn nearest_package_color<'a>(
    path: &Path,
    package_colors: &'a BTreeMap<PathBuf, &'a str>,
) -> Option<&'a str> {
    package_colors
        .iter()
        .filter(|(package_path, _)| {
            package_path.as_os_str().is_empty() || path.starts_with(package_path)
        })
        .max_by_key(|(package_path, _)| package_path.components().count())
        .map(|(_, color)| *color)
}

fn write_node_name(output: &mut String, label: &str, ansi: Option<&str>, color: bool) {
    if color && let Some(ansi) = ansi {
        let _ = write!(output, "{ansi}{label}\x1b[0m");
    } else {
        output.push_str(label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Impact;
    use crate::release_plan::{DependencyCause, NamedPackageBump, ReleaseReasons};

    fn release(
        path: &str,
        old_version: &str,
        new_version: &str,
        impact: Impact,
        reasons: ReleaseReasons,
    ) -> Release {
        Release {
            package: Package {
                root: PathBuf::from(path),
                path: PathBuf::from(path),
            },
            last_tag: String::new(),
            old_version: old_version.to_string(),
            new_version: new_version.to_string(),
            impact,
            tag: String::new(),
            reasons,
        }
    }

    #[test]
    fn release_plan_tree_renders_propagation_and_file_changes() {
        let root_path = PathBuf::new();
        let app_path = PathBuf::from("packages/app");
        let library_path = PathBuf::from("packages/library");
        let plan = ReleasePlan {
            releases: BTreeMap::from([
                (
                    root_path.clone(),
                    release(
                        "",
                        "1.0.0",
                        "1.0.1",
                        Impact::Patch,
                        ReleaseReasons {
                            descendants: BTreeSet::from([library_path.clone()]),
                            ..ReleaseReasons::default()
                        },
                    ),
                ),
                (
                    app_path.clone(),
                    release(
                        "packages/app",
                        "3.0.0",
                        "3.0.1",
                        Impact::Patch,
                        ReleaseReasons {
                            dependencies: BTreeMap::from([(
                                library_path.clone(),
                                DependencyCause {
                                    names: BTreeSet::from(["library".to_string()]),
                                    manifests: BTreeSet::from([PathBuf::from(
                                        "packages/app/package.json",
                                    )]),
                                },
                            )]),
                            ..ReleaseReasons::default()
                        },
                    ),
                ),
                (
                    library_path.clone(),
                    release(
                        "packages/library",
                        "2.0.0",
                        "2.0.1",
                        Impact::Patch,
                        ReleaseReasons {
                            direct: Some(Impact::Patch),
                            ..ReleaseReasons::default()
                        },
                    ),
                ),
            ]),
            skipped_packages: BTreeSet::new(),
            named_bumps: BTreeMap::from([(
                "library".to_string(),
                NamedPackageBump {
                    package_path: library_path.clone(),
                    version: VersionBump {
                        old_version: "2.0.0".to_string(),
                        new_version: "2.0.1".to_string(),
                    },
                },
            )]),
            dependency_files: BTreeMap::new(),
        };
        let preview = BumpPreview {
            files: BTreeMap::from([
                (
                    PathBuf::from("package.json"),
                    PreviewFile {
                        version: Some(VersionBump {
                            old_version: "1.0.0".to_string(),
                            new_version: "1.0.1".to_string(),
                        }),
                        dependencies: BTreeMap::new(),
                    },
                ),
                (
                    PathBuf::from("packages/app/package.json"),
                    PreviewFile {
                        version: Some(VersionBump {
                            old_version: "3.0.0".to_string(),
                            new_version: "3.0.1".to_string(),
                        }),
                        dependencies: BTreeMap::from([(
                            library_path.clone(),
                            DependencyFileChange {
                                names: BTreeSet::from(["library".to_string()]),
                            },
                        )]),
                    },
                ),
                (
                    PathBuf::from("packages/library/package.json"),
                    PreviewFile {
                        version: Some(VersionBump {
                            old_version: "2.0.0".to_string(),
                            new_version: "2.0.1".to_string(),
                        }),
                        dependencies: BTreeMap::new(),
                    },
                ),
            ]),
        };

        assert_eq!(
            render_bump_preview(&plan, &preview, false),
            concat!(
                "Release plan:\n",
                ". (1.0.0 -> 1.0.1, patch)\n",
                "|-- propagated from child packages/library\n",
                "|-- package.json (1.0.0 -> 1.0.1)\n",
                "`-- packages\n",
                "    |-- app (3.0.0 -> 3.0.1, patch)\n",
                "    |   |-- propagated from dependency packages/library (library via packages/app/package.json)\n",
                "    |   `-- package.json (3.0.0 -> 3.0.1; library 2.0.0 -> 2.0.1)\n",
                "    `-- library (2.0.0 -> 2.0.1, patch)\n",
                "        |-- direct patch\n",
                "        `-- package.json (2.0.0 -> 2.0.1)\n",
            )
        );
    }

    #[test]
    fn release_plan_tree_keeps_tag_only_packages_and_plain_semantics_with_color() {
        let package_path = PathBuf::from("packages/service");
        let plan = ReleasePlan {
            releases: BTreeMap::from([(
                package_path.clone(),
                release(
                    "packages/service",
                    "2.3.4",
                    "2.3.5",
                    Impact::Patch,
                    ReleaseReasons {
                        direct: Some(Impact::Patch),
                        forced: true,
                        ..ReleaseReasons::default()
                    },
                ),
            )]),
            skipped_packages: BTreeSet::new(),
            named_bumps: BTreeMap::new(),
            dependency_files: BTreeMap::new(),
        };

        let plain = render_bump_preview(&plan, &BumpPreview::default(), false);
        let colored = render_bump_preview(&plan, &BumpPreview::default(), true);

        assert!(plain.contains("service (2.3.4 -> 2.3.5, patch)"));
        assert!(plain.contains("direct patch"));
        assert!(plain.contains("forced patch (--force)"));
        assert_eq!(
            colored
                .replace("\x1b[33m", "")
                .replace("\x1b[36m", "")
                .replace("\x1b[0m", ""),
            plain
        );
    }
}

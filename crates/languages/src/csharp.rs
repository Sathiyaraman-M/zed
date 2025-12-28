use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use collections::HashMap;
use futures::StreamExt;
use gpui::{App, AppContext, AsyncApp, Task};
use http_client::github::{AssetKind, GitHubLspBinaryVersion, latest_github_release};
use http_client::github_download::{GithubBinaryMetadata, download_server_binary};
pub use language::*;
use language::{LspAdapter, LspAdapterDelegate, LspInstaller, Toolchain};
use lsp::{LanguageServerBinary, LanguageServerName, Uri};
use project::lsp_store::language_server_settings;
use smol::fs;
use std::borrow::Cow;
use std::{
    env::consts,
    path::{Path, PathBuf},
    sync::Arc,
};
use task::{
    HideStrategy, RevealStrategy, RevealTarget, TaskTemplate, TaskTemplates, TaskVariables,
    VariableName,
};
use util::{ResultExt, fs::remove_matching, maybe};

pub struct CsharpLspAdapter;

impl CsharpLspAdapter {
    const SERVER_NAME: LanguageServerName = LanguageServerName::new_static("roslyn");
}

impl LspInstaller for CsharpLspAdapter {
    type BinaryVersion = GitHubLspBinaryVersion;

    async fn fetch_latest_server_version(
        &self,
        delegate: &dyn LspAdapterDelegate,
        pre_release: bool,
        _: &mut AsyncApp,
    ) -> Result<Self::BinaryVersion> {
        let release = latest_github_release(
            "SofusA/csharp-language-server",
            true,
            pre_release,
            delegate.http_client(),
        )
        .await?;

        let arch_str = match consts::ARCH {
            "aarch64" => "aarch64",
            "x86_64" => "x86_64",
            other => bail!("unsupported architecture: {other}"),
        };

        let os_str = match consts::OS {
            "macos" => "apple-darwin",
            "linux" => "unknown-linux-gnu",
            "windows" => "pc-windows-msvc",
            other => bail!("Running on unsupported os: {other}"),
        };

        let ext = if consts::OS == "windows" {
            "zip"
        } else {
            "tar.gz"
        };

        let asset_name = format!("csharp-language-server-{}-{}.{}", arch_str, os_str, ext);
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .with_context(|| format!("no asset found matching `{asset_name:?}`"))?;

        Ok(GitHubLspBinaryVersion {
            name: release.tag_name,
            url: asset.browser_download_url.clone(),
            digest: asset.digest.clone(),
        })
    }

    async fn check_if_user_installed(
        &self,
        delegate: &dyn LspAdapterDelegate,
        _: Option<Toolchain>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        let path = delegate.which("csharp-language-server".as_ref()).await?;
        Some(LanguageServerBinary {
            path,
            arguments: Default::default(),
            env: None,
        })
    }

    async fn fetch_server_binary(
        &self,
        version: GitHubLspBinaryVersion,
        container_dir: PathBuf,
        delegate: &dyn LspAdapterDelegate,
    ) -> Result<LanguageServerBinary> {
        let GitHubLspBinaryVersion {
            name,
            url,
            digest: expected_digest,
        } = version;
        let version_dir = container_dir.join(format!("roslyn-{}", name));
        let binary_name = if cfg!(target_os = "windows") {
            format!("csharp-language-server{}", std::env::consts::EXE_SUFFIX)
        } else {
            "csharp-language-server".to_string()
        };
        let binary_path = version_dir.join(&binary_name);

        let metadata_path = version_dir.join("metadata");
        let metadata = GithubBinaryMetadata::read_from_file(&metadata_path)
            .await
            .ok();
        if let Some(metadata) = metadata {
            let validity_check = async || {
                delegate
                    .try_exec(LanguageServerBinary {
                        path: binary_path.clone(),
                        arguments: vec!["--version".into()],
                        env: None,
                    })
                    .await
                    .inspect_err(|err| {
                        log::warn!("Unable to run {binary_path:?} asset, redownloading: {err:#}",)
                    })
            };
            if let (Some(actual_digest), Some(expected_digest)) =
                (&metadata.digest, &expected_digest)
            {
                if actual_digest == expected_digest {
                    if validity_check().await.is_ok() {
                        return Ok(LanguageServerBinary {
                            path: binary_path.clone(),
                            env: None,
                            arguments: Default::default(),
                        });
                    }
                } else {
                    log::info!(
                        "SHA-256 mismatch for {binary_path:?} asset, downloading new asset. Expected: {expected_digest}, Got: {actual_digest}"
                    );
                }
            } else if validity_check().await.is_ok() {
                return Ok(LanguageServerBinary {
                    path: binary_path.clone(),
                    env: None,
                    arguments: Default::default(),
                });
            }
        }

        let destination_container_path = container_dir.join(format!("roslyn-{}-tmp", name));
        if fs::metadata(&binary_path).await.is_err() {
            let asset_kind = if url.ends_with(".zip") {
                AssetKind::Zip
            } else {
                AssetKind::TarGz
            };
            download_server_binary(
                &*delegate.http_client(),
                &url,
                expected_digest.as_deref(),
                &destination_container_path,
                asset_kind,
            )
            .await?;

            let found = find_binary_in_dir(&destination_container_path, &binary_name)
                .await
                .context("failed to find csharp-language-server binary in extracted asset")?;

            fs::create_dir_all(&version_dir).await?;
            fs::copy(&found, &binary_path).await?;

            remove_matching(&container_dir, |entry| entry != version_dir).await;
            GithubBinaryMetadata::write_to_file(
                &GithubBinaryMetadata {
                    metadata_version: 1,
                    digest: expected_digest,
                },
                &metadata_path,
            )
            .await?;

            // Best-effort prefetch of Roslyn; ignore failures.
            let bp = binary_path.clone();
            smol::spawn(async move {
                let _ = util::command::new_smol_command(&bp)
                    .arg("--download")
                    .output()
                    .await;
            })
            .detach();

            #[cfg(not(windows))]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))?;
            }
        }

        Ok(LanguageServerBinary {
            path: binary_path,
            env: None,
            arguments: Default::default(),
        })
    }

    async fn cached_server_binary(
        &self,
        container_dir: PathBuf,
        _: &dyn LspAdapterDelegate,
    ) -> Option<LanguageServerBinary> {
        get_cached_roslyn_binary(container_dir).await
    }
}

#[async_trait(?Send)]
impl LspAdapter for CsharpLspAdapter {
    fn name(&self) -> LanguageServerName {
        Self::SERVER_NAME
    }

    async fn workspace_configuration(
        self: Arc<Self>,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _toolchain: Option<Toolchain>,
        _scope_uri: Option<Uri>,
        cx: &mut AsyncApp,
    ) -> Result<serde_json::Value> {
        let project_options = cx.update(|cx| {
            language_server_settings(delegate.as_ref(), &Self::SERVER_NAME, cx)
                .and_then(|s| s.settings.clone())
        })?;
        Ok(project_options.unwrap_or_default())
    }
}

async fn find_binary_in_dir(dir: &Path, filename: &str) -> Result<PathBuf> {
    // Quick check for the simple case where the binary is a direct child.
    let candidate = dir.join(filename);
    if fs::metadata(&candidate).await.is_ok() {
        return Ok(candidate);
    }

    // Iterative DFS to avoid recursive `async fn` calls which are not allowed.
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        let mut entries = fs::read_dir(&path).await?;
        while let Some(entry) = entries.next().await {
            let entry = entry?;
            let p = entry.path();
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                stack.push(p);
            } else if file_type.is_file()
                && p.file_name().and_then(|s| s.to_str()) == Some(filename)
            {
                return Ok(p);
            }
        }
    }

    bail!("failed to find {filename} in extracted archive {dir:?}")
}

async fn get_cached_roslyn_binary(container_dir: PathBuf) -> Option<LanguageServerBinary> {
    maybe!(async {
        let mut last_roslyn_dir = None;
        let mut entries = fs::read_dir(&container_dir).await?;
        while let Some(entry) = entries.next().await {
            let entry = entry?;
            if entry.file_type().await?.is_dir() {
                last_roslyn_dir = Some(entry.path());
            }
        }
        let roslyn_dir = last_roslyn_dir.context("no cached binary")?;
        let roslyn_bin = roslyn_dir.join(if cfg!(target_os = "windows") {
            format!("csharp-language-server{}", std::env::consts::EXE_SUFFIX)
        } else {
            "csharp-language-server".to_string()
        });
        anyhow::ensure!(
            roslyn_bin.exists(),
            "missing csharp-language-server binary in directory {:?}",
            roslyn_dir
        );
        Ok(LanguageServerBinary {
            path: roslyn_bin,
            env: None,
            arguments: Vec::new(),
        })
    })
    .await
    .log_err()
}

pub(crate) struct CsharpContextProvider;

const CS_PROJECT_TASK_VARIABLE: VariableName = VariableName::Custom(Cow::Borrowed("CS_PROJECT"));
const CS_PROJECT_DIR_TASK_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("CS_PROJECT_DIR"));
const CS_PROJECT_NAME_TASK_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("CS_PROJECT_NAME"));
const CS_SOLUTION_TASK_VARIABLE: VariableName = VariableName::Custom(Cow::Borrowed("CS_SOLUTION"));

impl ContextProvider for CsharpContextProvider {
    fn build_context(
        &self,
        _variables: &TaskVariables,
        location: ContextLocation<'_>,
        _project_env: Option<HashMap<String, String>>,
        _: Arc<dyn LanguageToolchainStore>,
        cx: &mut App,
    ) -> Task<Result<TaskVariables>> {
        let local_abs_path = location
            .file_location
            .buffer
            .read(cx)
            .file()
            .and_then(|file| Some(file.as_local()?.abs_path(cx)));

        let project_vars = local_abs_path
            .as_deref()
            .and_then(|local_abs_path| local_abs_path.parent())
            .and_then(|buffer_dir| {
                let mut found_csproj: Option<PathBuf> = None;
                let mut found_sln: Option<PathBuf> = None;

                for ancestor in buffer_dir.ancestors() {
                    if let Ok(entries) = std::fs::read_dir(ancestor) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.is_file() {
                                if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
                                    if ext.eq_ignore_ascii_case("csproj") {
                                        found_csproj = Some(p.clone());
                                        break;
                                    } else if ext.eq_ignore_ascii_case("sln") && found_sln.is_none()
                                    {
                                        found_sln = Some(p.clone());
                                    }
                                }
                            }
                        }
                    }
                    if found_csproj.is_some() {
                        break;
                    }
                }

                let found = found_csproj.or(found_sln)?;

                let project = found.to_string_lossy().into_owned();
                let project_dir = found
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| ".".to_string());
                let project_name = found
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();

                let solution_tuple = if found
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|e| e.eq_ignore_ascii_case("sln"))
                    .unwrap_or(false)
                {
                    Some((
                        CS_SOLUTION_TASK_VARIABLE.clone(),
                        found
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    ))
                } else {
                    None
                };

                Some(TaskVariables::from_iter(
                    [
                        Some((CS_PROJECT_TASK_VARIABLE.clone(), project)),
                        Some((CS_PROJECT_DIR_TASK_VARIABLE.clone(), project_dir)),
                        Some((CS_PROJECT_NAME_TASK_VARIABLE.clone(), project_name)),
                        solution_tuple,
                    ]
                    .into_iter()
                    .flatten(),
                ))
            });

        Task::ready(Ok(project_vars.unwrap_or_default()))
    }

    fn associated_tasks(
        &self,
        file: Option<Arc<dyn File>>,
        cx: &App,
    ) -> Task<Option<TaskTemplates>> {
        let Some(file) = project::File::from_dyn(file.as_ref()).cloned() else {
            return Task::ready(None);
        };
        let Some(worktree_root) = file.worktree.read(cx).root_dir() else {
            return Task::ready(None);
        };
        let file_relative_path = file.path().clone();

        cx.background_spawn(async move {
            // Locate the nearest `.csproj` (preferred) or `.sln` ancestor, like `build_context`.
            let start = worktree_root.join(file_relative_path.as_unix_str());
            let buffer_dir = start
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| worktree_root.to_path_buf());

            let mut found_csproj: Option<PathBuf> = None;
            let mut found_sln: Option<PathBuf> = None;

            for ancestor in buffer_dir.ancestors() {
                if let Ok(entries) = std::fs::read_dir(ancestor) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_file() {
                            if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
                                if ext.eq_ignore_ascii_case("csproj") {
                                    found_csproj = Some(p.clone());
                                    break;
                                } else if ext.eq_ignore_ascii_case("sln") && found_sln.is_none() {
                                    found_sln = Some(p.clone());
                                }
                            }
                        }
                    }
                }
                if found_csproj.is_some() {
                    break;
                }
            }

            let project_path = match found_csproj.or(found_sln) {
                Some(p) => p,
                None => return None,
            };

            let mut task_templates: Vec<TaskTemplate> = Vec::new();

            // Always provide a build task.
            task_templates.push(TaskTemplate {
                label: "Build current project".into(),
                command: "dotnet".into(),
                args: vec!["build".into(), CS_PROJECT_TASK_VARIABLE.template_value()],
                cwd: Some(CS_PROJECT_DIR_TASK_VARIABLE.template_value()),
                tags: vec!["dotnet-build".to_owned()],
                ..TaskTemplate::default()
            });

            // For a .csproj, try to detect capabilities via MSBuild properties.
            let is_csproj = project_path
                .extension()
                .and_then(|s| s.to_str())
                .map(|e| e.eq_ignore_ascii_case("csproj"))
                .unwrap_or(false);

            let mut can_run = false;
            let mut is_test_project = false;

            if is_csproj {
                let props =
                    msbuild_get_properties(&project_path, &["OutputType", "IsTestProject"]).await;
                if let Some(output_type) = props.get("OutputType") {
                    let lower = output_type.to_lowercase();
                    if lower == "exe" || lower == "winexe" {
                        can_run = true;
                    }
                }

                if let Some(is_test) = props.get("IsTestProject") {
                    if is_test.to_lowercase() == "true" {
                        is_test_project = true;
                    }
                }
            }

            // Add `dotnet run` only for projects that produce an executable.
            if can_run {
                task_templates.push(TaskTemplate {
                    label: "Run current project".into(),
                    command: "dotnet".into(),
                    args: vec![
                        "run".into(),
                        "--project".into(),
                        CS_PROJECT_TASK_VARIABLE.template_value(),
                    ],
                    cwd: Some(CS_PROJECT_DIR_TASK_VARIABLE.template_value()),
                    tags: vec!["dotnet-run".to_owned()],
                    ..TaskTemplate::default()
                });
            }

            // Add test tasks only for test projects.
            if is_test_project {
                task_templates.push(TaskTemplate {
                    label: "Test current project".into(),
                    command: "dotnet".into(),
                    args: vec!["test".into(), CS_PROJECT_TASK_VARIABLE.template_value()],
                    cwd: Some(CS_PROJECT_DIR_TASK_VARIABLE.template_value()),
                    tags: vec!["dotnet-test".to_owned()],
                    ..TaskTemplate::default()
                });

                task_templates.push(TaskTemplate {
                    label: "Test (symbol)".to_owned(),
                    command: "dotnet".into(),
                    args: vec![
                        "test".into(),
                        CS_PROJECT_TASK_VARIABLE.template_value(),
                        "--filter".into(),
                        format!(
                            "FullyQualifiedName~{}",
                            VariableName::Symbol.template_value()
                        ),
                    ],
                    cwd: Some(CS_PROJECT_DIR_TASK_VARIABLE.template_value()),
                    tags: vec!["dotnet-test-symbol".to_owned()],
                    ..TaskTemplate::default()
                });
            }

            // Restore and publish are always available for identified .NET project context.
            task_templates.push(TaskTemplate {
                label: "Restore current project".into(),
                command: "dotnet".into(),
                args: vec!["restore".into(), CS_PROJECT_TASK_VARIABLE.template_value()],
                cwd: Some(CS_PROJECT_DIR_TASK_VARIABLE.template_value()),
                tags: vec!["dotnet-restore".to_owned()],
                use_new_terminal: false,
                allow_concurrent_runs: true,
                reveal: RevealStrategy::Always,
                reveal_target: RevealTarget::Center,
                hide: HideStrategy::OnSuccess,
                ..TaskTemplate::default()
            });

            task_templates.push(TaskTemplate {
                label: "Publish current project to Release".into(),
                command: "dotnet".into(),
                args: vec![
                    "publish".into(),
                    "--project".into(),
                    CS_PROJECT_TASK_VARIABLE.template_value(),
                    "-c".into(),
                    "Release".into(),
                ],
                cwd: Some(CS_PROJECT_DIR_TASK_VARIABLE.template_value()),
                tags: vec!["dotnet-publish".to_owned()],
                ..TaskTemplate::default()
            });

            Some(TaskTemplates(task_templates))
        })
    }
}

async fn msbuild_get_properties(project: &Path, properties: &[&str]) -> HashMap<String, String> {
    // Run `dotnet msbuild <project> /nologo /v:q /getProperty:...` for all
    // requested properties in a single invocation and parse the resulting
    // combined output (JSON or text) for those properties.
    let mut cmd = util::command::new_smol_command("dotnet");
    cmd.arg("msbuild").arg(project).arg("/nologo").arg("/v:q");
    for prop in properties {
        cmd.arg(format!("/getProperty:{}", prop));
    }

    let output = match cmd.output().await {
        Ok(output) => output,
        Err(e) => {
            log::debug!("failed to run msbuild to get properties: {e:#}");
            return HashMap::default();
        }
    };

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut map = HashMap::default();
    for prop in properties {
        if let Some(val) = parse_msbuild_property_output(&combined, prop) {
            map.insert(prop.to_string(), val);
        }
    }

    map
}

/// Parse MSBuild output and attempt to extract the value of `property`.
///
/// This parser supports multiple output formats:
/// 1. If the command returned JSON with a top-level `Properties` object (e.g.
///    when multiple properties were requested), that JSON is parsed and the
///    property is read from `Properties` (preferred).
/// 2. Otherwise the parser falls back to looking for a line that mentions the
///    property and extracts a value after `=` or `:` (or the token following the
///    property name).
///
/// Values are sanitized (trimmed, surrounding quotes removed, trailing commas/braces
/// trimmed) so formats like `"OutputType": "Exe",` are handled correctly.
///
/// This helper is pure and unit-testable.
fn parse_msbuild_property_output(output: &str, property: &str) -> Option<String> {
    // Prefer JSON output when available.
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(output) {
        if let Some(props) = json.get("Properties") {
            if let Some(val) = props.get(property) {
                if val.is_string() {
                    return Some(val.as_str().unwrap_or_default().to_string());
                } else {
                    return Some(val.to_string());
                }
            }
        }
    }

    // Helper to normalize crude values like `"Exe",`, `"",` or `Exe}` into `Exe`/``.
    fn sanitize_property_value(s: &str) -> String {
        let mut s = s.trim();
        // Remove trailing commas, braces, and brackets that can appear in inline JSON.
        s = s.trim_end_matches(|c: char| c == ',' || c == '}' || c == ']' || c.is_whitespace());
        // Trim again and strip surrounding quotes if present.
        s = s.trim();
        if s.starts_with('\"') && s.ends_with('\"') && s.len() >= 2 {
            s = &s[1..s.len() - 1];
        }
        s.trim().to_string()
    }

    let prop_lower = property.to_lowercase();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let lower = line.to_lowercase();
        if lower.contains(&prop_lower) {
            // Prefer explicit separators and sanitize extracted value.
            if let Some((_, val)) = line.split_once('=') {
                return Some(sanitize_property_value(val));
            }
            if let Some((_, val)) = line.split_once(':') {
                return Some(sanitize_property_value(val));
            }

            // Try the token after the property name: `OutputType Exe`.
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if tokens.len() >= 2 {
                let prop_idx = tokens
                    .iter()
                    .position(|t| t.to_lowercase().contains(&prop_lower));
                if let Some(idx) = prop_idx {
                    if idx + 1 < tokens.len() {
                        return Some(sanitize_property_value(tokens[idx + 1]));
                    }
                }
            }

            // As a last resort return the sanitized whole line.
            return Some(sanitize_property_value(line));
        }
    }

    // If the whole output is a single token (best-effort), return it (sanitized).
    let non_empty: Vec<&str> = output
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if non_empty.len() == 1 && non_empty[0].split_whitespace().count() == 1 {
        return Some(sanitize_property_value(non_empty[0]));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_equals() {
        let out = "OutputType = Exe\n";
        assert_eq!(
            parse_msbuild_property_output(out, "OutputType"),
            Some("Exe".to_string())
        );
    }

    #[test]
    fn parse_colon() {
        let out = "OutputType: Exe\n";
        assert_eq!(
            parse_msbuild_property_output(out, "OutputType"),
            Some("Exe".to_string())
        );
    }

    #[test]
    fn parse_value_only() {
        let out = "Exe\n";
        assert_eq!(
            parse_msbuild_property_output(out, "OutputType"),
            Some("Exe".to_string())
        );
    }

    #[test]
    fn parse_whitespace_value_only() {
        let out = "   Exe   \n";
        assert_eq!(
            parse_msbuild_property_output(out, "OutputType"),
            Some("Exe".to_string())
        );
    }

    #[test]
    fn parse_case_insensitive() {
        let out = "Property OutputType: Exe\n";
        assert_eq!(
            parse_msbuild_property_output(out, "outputtype"),
            Some("Exe".to_string())
        );
    }

    #[test]
    fn parse_absent_property_returns_none() {
        let out = "Some noise\n";
        assert_eq!(parse_msbuild_property_output(out, "OutputType"), None);
    }

    #[test]
    fn parse_json_properties() {
        let out = r#"{
  "Properties": {
    "IsTestProject": "",
    "OutputType": "Exe"
  }
}"#;
        assert_eq!(
            parse_msbuild_property_output(out, "OutputType"),
            Some("Exe".to_string())
        );
        assert_eq!(
            parse_msbuild_property_output(out, "IsTestProject"),
            Some("".to_string())
        );
    }

    #[test]
    fn parse_is_test_project_true() {
        let out = "IsTestProject = true\n";
        assert_eq!(
            parse_msbuild_property_output(out, "IsTestProject"),
            Some("true".to_string())
        );
    }
}

use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use collections::HashMap;
use futures::StreamExt;
use gpui::{App, AsyncApp, Task};
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
use task::{TaskTemplate, TaskTemplates, TaskVariables, VariableName};
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

    fn associated_tasks(&self, _: Option<Arc<dyn File>>, _: &App) -> Task<Option<TaskTemplates>> {
        Task::ready(Some(TaskTemplates(vec![
            TaskTemplate {
                label: format!("Build: {}", CS_PROJECT_TASK_VARIABLE.template_value()),
                command: "dotnet".into(),
                args: vec!["build".into(), CS_PROJECT_TASK_VARIABLE.template_value()],
                cwd: Some(CS_PROJECT_DIR_TASK_VARIABLE.template_value()),
                tags: vec!["dotnet-build".to_owned()],
                ..TaskTemplate::default()
            },
            TaskTemplate {
                label: format!("Run: {}", CS_PROJECT_TASK_VARIABLE.template_value()),
                command: "dotnet".into(),
                args: vec![
                    "run".into(),
                    "--project".into(),
                    CS_PROJECT_TASK_VARIABLE.template_value(),
                ],
                cwd: Some(CS_PROJECT_DIR_TASK_VARIABLE.template_value()),
                tags: vec!["dotnet-run".to_owned()],
                ..TaskTemplate::default()
            },
            TaskTemplate {
                label: format!("Test: {}", CS_PROJECT_TASK_VARIABLE.template_value()),
                command: "dotnet".into(),
                args: vec!["test".into(), CS_PROJECT_TASK_VARIABLE.template_value()],
                cwd: Some(CS_PROJECT_DIR_TASK_VARIABLE.template_value()),
                tags: vec!["dotnet-test".to_owned()],
                ..TaskTemplate::default()
            },
            TaskTemplate {
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
            },
            TaskTemplate {
                label: format!("Restore: {}", CS_PROJECT_TASK_VARIABLE.template_value()),
                command: "dotnet".into(),
                args: vec!["restore".into(), CS_PROJECT_TASK_VARIABLE.template_value()],
                cwd: Some(CS_PROJECT_DIR_TASK_VARIABLE.template_value()),
                tags: vec!["dotnet-restore".to_owned()],
                ..TaskTemplate::default()
            },
            TaskTemplate {
                label: format!(
                    "Publish (Release): {}",
                    CS_PROJECT_TASK_VARIABLE.template_value()
                ),
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
            },
        ])))
    }
}

use smol::fs;
use smol::process::Command;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

use anyhow::Result;
use gpui::AsyncApp;
use language::{LspAdapter, LspAdapterDelegate, LspInstaller};
use lsp::{LanguageServerBinary, LanguageServerName};

use std::process::Stdio;

pub struct CSharpLspAdapter;

impl CSharpLspAdapter {
    const SERVER_NAME: LanguageServerName = LanguageServerName::new_static("roslyn");

    fn get_runtime_rid() -> &'static str {
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        return "win-x64";
        #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
        return "win-arm64";
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        return "osx-x64";
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        return "osx-arm64";
        #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
        return "linux-x64";
        #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
        return "linux-arm64";
        #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "musl"))]
        return "linux-musl-x64";
        #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "musl"))]
        return "linux-musl-arm64";
    }
}

impl LspInstaller for CSharpLspAdapter {
    type BinaryVersion = String;

    async fn fetch_latest_server_version(
        &self,
        delegate: &dyn LspAdapterDelegate,
        _pre_release: bool,
        cx: &mut AsyncApp,
    ) -> Result<Self::BinaryVersion> {
        static DID_SHOW_NOTIFICATION: AtomicBool = AtomicBool::new(false);

        const NOTIFICATION_MESSAGE: &str = "Could not install the Roslyn Language Server because the 'dotnet' CLI was not found in your PATH. Please install the .NET SDK from https://dotnet.microsoft.com/en-us/download and ensure that 'dotnet' is available in your system PATH.";

        if delegate.which("dotnet".as_ref()).await.is_none() {
            if DID_SHOW_NOTIFICATION
                .compare_exchange(false, true, SeqCst, SeqCst)
                .is_ok()
            {
                cx.update(|cx| {
                    delegate.show_notification(NOTIFICATION_MESSAGE, cx);
                })?
            }
            anyhow::bail!("cannot install Roslyn Language Server: 'dotnet' CLI not found");
        }

        // Try to fetch the latest version from the Azure Artifacts feed using dotnet.
        let dotnet_path = "dotnet";

        let mut cmd = Command::new(dotnet_path);
        cmd.arg("package")
            .arg("search")
            .arg("Microsoft.CodeAnalysis.LanguageServer.neutral")
            .arg("--source")
            .arg("https://pkgs.dev.azure.com/azure-public/vside/_packaging/vs-impl/nuget/v3/index.json")
            .arg("--prerelease")
            .arg("--format")
            .arg("json")
            .stdout(Stdio::piped());

        let child = cmd.spawn()?;
        let output = child.output().await?;

        if !output.status.success() {
            anyhow::bail!("Unable to fetch latest version of Roslyn Language Server");
        }

        // Parse the JSON output to extract the latestVersion field.
        let json_str = String::from_utf8_lossy(&output.stdout);
        let version = (|| {
            use serde_json::Value;
            let v: Value = serde_json::from_str(&json_str).ok()?;
            let search_result = v.get("searchResult")?;
            let first_result = search_result.get(0)?;
            let packages = first_result.get("packages")?;
            let first_package = packages.get(0)?;
            let latest_version = first_package.get("latestVersion")?;
            latest_version.as_str().map(|s| s.to_string())
        })();

        let version = match version {
            Some(ver) if !ver.trim().is_empty() => ver.trim().to_string(),
            _ => {
                anyhow::bail!(
                    "Could not determine the latest version of the Roslyn Language Server"
                );
            }
        };

        Ok(version)
    }

    async fn fetch_server_binary(
        &self,
        latest_version: Self::BinaryVersion,
        container_dir: PathBuf,
        _delegate: &dyn LspAdapterDelegate,
    ) -> Result<LanguageServerBinary> {
        let rid = Self::get_runtime_rid();

        // Create a temporary directory for the dotnet restore operation.
        let temp_project_dir = container_dir.join("roslyn_server_temp_project");
        fs::create_dir_all(&temp_project_dir).await?;

        // Write a minimal .csproj file that references the Roslyn Language Server NuGet package.
        let csproj_path = temp_project_dir.join("RoslynServer.csproj");
        let csproj_contents: &str = r#"<Project Sdk="Microsoft.Build.NoTargets/1.0.80">
                    <PropertyGroup>
                        <RestoreSources>https://pkgs.dev.azure.com/azure-public/vside/_packaging/vs-impl/nuget/v3/index.json</RestoreSources>
                        <RestorePackagesPath>out</RestorePackagesPath>
                        <TargetFramework>netstandard2.0</TargetFramework>
                        <DisableImplicitNuGetFallbackFolder>true</DisableImplicitNuGetFallbackFolder>
                        <DisableImplicitFrameworkReferences>true</DisableImplicitFrameworkReferences>
                    </PropertyGroup>

                    <ItemGroup>
                        <PackageDownload Include="$(LanguageServerPackage)" Version="[$(LanguageServerVersion)]" />
                    </ItemGroup>
                </Project>
"#;
        fs::write(&csproj_path, csproj_contents).await?;

        // Run `dotnet restore` to download the package.
        let mut restore_cmd = Command::new("dotnet");
        restore_cmd
            .arg("restore")
            .arg(format!(
                "-p:LanguageServerPackage=Microsoft.CodeAnalysis.LanguageServer.{rid}"
            ))
            .arg(format!("-p:LanguageServerVersion={latest_version}"))
            .current_dir(&temp_project_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let status = restore_cmd.status().await?;
        if !status.success() {
            anyhow::bail!("Failed to restore Roslyn Language Server NuGet package");
        }

        let temp_build_dir = temp_project_dir
            .join("out")
            .join(format!("microsoft.codeanalysis.languageserver.{rid}"))
            .join(latest_version)
            .join("content")
            .join("LanguageServer")
            .join(rid);

        move_dir(&temp_build_dir, &container_dir)?;
        remove(temp_project_dir)?;

        Ok(LanguageServerBinary {
            path: container_dir.join("Microsoft.CodeAnalysis.LanguageServer"),
            arguments: vec![
                "--logLevel=Information".into(),
                "--extensionLogDirectory".into(),
                OsString::from(container_dir.join("log").to_string_lossy().into_owned()),
                "--stdio".into(),
            ],
            env: None,
        })
    }

    async fn cached_server_binary(
        &self,
        container_dir: PathBuf,
        _delegate: &dyn LspAdapterDelegate,
    ) -> Option<LanguageServerBinary> {
        // Check if the server binary already exists in the container_dir.
        #[cfg(target_os = "windows")]
        let binary_name = "Microsoft.CodeAnalysis.LanguageServer.exe";
        #[cfg(not(target_os = "windows"))]
        let binary_name = "Microsoft.CodeAnalysis.LanguageServer";

        let binary_path = container_dir.join(binary_name);
        if fs::metadata(&binary_path).await.is_ok() {
            let args = vec![
                "--logLevel=Information".into(),
                "--extensionLogDirectory".into(),
                OsString::from(container_dir.join("log").to_string_lossy().into_owned()),
                "--stdio".into(),
            ];
            Some(LanguageServerBinary {
                path: binary_path,
                arguments: args,
                env: None,
            })
        } else {
            None
        }
    }
}

impl LspAdapter for CSharpLspAdapter {
    fn name(&self) -> LanguageServerName {
        Self::SERVER_NAME
    }
}

// TODO: The following methods utilities are using std::fs and are blocking. They should be replaced with async equivalents.
fn remove<P: AsRef<Path>>(path: P) -> Result<()> {
    if path.as_ref().exists() {
        Ok(std::fs::remove_dir_all(path)?)
    } else {
        Ok(())
    }
}

fn move_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_tree(src, dst)?;
            std::fs::remove_dir_all(src)
        }
        Err(e) => Err(e),
    }
}

fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());

        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

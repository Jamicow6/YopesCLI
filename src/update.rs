use std::{
    env, fs,
    io::{self, Write},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/Jamicow6/YopesCLI/releases/latest";
const CHECKSUM_ASSET: &str = "SHA256SUMS";
const INSTALL_FRAMES: [&str; 3] = [".", "..", "..."];

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

pub fn current_version() -> &'static str {
    option_env!("YOPES_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

pub async fn run() -> Result<()> {
    println!("Checking for updates...");
    let release = fetch_release().await?;
    let latest = parse_version(&release.tag_name)?;
    let current = parse_version(current_version())?;

    if latest <= current {
        println!("Already up to date (v{current}).");
        return Ok(());
    }

    println!("Found tag v{latest} newer than your build v{current}.");
    print!("Installing ");
    io::stdout().flush().context("writing update progress")?;

    let dots = tokio::spawn(async {
        let mut index = 0;
        loop {
            print!(
                "\rInstalling {:<3}",
                INSTALL_FRAMES[index % INSTALL_FRAMES.len()]
            );
            let _ = io::stdout().flush();
            index = index.wrapping_add(1);
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    });

    let result = install_release(&release).await;
    dots.abort();
    let _ = dots.await;

    match result {
        Ok(()) => {
            print!("\rInstalling ... done\n");
            let stopped = stop_other_yopes();
            if stopped > 0 {
                println!(
                    "Stopped {stopped} older yopes instance{}.",
                    if stopped == 1 { "" } else { "s" }
                );
            }
            println!("Finished. Starting yopes...");
            restart()
        }
        Err(error) => {
            print!("\rInstalling ... failed\n");
            Err(error)
        }
    }
}

async fn fetch_release() -> Result<Release> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .user_agent(format!("yopes/{}", current_version()))
        .build()
        .context("creating update client")?;
    let mut request = client
        .get(LATEST_RELEASE_URL)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Ok(token) = env::var("GITHUB_TOKEN").or_else(|_| env::var("GH_TOKEN")) {
        if !token.trim().is_empty() {
            request = request.bearer_auth(token);
        }
    }
    let response = request
        .send()
        .await
        .context("contacting GitHub releases")?
        .error_for_status()
        .context("fetching the latest release")?;
    response
        .json()
        .await
        .context("reading the latest release response")
}

async fn install_release(release: &Release) -> Result<()> {
    let asset_name = platform_asset_name()?;
    let asset = find_asset(release, &asset_name)?;
    let checksums = find_asset(release, CHECKSUM_ASSET)?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .user_agent(format!("yopes/{}", current_version()))
        .build()
        .context("creating download client")?;

    let archive = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .context("downloading the new build")?
        .error_for_status()
        .context("downloading the new build")?
        .bytes()
        .await
        .context("reading the new build")?;
    let checksum_text = client
        .get(&checksums.browser_download_url)
        .send()
        .await
        .context("downloading checksums")?
        .error_for_status()
        .context("downloading checksums")?
        .text()
        .await
        .context("reading checksums")?;

    let expected = checksum_for(&checksum_text, &asset_name)
        .with_context(|| format!("finding the checksum for {asset_name}"))?;
    let actual = sha256(&archive);
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("checksum mismatch for {asset_name}");
    }

    let temporary = env::temp_dir().join(format!("yopes-update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temporary);
    fs::create_dir_all(&temporary).with_context(|| format!("creating {}", temporary.display()))?;

    let archive_path = temporary.join(&asset_name);
    fs::write(&archive_path, &archive)
        .with_context(|| format!("writing {}", archive_path.display()))?;
    let extracted = temporary.join("extracted");
    fs::create_dir_all(&extracted).with_context(|| format!("creating {}", extracted.display()))?;
    extract(&archive_path, &extracted)?;

    let binary_name = if cfg!(windows) { "yopes.exe" } else { "yopes" };
    let new_binary = extracted.join(binary_name);
    if !new_binary.is_file() {
        bail!("the downloaded archive does not contain {binary_name}");
    }
    install_binary(&new_binary)?;
    let _ = fs::remove_dir_all(&temporary);
    Ok(())
}

fn find_asset<'a>(release: &'a Release, name: &str) -> Result<&'a Asset> {
    release
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .with_context(|| format!("release {} does not include {name}", release.tag_name))
}

fn platform_asset_name() -> Result<String> {
    let os = match env::consts::OS {
        "linux" => "Linux",
        "windows" => "Windows",
        other => bail!("automatic updates are not supported on {other}"),
    };
    let arch = match env::consts::ARCH {
        "x86_64" => "X64",
        other => bail!("automatic updates are not supported on {other}"),
    };
    let extension = if cfg!(windows) { "zip" } else { "tar.gz" };
    Ok(format!("yopes-{os}-{arch}.{extension}"))
}

fn parse_version(value: &str) -> Result<Version> {
    let value = value.trim().trim_start_matches('v');
    Version::parse(value).with_context(|| format!("invalid version `{value}`"))
}

fn checksum_for<'a>(checksums: &'a str, name: &str) -> Option<&'a str> {
    checksums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let checksum = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        (file == name).then_some(checksum)
    })
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn extract(archive: &Path, destination: &Path) -> Result<()> {
    #[cfg(windows)]
    let status = {
        let script = "Expand-Archive -LiteralPath $env:YOPES_ARCHIVE -DestinationPath $env:YOPES_DEST -Force";
        Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("YOPES_ARCHIVE", archive)
            .env("YOPES_DEST", destination)
            .status()
            .context("running PowerShell to extract the update")?
    };
    #[cfg(not(windows))]
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(destination)
        .status()
        .context("running tar to extract the update")?;
    if !status.success() {
        bail!("could not extract {}", archive.display());
    }
    Ok(())
}

fn install_binary(new_binary: &Path) -> Result<()> {
    let current = env::current_exe().context("locating the current yopes executable")?;
    let current = fs::canonicalize(&current).unwrap_or(current);
    let directory = current
        .parent()
        .context("locating the yopes install directory")?;
    let file_name = current
        .file_name()
        .context("locating the yopes executable name")?;
    let staged = directory.join(format!(".{}.new", file_name.to_string_lossy()));
    fs::copy(new_binary, &staged)
        .with_context(|| format!("staging the update at {}", staged.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
            .context("marking the new build executable")?;
    }

    #[cfg(windows)]
    {
        let backup = directory.join(format!(".{}.old", file_name.to_string_lossy()));
        let _ = fs::remove_file(&backup);
        fs::rename(&current, &backup).with_context(|| {
            format!(
                "replacing {}; close any other yopes instances and try again",
                current.display()
            )
        })?;
        if let Err(error) = fs::rename(&staged, &current) {
            let _ = fs::rename(&backup, &current);
            let _ = fs::remove_file(&staged);
            return Err(error).with_context(|| format!("installing {}", current.display()));
        }
        let _ = fs::remove_file(&backup);
    }

    #[cfg(not(windows))]
    {
        fs::rename(&staged, &current)
            .with_context(|| format!("installing {}", current.display()))?;
    }
    Ok(())
}

fn restart() -> Result<()> {
    let current = env::current_exe().context("locating the updated executable")?;
    Command::new(current)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("starting the updated yopes")?;
    Ok(())
}

fn stop_other_yopes() -> usize {
    #[cfg(windows)]
    {
        let script = "(Get-Process -Name yopes -ErrorAction SilentlyContinue | Where-Object { $_.Id -ne [int]$env:YOPES_SELF } | Stop-Process -Force).Count";
        let output = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("YOPES_SELF", std::process::id().to_string())
            .output();
        match output {
            Ok(out) => out.stdout().to_string().trim().parse().unwrap_or(0),
            Err(_) => 0,
        }
    }
    #[cfg(not(windows))]
    {
        let pids = other_yopes_pids();
        if pids.is_empty() {
            return 0;
        }
        let args: Vec<String> = pids.iter().map(|pid| pid.to_string()).collect();
        match Command::new("kill").args(args).status() {
            Ok(_) => pids.len(),
            Err(_) => 0,
        }
    }
}

#[cfg(not(windows))]
fn other_yopes_pids() -> Vec<u32> {
    let self_pid = std::process::id();
    let mut pids = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return pids;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let Ok(comm) = fs::read(format!("/proc/{pid}/comm")) else {
            continue;
        };
        if comm.trim() == "yopes" {
            pids.push(pid);
        }
    }
    pids
}

#[cfg(test)]
mod tests {
    use super::{checksum_for, parse_version, platform_asset_name, sha256};

    #[test]
    fn parses_release_tags_with_an_optional_v_prefix() {
        assert_eq!(parse_version("v0.1.2").unwrap().to_string(), "0.1.2");
        assert_eq!(parse_version("0.2.0").unwrap().to_string(), "0.2.0");
        assert!(parse_version("nightly").is_err());
    }

    #[test]
    fn selects_the_checksum_for_the_platform_asset() {
        let checksums = "aaa  yopes-Linux-X64.tar.gz\nbbb  yopes-Windows-X64.zip\n";
        assert_eq!(
            checksum_for(checksums, "yopes-Windows-X64.zip"),
            Some("bbb")
        );
        assert_eq!(checksum_for(checksums, "missing.zip"), None);
    }

    #[test]
    fn platform_asset_uses_the_release_naming_scheme() {
        let name = platform_asset_name().unwrap();
        assert!(name.starts_with("yopes-"));
        assert!(name.ends_with(if cfg!(windows) { ".zip" } else { ".tar.gz" }));
    }

    #[test]
    fn hashes_match_sha256() {
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}

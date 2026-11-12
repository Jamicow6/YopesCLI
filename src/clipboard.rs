//! Clipboard reads, platform-gated: image first, then text. Windows goes
//! through PowerShell, so binary image data never crosses stdout.

// `context` is only called from the PowerShell path below.
#[cfg(windows)]
use anyhow::Context;
use anyhow::Result;

/// Read the clipboard, preferring an image; through PowerShell on Windows.
#[cfg(windows)]
pub(crate) fn read_clipboard() -> Result<(String, String), String> {
    let image = read_clipboard_image().map_err(|error| format!("{error:#}"))?;
    if let Some(found) = image {
        return Ok(found);
    }
    match read_clipboard_text() {
        Some(text) => Ok(("text".to_owned(), text)),
        None => Err("the clipboard holds neither text nor an image".to_owned()),
    }
}

#[cfg(not(windows))]
pub(crate) fn read_clipboard() -> Result<(String, String), String> {
    for argv in [
        vec!["pbpaste"],
        vec!["xclip", "-selection", "clipboard", "-o"],
        vec!["wl-paste", "--no-newline"],
    ] {
        if let Ok(output) = std::process::Command::new(argv[0])
            .args(&argv[1..])
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                let text = text.trim();
                if !text.is_empty() {
                    return Ok(("text".to_owned(), text.to_owned()));
                }
            }
        }
    }
    Err("the clipboard holds neither text nor an image on this platform".to_owned())
}

/// Read clipboard text with `Get-Clipboard -Raw`; `None` when it has none.
#[cfg(windows)]
fn read_clipboard_text() -> Option<String> {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-STA",
            "-Command",
            "[Console]::Out.Write((Get-Clipboard -Raw))",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if text.is_empty() { None } else { Some(text) }
}

/// Read the clipboard as a PNG via PowerShell, returning `None` when the
/// clipboard holds no image.
///
/// PowerShell encodes the image itself so binary data never crosses stdout,
/// which Windows PowerShell would otherwise re-encode.
#[cfg(windows)]
fn read_clipboard_image() -> Result<Option<(String, String)>> {
    use std::process::Command as ProcessCommand;

    const SCRIPT: &str = "Add-Type -AssemblyName System.Windows.Forms; \
        Add-Type -AssemblyName System.Drawing; \
        $img = [System.Windows.Forms.Clipboard]::GetImage(); \
        if ($img -eq $null) { exit 2 } \
        $ms = New-Object System.IO.MemoryStream; \
        $img.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png); \
        [Convert]::ToBase64String($ms.ToArray())";
    let output = ProcessCommand::new("powershell")
        .args(["-NoProfile", "-STA", "-Command", SCRIPT])
        .output()
        .context("running powershell to read the clipboard")?;
    if output.status.code() == Some(2) {
        return Ok(None);
    }
    let encoded = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || encoded.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("could not read the clipboard image: {}", stderr.trim());
    }
    Ok(Some(("paste".to_owned(), encoded)))
}

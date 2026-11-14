//! Clipboard reads and writes, platform-gated: image first, then text.
//! On Windows, binary image data is converted to PNG base64 in PowerShell so
//! it never crosses stdout as raw binary.

#[cfg(windows)]
use anyhow::Result;

/// Read the clipboard, preferring an image; through PowerShell on Windows.
#[cfg(windows)]
pub(crate) fn read_clipboard() -> Result<(String, String), String> {
    use std::process::Command as ProcessCommand;

    const SCRIPT: &str = r#"
        Add-Type -AssemblyName System.Windows.Forms;
        Add-Type -AssemblyName System.Drawing;
        $img = [System.Windows.Forms.Clipboard]::GetImage();
        if ($img -ne $null) {
            $ms = New-Object System.IO.MemoryStream;
            $img.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png);
            $img.Dispose();
            [Console]::Out.Write('IMG:' + [Convert]::ToBase64String($ms.ToArray()));
            exit 0;
        }
        $stream = [System.Windows.Forms.Clipboard]::GetData('PNG');
        if ($stream -ne $null -and ($stream -is [System.IO.MemoryStream])) {
            [Console]::Out.Write('IMG:' + [Convert]::ToBase64String($stream.ToArray()));
            exit 0;
        }
        if ([System.Windows.Forms.Clipboard]::ContainsFileDropList()) {
            foreach ($f in [System.Windows.Forms.Clipboard]::GetFileDropList()) {
                if ($f -match '\.(png|jpe?g|webp|gif|bmp)$' -and (Test-Path $f)) {
                    $i = [System.Drawing.Image]::FromFile($f);
                    $m = New-Object System.IO.MemoryStream;
                    $i.Save($m, [System.Drawing.Imaging.ImageFormat]::Png);
                    $i.Dispose();
                    [Console]::Out.Write('IMG:' + [Convert]::ToBase64String($m.ToArray()));
                    exit 0;
                }
            }
        }
        $txt = [System.Windows.Forms.Clipboard]::GetText();
        if ($txt) {
            $c = $txt.Trim('"', '''', ' ', "`t", "`r", "`n");
            if ($c -match '\.(png|jpe?g|webp|gif|bmp)$' -and (Test-Path $c)) {
                $i = [System.Drawing.Image]::FromFile($c);
                $m = New-Object System.IO.MemoryStream;
                $i.Save($m, [System.Drawing.Imaging.ImageFormat]::Png);
                $i.Dispose();
                [Console]::Out.Write('IMG:' + [Convert]::ToBase64String($m.ToArray()));
                exit 0;
            }
            [Console]::Out.Write('TXT:' + $txt);
            exit 0;
        }
        exit 2;
    "#;

    let output = ProcessCommand::new("powershell")
        .args(["-NoProfile", "-STA", "-Command", SCRIPT])
        .output()
        .map_err(|error| format!("running powershell to read clipboard: {error}"))?;

    if output.status.code() == Some(2) {
        return Err("the clipboard holds neither text nor an image".to_owned());
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("could not read clipboard: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Some(encoded) = stdout.strip_prefix("IMG:") {
        let trimmed = encoded.trim();
        if !trimmed.is_empty() {
            return Ok(("paste".to_owned(), trimmed.to_owned()));
        }
    } else if let Some(text) = stdout.strip_prefix("TXT:") {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Ok(("text".to_owned(), trimmed.to_owned()));
        }
    }

    Err("the clipboard holds neither text nor an image".to_owned())
}

#[cfg(not(windows))]
pub(crate) fn read_clipboard() -> Result<(String, String), String> {
    for argv in [
        vec!["wl-paste", "--type", "image/png"],
        vec!["xclip", "-selection", "clipboard", "-t", "image/png", "-o"],
    ] {
        if let Ok(output) = std::process::Command::new(argv[0])
            .args(&argv[1..])
            .output()
        {
            if output.status.success() && !output.stdout.is_empty() {
                use base64::Engine;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&output.stdout);
                return Ok(("paste".to_owned(), b64));
            }
        }
    }
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

/// Write text to system clipboard.
#[cfg(windows)]
pub(crate) fn write_clipboard(text: &str) -> Result<(), String> {
    use base64::Engine;
    use std::process::Command;

    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         [System.Windows.Forms.Clipboard]::SetText(\
             [System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{encoded}'))\
         )"
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-STA", "-Command", &script])
        .output()
        .map_err(|err| format!("failed to run powershell: {err}"))?;
    if !output.status.success() {
        return Err("failed to copy to clipboard".to_owned());
    }
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn write_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    for argv in [
        vec!["pbcopy"],
        vec!["wl-copy"],
        vec!["xclip", "-selection", "clipboard"],
    ] {
        if let Ok(mut child) = Command::new(argv[0])
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if child.wait().map(|status| status.success()).unwrap_or(false) {
                return Ok(());
            }
        }
    }
    Err("clipboard write failed".to_owned())
}

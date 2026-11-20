use std::path::Path;
use std::process::Command;

pub fn command(milliseconds: u64, output: Option<(&Path, &str)>) -> Command {
    #[cfg(windows)]
    {
        let mut process = Command::new("powershell.exe");
        process
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$ErrorActionPreference = 'Stop'; Start-Sleep -Milliseconds ([int]$env:SIC_TEST_DELAY_MS); if ($env:SIC_TEST_TOOL_FILE) { [IO.File]::WriteAllText($env:SIC_TEST_TOOL_FILE, $env:SIC_TEST_TOOL_TEXT) }",
            ])
            .env("SIC_TEST_DELAY_MS", milliseconds.to_string())
            .env_remove("SIC_TEST_TOOL_FILE");
        if let Some((path, text)) = output {
            process.env("SIC_TEST_TOOL_FILE", path).env("SIC_TEST_TOOL_TEXT", text);
        }
        process
    }
    #[cfg(not(windows))]
    {
        let seconds = format!("{}.{:03}", milliseconds / 1000, milliseconds % 1000);
        let mut process = Command::new("sh");
        process
            .args(["-c", "sleep \"$1\" && { [ \"$#\" -eq 1 ] || printf '%s' \"$3\" > \"$2\"; }", "sic-test-tool"])
            .arg(seconds);
        if let Some((path, text)) = output {
            process.arg(path).arg(text);
        }
        process
    }
}

use clap::{Parser, Subcommand};
use serde::Serialize;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Parser)]
#[command(name = "lewittctl", version, about = "Stream 4x5 control CLI")]
pub(crate) struct Cli {
    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    List,
    Status,
    WatchMeters,
    Get {
        control: String,
    },
    Set {
        control: String,
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    Preset {
        #[command(subcommand)]
        command: PresetCommand,
    },
    Diagnose,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PresetCommand {
    Import { source: PathBuf, output: PathBuf },
    Export { source: PathBuf, output: PathBuf },
    Apply { preset: PathBuf },
}

#[derive(Debug, Serialize)]
pub(crate) struct DiagnosticReport {
    command: &'static str,
    platform: &'static str,
    vendor_api_found: bool,
    vendor_api_path: Option<String>,
    writes_enabled: bool,
    protocol_evidence_available: bool,
    detail: String,
}

#[derive(Debug, Error)]
pub(crate) enum CliError {
    #[error("{command} is not available until its backend operations are verified")]
    Unavailable { command: &'static str },
}

#[derive(Debug, Serialize)]
struct ErrorReport {
    ok: bool,
    error: String,
}

pub(crate) fn execute(command: &Command) -> Result<DiagnosticReport, CliError> {
    match command {
        Command::Diagnose => Ok(diagnose()),
        Command::List => Err(unavailable("list")),
        Command::Status => Err(unavailable("status")),
        Command::WatchMeters => Err(unavailable("watch-meters")),
        Command::Get { .. } => Err(unavailable("get")),
        Command::Set { .. } => Err(unavailable("set")),
        Command::Preset { command } => match command {
            PresetCommand::Import { .. } => Err(unavailable("preset import")),
            PresetCommand::Export { .. } => Err(unavailable("preset export")),
            PresetCommand::Apply { .. } => Err(unavailable("preset apply")),
        },
    }
}

pub(crate) fn render_report(
    report: &DiagnosticReport,
    json: bool,
) -> Result<String, serde_json::Error> {
    if json {
        serde_json::to_string_pretty(report)
    } else {
        Ok(format!(
            "platform: {}\nvendor API found: {}\nwrites enabled: {}\n{}",
            report.platform, report.vendor_api_found, report.writes_enabled, report.detail
        ))
    }
}

pub(crate) fn render_error(error: &CliError, json: bool) -> String {
    if !json {
        return error.to_string();
    }
    let report = ErrorReport {
        ok: false,
        error: error.to_string(),
    };
    serde_json::to_string_pretty(&report)
        .unwrap_or_else(|_| "{\"ok\":false,\"error\":\"serialization failed\"}".into())
}

const fn unavailable(command: &'static str) -> CliError {
    CliError::Unavailable { command }
}

#[cfg(windows)]
fn diagnose() -> DiagnosticReport {
    match lewitt_backend_windows::locate_registered_vendor_api() {
        Ok(path) => DiagnosticReport {
            command: "diagnose",
            platform: "windows",
            vendor_api_found: true,
            vendor_api_path: Some(path.display().to_string()),
            writes_enabled: false,
            protocol_evidence_available: true,
            detail: "registered vendor API path is valid; the application enables only the verified read-only ABI subset"
                .into(),
        },
        Err(error) => DiagnosticReport {
            command: "diagnose",
            platform: "windows",
            vendor_api_found: false,
            vendor_api_path: None,
            writes_enabled: false,
            protocol_evidence_available: false,
            detail: error.to_string(),
        },
    }
}

#[cfg(target_os = "linux")]
fn diagnose() -> DiagnosticReport {
    DiagnosticReport {
        command: "diagnose",
        platform: "linux",
        vendor_api_found: false,
        vendor_api_path: None,
        writes_enabled: false,
        protocol_evidence_available: false,
        detail: "system-libusb discovery is not implemented yet".into(),
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn diagnose() -> DiagnosticReport {
    DiagnosticReport {
        command: "diagnose",
        platform: "unsupported",
        vendor_api_found: false,
        vendor_api_path: None,
        writes_enabled: false,
        protocol_evidence_available: false,
        detail: "this platform is outside the v1 target set".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_command_surface_parses() {
        for arguments in [
            vec!["lewittctl", "list"],
            vec!["lewittctl", "status"],
            vec!["lewittctl", "watch-meters"],
            vec!["lewittctl", "get", "monitor-volume"],
            vec!["lewittctl", "set", "monitor-volume", "-12"],
            vec!["lewittctl", "preset", "apply", "voice.json"],
            vec!["lewittctl", "diagnose", "--json"],
        ] {
            Cli::try_parse_from(arguments).expect("required command should parse");
        }
    }

    #[test]
    fn unavailable_commands_return_structured_json_errors() {
        let error = execute(&Command::Status).expect_err("status is not verified yet");
        let rendered = render_error(&error, true);
        let decoded: serde_json::Value =
            serde_json::from_str(&rendered).expect("error output should be valid JSON");
        assert_eq!(decoded["ok"], false);
    }

    #[test]
    fn diagnose_never_claims_writes_are_enabled() {
        let report = diagnose();
        assert!(!report.writes_enabled);
        if report.vendor_api_found {
            assert!(report.protocol_evidence_available);
        }
    }
}

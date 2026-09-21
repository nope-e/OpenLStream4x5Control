use lewitt_app::single_instance::{InstanceLaunch, claim_or_notify};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let log_path = lewitt_app::logging::init();
    if let Some(path) = &log_path {
        tracing::info!(path = %path.display(), "local logging initialized");
    } else {
        tracing::warn!("file logging unavailable; using stderr when possible");
    }

    let result = run();
    if let Err(error) = &result {
        tracing::error!(error = %error, "application terminated with an error");
    }
    result
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let background = std::env::args_os().any(|argument| argument == "--background");
    tracing::info!(background, "starting Stream 4x5 control panel");
    match claim_or_notify()? {
        InstanceLaunch::Primary(instance) => lewitt_app::daemon::run(background, instance)?,
        InstanceLaunch::ExistingNotified => {
            tracing::info!("notified the existing application instance");
        }
    }
    Ok(())
}

use queen_client::RunnerClient;
use queen_core::models::EngineProfile;
use queen_core::CoreEvent;
use queen_protocol::{RunnerCommand, RunnerIdentity};
use std::{env, path::PathBuf};

#[tokio::main]
async fn main() {
    if let Err(error) = probe().await {
        eprintln!("runner probe failed: {error}");
        std::process::exit(1);
    }
}

async fn probe() -> Result<(), String> {
    let identity_path = env::var_os("QUEENUI_RUNNER_IDENTITY_FILE")
        .map(PathBuf::from)
        .ok_or_else(|| "Set QUEENUI_RUNNER_IDENTITY_FILE before running the probe".to_string())?;
    let identity: RunnerIdentity = serde_json::from_slice(
        &std::fs::read(identity_path)
            .map_err(|error| format!("Could not read the runner identity: {error}"))?,
    )
    .map_err(|error| format!("Could not decode the runner identity: {error}"))?;
    let client = RunnerClient::from_identity(identity)?;
    let capabilities = client.capabilities().await?;
    println!(
        "{} {} {} logical-cpus={}",
        capabilities.hostname,
        capabilities.operating_system,
        capabilities.architecture,
        capabilities.logical_cpus
    );
    let mut events = client.events().await?;
    let envelope = events
        .next()
        .await?
        .ok_or_else(|| "Runner closed the event stream before its initial snapshot".to_string())?;
    match envelope.event {
        CoreEvent::Snapshot(snapshot) => {
            println!(
                "snapshot engines={} accounts={} games={}",
                snapshot.engines.len(),
                snapshot.accounts.len(),
                snapshot.games.len()
            );
        }
        _ => return Err("Runner did not begin the event stream with a snapshot".into()),
    }
    if let (Ok(root_id), Ok(relative_path)) = (
        env::var("QUEENUI_PROBE_ENGINE_ROOT"),
        env::var("QUEENUI_PROBE_ENGINE_PATH"),
    ) {
        let profile: EngineProfile = client
            .command(RunnerCommand::RegisterEngine {
                root_id,
                relative_path,
            })
            .await?;
        println!(
            "engine name={} options={}",
            profile.name, profile.option_count
        );
        client
            .command::<()>(RunnerCommand::RemoveEngine {
                engine_id: profile.id,
            })
            .await?;
    }
    Ok(())
}

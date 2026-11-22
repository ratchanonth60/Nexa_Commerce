use std::time::Duration;

use tauri::{App, AppHandle, State};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Mutex;

#[derive(Default)]
pub(crate) struct UpdateInstall(Mutex<()>);

pub(crate) fn configure(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    let Some(config) = app.config().plugins.0.get("updater") else {
        return Ok(());
    };
    if config
        .get("requireSignedVersion")
        .and_then(|value| value.as_bool())
        != Some(true)
        || !config.as_object().is_some_and(|settings| {
            settings.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "requireSignedVersion" | "endpoints" | "pubkey"
                )
            })
        })
    {
        return Err(
            "Update configuration must require signed versions and use secure defaults.".into(),
        );
    }

    app.handle()
        .plugin(tauri_plugin_updater::Builder::new().build())?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn check_for_update(app: AppHandle) -> Result<Option<String>, String> {
    Ok(available_update(&app).await?.map(|update| update.version))
}

#[tauri::command]
pub(crate) async fn install_update(
    app: AppHandle,
    install: State<'_, UpdateInstall>,
    version: String,
) -> Result<(), String> {
    let _install = install
        .0
        .try_lock()
        .map_err(|_| "An update is already in progress.")?;
    let mut update = available_update(&app)
        .await?
        .ok_or("No update is available.")?;
    if update.version != version {
        return Err("The available version changed. Check for updates again.".into());
    }
    update.timeout = Some(Duration::from_secs(180));
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|_| "The update could not be downloaded, verified, or installed.")?;
    app.request_restart();
    Ok(())
}

async fn available_update(app: &AppHandle) -> Result<Option<Update>, String> {
    if !super::updater_enabled(app.clone()) {
        return Err("Updates are disabled for this build.".into());
    }
    app.updater_builder()
        .configure_client(|client| client.https_only(true))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| "Could not configure desktop updates.")?
        .check()
        .await
        .map_err(|_| "Could not check for desktop updates.".into())
}

//! Channel/playlist subscription commands.

use crate::commands::CmdResult;
use crate::logging::push_app_log;
use crate::AppState;
use tauri::{AppHandle, State};
use uuid::Uuid;
use vidgrab_shared::{DownloadItem, Subscription};

#[tauri::command]
pub async fn cmd_list_subscriptions(state: State<'_, AppState>) -> CmdResult<Vec<Subscription>> {
    state
        .subscriptions
        .list_subscriptions()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_save_subscription(
    state: State<'_, AppState>,
    subscription: Subscription,
) -> CmdResult<()> {
    state
        .subscriptions
        .save_subscription(&subscription)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_remove_subscription(state: State<'_, AppState>, id: Uuid) -> CmdResult<()> {
    state
        .subscriptions
        .remove_subscription(id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_check_subscription_now(
    state: State<'_, AppState>,
    id: Uuid,
) -> CmdResult<Vec<DownloadItem>> {
    state
        .subscriptions
        .check_subscription_now(id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_check_all_subscriptions(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<usize> {
    let subs = state
        .subscriptions
        .list_subscriptions()
        .map_err(|e| e.to_string())?;
    let mut total_added = 0usize;
    for sub in subs {
        if sub.paused {
            continue;
        }
        if let Ok(items) = state.subscriptions.check_subscription_now(sub.id).await {
            total_added += items.len();
        }
    }
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Vérification abonnements terminée : {total_added} nouvelle(s) vidéo(s)"),
    );
    Ok(total_added)
}

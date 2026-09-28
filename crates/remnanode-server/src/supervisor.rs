//! Сторож xray: если процесс упал сам (OOM и т.п.), перезапускаем его с последним
//! конфигом панели и доигрываем изменения юзеров. Иначе нода висит без xray,
//! а панель лишь пишет «xrayInfo is null» и не перезапускает её.
use std::time::Duration;
use axum::body::Bytes;
use crate::handlers::handler;
use crate::state::AppState;

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if let Some(status) = check_exited(&state).await {
                tracing::error!(
                    "Xray exited unexpectedly ({status}); memory: {}",
                    remnanode_xray::process::memory_breakdown()
                );
                restart(&state).await;
                // Не перезапускаем чаще раза в 10 секунд
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    });
}

async fn check_exited(state: &AppState) -> Option<String> {
    let mut xray = state.xray.write().await;
    let status = match xray.process.as_mut()?.try_wait() {
        Ok(Some(status)) => status.to_string(),
        _ => return None,
    };
    xray.process = None;
    xray.handler_client = None;
    xray.stats_client = None;
    xray.router_client = None;
    Some(status)
}

async fn restart(state: &AppState) {
    {
        let mut xray = state.xray.write().await;
        if xray.config.is_none() || xray.process.is_some() {
            return;
        }
        match crate::xray_process::start_xray(&state.internal.socket_path, &state.internal.token).await {
            Ok(child) => {
                tracing::info!("Xray restarted by supervisor (PID {:?})", child.id());
                xray.process = Some(child);
            }
            Err(e) => {
                tracing::error!("Supervisor failed to restart xray: {e}");
                return;
            }
        }
    }

    // Ждём gRPC; блокировку отпускаем между попытками — xray берёт конфиг через internal API
    let mut connected = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let mut xray = state.xray.write().await;
        if xray.process.is_none() {
            return;
        }
        if xray.connect_grpc(state.env.xtls_api_port, &state.mtls_certs).await.is_ok() {
            connected = true;
            break;
        }
    }
    if !connected {
        tracing::error!("Supervisor: gRPC to restarted xray failed");
        return;
    }

    let (journal, overflow) = {
        let xray = state.xray.read().await;
        (xray.journal.clone(), xray.journal_overflow)
    };
    if overflow {
        tracing::warn!("Supervisor: user journal overflowed, some users may be missing until panel restart");
    }
    for (path, body) in &journal {
        let body = Bytes::from(body.clone());
        let _ = match path.as_str() {
            "add_user" => handler::add_user_inner(state.clone(), body).await,
            "remove_user" => handler::remove_user_inner(state.clone(), body).await,
            "add_users" => handler::add_users_inner(state.clone(), body).await,
            "remove_users" => handler::remove_users_inner(state.clone(), body).await,
            _ => continue,
        };
    }
    tracing::info!("Supervisor: xray recovered, replayed {} user changes", journal.len());
}

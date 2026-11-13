//! Headless HTTP REST API & WebUI server (`vidgrab-cli serve`) for remote/NAS/Docker control.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use uuid::Uuid;
use vidgrab_core::config::{load_or_create_settings, AppPaths};
use vidgrab_core::db::Database;
use vidgrab_core::queue::DownloadQueue;
use vidgrab_shared::{DownloadProgress, DownloadRequest, Settings};

#[derive(Clone)]
pub struct ServerState {
    pub queue: DownloadQueue,
    pub settings: Arc<RwLock<Settings>>,
    pub token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EnqueuePayload {
    pub url: String,
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub active_items: usize,
}

fn check_auth(headers: &HeaderMap, expected: Option<&str>) -> Result<(), StatusCode> {
    let Some(token) = expected.filter(|t| !t.is_empty()) else {
        return Ok(());
    };
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if auth == format!("Bearer {token}") {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

async fn handle_index() -> Html<&'static str> {
    Html(
        r#"<!DOCTYPE html>
<html lang="fr">
<head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>vidgrab — Contrôle à distance</title>
<style>
body{font-family:system-ui,sans-serif;background:#111317;color:#e5e7eb;max-width:760px;margin:2rem auto;padding:0 1rem}
.row{display:flex;gap:.5rem;margin-bottom:1rem}
input{flex:1;padding:.6rem;border-radius:8px;border:1px solid #374151;background:#1f2937;color:#fff}
button{padding:.6rem 1rem;border-radius:8px;border:0;background:#10b981;color:#fff;font-weight:600;cursor:pointer}
.card{background:#1b1f26;border:1px solid #2d333f;border-radius:10px;padding:.8rem;margin-bottom:.6rem;display:flex;justify-content:space-between;align-items:center}
</style>
</head>
<body>
<h2>4k free video dowloader — Serveur Web</h2>
<div class="row"><input id="tok" type="password" placeholder="Token Bearer (optionnel)"><input id="url" placeholder="Coller une URL vidéo ou playlist..."><button onclick="enqueue()">Télécharger</button></div>
<div id="list"></div>
<script>
const hdrs=()=>document.getElementById('tok').value?{'Authorization':'Bearer '+document.getElementById('tok').value,'Content-Type':'application/json'}:{'Content-Type':'application/json'};
async function load(){const r=await fetch('/api/downloads',{headers:hdrs()});if(!r.ok)return;const items=await r.json();document.getElementById('list').innerHTML=items.map(i=>`<div class="card"><div><b>${i.request.title}</b><br><small>${i.progress.status} — ${Math.round(i.progress.percent||0)}%</small></div></div>`).join('');}
async function enqueue(){const url=document.getElementById('url').value.trim();if(!url)return;await fetch('/api/enqueue',{method:'POST',headers:hdrs(),body:JSON.stringify({url})});document.getElementById('url').value='';load();}
setInterval(load,2500);load();
</script>
</body></html>"#,
    )
}

async fn handle_health(State(st): State<ServerState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        active_items: st.queue.list_items().len(),
    })
}

async fn handle_list_downloads(
    State(st): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    check_auth(&headers, st.token.as_deref())?;
    Ok(Json(st.queue.list_items()))
}

async fn handle_enqueue(
    State(st): State<ServerState>,
    headers: HeaderMap,
    Json(payload): Json<EnqueuePayload>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    check_auth(&headers, st.token.as_deref())
        .map_err(|s| (s, "Unauthorized".to_string()))?;
    let url = payload.url.trim().to_string();
    if url.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "URL vide".to_string()));
    }
    let out_dir = st
        .settings
        .read()
        .map(|s| s.video_output_dir.clone())
        .unwrap_or_default();
    let req = DownloadRequest {
        id: Uuid::new_v4(),
        url: url.clone(),
        title: payload.title.unwrap_or(url),
        output_dir: out_dir,
        ..Default::default()
    };
    let item = st
        .queue
        .enqueue(req)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok((StatusCode::CREATED, Json(item)))
}

async fn handle_delete_download(
    State(st): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    check_auth(&headers, st.token.as_deref())?;
    st.queue
        .remove(id, false)
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|_| StatusCode::NOT_FOUND)
}

pub fn build_router(state: ServerState) -> Router {
    Router::new()
        .route("/", get(handle_index))
        .route("/api/health", get(handle_health))
        .route("/api/downloads", get(handle_list_downloads))
        .route("/api/enqueue", post(handle_enqueue))
        .route("/api/downloads/{id}", delete(handle_delete_download))
        .with_state(state)
}

pub async fn run_serve(args: &[String]) -> i32 {
    let mut bind_addr = "127.0.0.1:8787".to_string();
    let mut token: Option<String> = std::env::var("VIDGRAB_SERVER_TOKEN").ok();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--bind" if i + 1 < args.len() => {
                bind_addr = args[i + 1].clone();
                i += 2;
            }
            "--token" if i + 1 < args.len() => {
                token = Some(args[i + 1].clone());
                i += 2;
            }
            _ => i += 1,
        }
    }

    let Ok(paths) = AppPaths::initialize() else {
        eprintln!("Impossible d'initialiser les chemins vidgrab.");
        return 1;
    };
    let settings = load_or_create_settings(&paths).unwrap_or_default();
    let Ok(db) = Database::open(&paths.db_path) else {
        eprintln!("Impossible d'ouvrir la base SQLite.");
        return 1;
    };
    let db = Arc::new(db);
    let shared_settings = Arc::new(RwLock::new(settings));
    let (tx, mut rx) = mpsc::unbounded_channel::<DownloadProgress>();
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let Ok(queue) = DownloadQueue::new(paths, db, shared_settings.clone(), tx) else {
        eprintln!("Impossible d'initialiser la file de téléchargement.");
        return 1;
    };

    let router = build_router(ServerState {
        queue,
        settings: shared_settings,
        token,
    });

    let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Erreur bind {bind_addr}: {e}");
            return 1;
        }
    };
    println!("vidgrab-cli serve actif sur http://{bind_addr}");
    if let Err(e) = axum::serve(listener, router).await {
        eprintln!("Erreur serveur HTTP: {e}");
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn test_serve_health_and_auth_guard() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = AppPaths::from_roots(tmp.path(), tmp.path()).expect("paths");
        let db = Arc::new(Database::open(&paths.db_path).expect("db"));
        let settings = Arc::new(RwLock::new(Settings::default()));
        let (tx, _rx) = mpsc::unbounded_channel();
        let queue = DownloadQueue::new(paths, db, settings.clone(), tx).expect("queue");

        let app = build_router(ServerState {
            queue,
            settings,
            token: Some("secret123".to_string()),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut s1 = tokio::net::TcpStream::connect(addr).await.expect("conn");
        s1.write_all(b"GET /api/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("write");
        let mut buf1 = String::new();
        s1.read_to_string(&mut buf1).await.expect("read");
        assert!(buf1.contains("200 OK") && buf1.contains("\"status\":\"ok\""));

        let mut s2 = tokio::net::TcpStream::connect(addr).await.expect("conn2");
        s2.write_all(b"GET /api/downloads HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("write2");
        let mut buf2 = String::new();
        s2.read_to_string(&mut buf2).await.expect("read2");
        assert!(buf2.contains("401 Unauthorized"));
    }
}

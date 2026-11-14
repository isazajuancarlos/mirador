// SPDX-License-Identifier: AGPL-3.0-or-later
//! El núcleo de mirador SIN pantalla: la base, el cliente HTTP y la caché de
//! videos. Lo comparten la interfaz GTK (por defecto) y —tras `--features
//! webview`— el servidor axum. La lógica de suscripciones/feed/extracción vive
//! en `db`, `rss` y `extractor`; aquí solo se ABRE todo y se sostiene vivo.

use std::path::PathBuf;
use std::sync::Arc;

use crate::db;

/// El estado compartido, sin nada de UI. `Clone` es barato: `SqlitePool` y
/// `reqwest::Client` son `Arc` por dentro, y `descarga` lo es explícitamente.
#[derive(Clone)]
pub struct Nucleo {
    pub pool: sqlx::SqlitePool,
    pub http: reqwest::Client,
    /// Directorio donde yt-dlp deja los mp4 (offline + re-ver instantáneo).
    pub cache_dir: PathBuf,
    /// Serializa las descargas: una a la vez (se ve un video a la vez), con
    /// doble-chequeo de caché para no bajar dos veces lo mismo.
    pub descarga: Arc<tokio::sync::Mutex<()>>,
}

/// Cuántos videos trae el feed.
pub const LIMITE_FEED: i64 = 120;

/// Abre la base, el cliente HTTP y la caché, y arranca el proveedor de PoToken
/// (best-effort). Devuelve el núcleo y —si arrancó— el hijo del PoToken, que el
/// llamante debe MANTENER VIVO (muere con mirador al soltarlo).
pub async fn preparar() -> Result<(Nucleo, Option<tokio::process::Child>), Box<dyn std::error::Error>>
{
    let url_bd = std::env::var("MIRADOR_DB").unwrap_or_else(|_| "sqlite://mirador.db".to_string());

    // Caché de videos: $XDG_CACHE_HOME/mirador (o ~/.cache/mirador).
    let cache_dir = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mirador");
    tokio::fs::create_dir_all(&cache_dir).await?;

    // Proveedor de PoToken (para DASH HD); best-effort.
    let pot = crate::extractor::arrancar_potoken();

    let pool = db::abrir(&url_bd).await?;
    let http = reqwest::Client::builder()
        .user_agent("mirador/0.1 (+local)")
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let nucleo = Nucleo {
        pool,
        http,
        cache_dir,
        descarga: Arc::new(tokio::sync::Mutex::new(())),
    };
    Ok((nucleo, pot))
}

/// Baja+remuxea un video a la caché si falta (bajo lock, con doble-chequeo), y
/// devuelve la ruta del mp4 local. Es la misma secuencia que servía el `/stream`
/// del WebView, pero devuelve el ARCHIVO en vez de proxearlo por HTTP: GtkVideo
/// (GStreamer) reproduce un fichero local directamente. Marca `no_disponible` si
/// yt-dlp no puede con él (premiere/privado/geo/borrado), como hacía el servidor.
pub async fn asegurar_local(nucleo: &Nucleo, video_id: &str) -> crate::error::Resultado<PathBuf> {
    let archivo = nucleo.cache_dir.join(format!("{video_id}.mp4"));
    if archivo.exists() {
        return Ok(archivo);
    }
    let _g = nucleo.descarga.lock().await;
    if archivo.exists() {
        return Ok(archivo);
    }
    match crate::extractor::descargar(video_id, &nucleo.cache_dir).await {
        Ok(ruta) => Ok(ruta),
        Err(e) => {
            db::marcar_no_disponible(&nucleo.pool, video_id).await.ok();
            Err(e)
        }
    }
}

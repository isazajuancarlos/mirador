// SPDX-License-Identifier: AGPL-3.0-or-later
//! El FRONTAL VIEJO por WebView (axum + maud + tao/wry), tras `--features
//! webview`. El estándar de escritorio es GTK puro (`escritorio.rs`); esto se
//! conserva como referencia mientras se porta, y se borrará al alcanzar paridad.
//! Sirve el HTML del `vista` en `127.0.0.1:{puerto}` y lo carga en una ventana
//! WebKitGTK. La lógica de dominio (db/rss/extractor) es la MISMA que usa la UI
//! nativa; lo único propio de aquí es el router y el proxy de `/stream`.

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use sqlx::SqlitePool;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use crate::error::{Error, Resultado};
use crate::{db, extractor, importar, rss, vista};

const LIMITE_FEED: i64 = crate::nucleo::LIMITE_FEED;

#[derive(Clone)]
struct Estado {
    pool: SqlitePool,
    http: reqwest::Client,
    cache_dir: std::path::PathBuf,
    descarga: std::sync::Arc<tokio::sync::Mutex<()>>,
}

/// Arranca el servidor axum en el runtime y abre la ventana WebView (bloquea el
/// hilo principal hasta cerrarla). Reúsa el runtime creado por `main`.
pub fn correr(rt: &tokio::runtime::Runtime) {
    let (puerto, _pot) = match rt.block_on(preparar_servidor()) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("mirador no arrancó: {e}");
            std::process::exit(1);
        }
    };

    if std::env::var("MIRADOR_HEADLESS").is_ok() {
        tracing::info!("modo headless: sirviendo sin ventana");
        rt.block_on(std::future::pending::<()>());
        return;
    }

    if let Err(e) = crate::gui::abrir_ventana(puerto) {
        tracing::error!("no se pudo abrir la ventana: {e}");
        std::process::exit(1);
    }
}

/// Abre la BD, monta el router y ARRANCA axum en una tarea de fondo del runtime.
/// Devuelve el puerto real en el que quedó escuchando.
async fn preparar_servidor(
) -> Result<(u16, Option<tokio::process::Child>), Box<dyn std::error::Error>> {
    let (nucleo, pot) = crate::nucleo::preparar().await?;
    let puerto: u16 = std::env::var("MIRADOR_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8600);

    let estado = Estado {
        pool: nucleo.pool,
        http: nucleo.http,
        cache_dir: nucleo.cache_dir,
        descarga: nucleo.descarga,
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/suscripciones", get(suscripciones_get).post(suscripciones_post))
        .route("/suscripciones/importar", post(suscripciones_importar))
        .route("/suscripciones/{channel_id}/borrar", post(suscripciones_borrar))
        .route("/actualizar", post(actualizar))
        .route("/buscar", get(buscar))
        .route("/ver/{video_id}", get(ver))
        .route("/stream/{video_id}", get(stream))
        .route("/htmx.min.js", get(vista::htmx_js))
        .route("/boi.js", get(vista::boi_js))
        .route("/estilo.css", get(vista::estilo_css))
        .with_state(estado)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http());

    let escucha = tokio::net::TcpListener::bind(("127.0.0.1", puerto)).await?;
    let real = escucha.local_addr()?.port();
    tracing::info!("mirador sirviendo en http://127.0.0.1:{real}");
    tokio::spawn(async move {
        if let Err(e) = axum::serve(escucha, app).await {
            tracing::error!("servidor: {e}");
        }
    });
    Ok((real, pot))
}

async fn index(State(e): State<Estado>) -> Resultado<Response> {
    let videos = db::feed(&e.pool, LIMITE_FEED).await?;
    Ok(vista::feed_pagina(&videos).into_response())
}

async fn suscripciones_get(State(e): State<Estado>) -> Resultado<Response> {
    render_subs(&e, None, false).await
}

#[derive(serde::Deserialize)]
struct FormCanal {
    canal: String,
}

async fn suscripciones_post(
    State(e): State<Estado>,
    axum::Form(f): axum::Form<FormCanal>,
) -> Response {
    match agregar_canal(&e, &f.canal).await {
        Ok(titulo) => render_subs(&e, Some(&format!("Añadido: {titulo}")), false)
            .await
            .unwrap_or_else(|err| err.into_response()),
        Err(err) => render_subs(&e, Some(&err.to_string()), true)
            .await
            .unwrap_or_else(|err| err.into_response()),
    }
}

/// Importa el `subscriptions.csv` de Google Takeout (subida multipart). Siembra
/// las suscripciones sin bajar sus videos —eso lo hace Actualizar, como FreeTube—.
async fn suscripciones_importar(
    State(e): State<Estado>,
    mut mp: axum::extract::Multipart,
) -> Response {
    let mut texto = String::new();
    while let Ok(Some(field)) = mp.next_field().await {
        if let Ok(bytes) = field.bytes().await {
            texto = String::from_utf8_lossy(&bytes).into_owned();
            break;
        }
    }
    let resultado = if texto.trim().is_empty() {
        render_subs(&e, Some("El archivo llegó vacío."), true).await
    } else {
        let items = importar::parsear_csv_youtube(&texto);
        if items.is_empty() {
            render_subs(&e, Some("No se reconoció ningún canal (¿es el subscriptions.csv de Takeout?)."), true).await
        } else {
            match db::importar_suscripciones(&e.pool, &items).await {
                Ok((nuevos, total)) => {
                    let msg = format!(
                        "Importadas {total} suscripciones ({nuevos} nuevas). Pulsa Actualizar para traer sus videos."
                    );
                    render_subs(&e, Some(&msg), false).await
                }
                Err(err) => render_subs(&e, Some(&err.to_string()), true).await,
            }
        }
    };
    resultado.unwrap_or_else(|err| err.into_response())
}

async fn suscripciones_borrar(
    State(e): State<Estado>,
    Path(channel_id): Path<String>,
) -> Resultado<Response> {
    db::quitar_suscripcion(&e.pool, &channel_id).await?;
    render_subs(&e, Some("Canal quitado."), false).await
}

async fn actualizar(State(e): State<Estado>) -> Resultado<Response> {
    let subs = db::listar_suscripciones(&e.pool).await?;
    let (mut ok, mut fallos) = (0u32, 0u32);
    for s in &subs {
        // Falla SUAVE por canal (directiva 20): uno caído no tumba el refresco.
        match rss::bajar_feed(&e.http, &s.channel_id).await {
            Ok(feed) => match db::upsert_videos(&e.pool, &feed.videos).await {
                Ok(()) => ok += 1,
                Err(err) => {
                    tracing::warn!("actualizar {}: {err}", s.channel_id);
                    fallos += 1;
                }
            },
            Err(err) => {
                tracing::warn!("actualizar {}: {err}", s.channel_id);
                fallos += 1;
            }
        }
    }
    tracing::info!("actualizar: {ok} canales, {fallos} con fallo");
    let videos = db::feed(&e.pool, LIMITE_FEED).await?;
    Ok(vista::feed_pagina(&videos).into_response())
}

#[derive(serde::Deserialize)]
struct FormBuscar {
    q: Option<String>,
}

/// Búsqueda de videos en YouTube (yt-dlp ytsearch, keyless).
async fn buscar(Query(f): Query<FormBuscar>) -> Resultado<Response> {
    let q = f.q.unwrap_or_default();
    let resultados = if q.trim().is_empty() {
        Vec::new()
    } else {
        extractor::buscar(&q, 24).await?
    };
    Ok(vista::buscar_pagina(&q, &resultados).into_response())
}

#[derive(serde::Deserialize)]
struct VerParams {
    t: Option<String>,
    c: Option<String>,
}

async fn ver(
    State(e): State<Estado>,
    Path(video_id): Path<String>,
    Query(p): Query<VerParams>,
) -> Resultado<Response> {
    if !extractor::es_video_id(&video_id) {
        return Err(Error::Entrada(format!("id de video inválido: {video_id}")));
    }
    let v = match db::video(&e.pool, &video_id).await? {
        Some(v) => v,
        None => db::VideoFeed {
            video_id: video_id.clone(),
            title: p.t.filter(|s| !s.is_empty()).unwrap_or_else(|| "Video".into()),
            published: chrono::Utc::now(),
            thumbnail: Some(format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg")),
            channel_title: p.c.unwrap_or_default(),
        },
    };
    Ok(vista::ver_pagina(&v).into_response())
}

/// Sirve el stream PROXEÁNDOLO por el servidor. Un 302 directo a googlevideo NO
/// funciona: GStreamer (el motor de WebKitGTK) no abre esas URLs — quedan en
/// negro. Así que mirador resuelve con yt-dlp, baja a la caché y reenvía los
/// bytes por HTTP local, propagando `Range` (seek) y el estado 206.
async fn stream(
    State(e): State<Estado>,
    Path(video_id): Path<String>,
    req: axum::extract::Request,
) -> Response {
    use tower::ServiceExt;
    if !extractor::es_video_id(&video_id) {
        return Error::Entrada(format!("id de video inválido: {video_id}")).into_response();
    }
    let archivo = e.cache_dir.join(format!("{video_id}.mp4"));

    if !archivo.exists() {
        let _g = e.descarga.lock().await;
        if !archivo.exists()
            && let Err(err) = extractor::descargar(&video_id, &e.cache_dir).await
        {
            db::marcar_no_disponible(&e.pool, &video_id).await.ok();
            return err.into_response();
        }
    }

    match tower_http::services::ServeFile::new(&archivo).oneshot(req).await {
        Ok(resp) => resp.map(axum::body::Body::new).into_response(),
        Err(_) => Error::Resolver(format!("no se pudo servir {video_id}")).into_response(),
    }
}

/// Resuelve el canal, baja su feed y lo siembra. Devuelve el título del canal.
async fn agregar_canal(e: &Estado, entrada: &str) -> Resultado<String> {
    let channel_id = extractor::resolver_channel_id(entrada).await?;
    let feed = rss::bajar_feed(&e.http, &channel_id).await?;
    db::agregar_suscripcion(&e.pool, &channel_id, &feed.channel_title).await?;
    db::upsert_videos(&e.pool, &feed.videos).await?;
    Ok(feed.channel_title)
}

async fn render_subs(e: &Estado, mensaje: Option<&str>, error: bool) -> Resultado<Response> {
    let subs = db::listar_suscripciones(&e.pool).await?;
    Ok(vista::suscripciones_pagina(&subs, mensaje, error).into_response())
}

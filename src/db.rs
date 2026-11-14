// SPDX-License-Identifier: AGPL-3.0-or-later
//! Persistencia: suscripciones y la caché de videos del feed. Una base por
//! instancia (WAL + claves foráneas para la cascada al quitar un canal).

use crate::error::Resultado;
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;

/// Un canal suscrito.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Suscripcion {
    pub channel_id: String,
    pub title: String,
}

/// Un video del feed, ya con el título de su canal (para pintar el feed sin un
/// segundo viaje a la BD).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct VideoFeed {
    pub video_id: String,
    pub title: String,
    pub published: DateTime<Utc>,
    // La miniatura la usa el frontal WebView (vista maud); la UI GTK aún no
    // pinta miniaturas (v1: filas de texto). Se lee de la BD igual, para no
    // perder el dato cuando la UI nativa las muestre.
    #[cfg_attr(not(feature = "webview"), allow(dead_code))]
    pub thumbnail: Option<String>,
    pub channel_title: String,
}

/// Un video tal como llega del RSS, para volcar a la caché.
#[derive(Debug, Clone)]
pub struct VideoNuevo {
    pub video_id: String,
    pub channel_id: String,
    pub title: String,
    pub published: DateTime<Utc>,
    pub thumbnail: Option<String>,
}

/// Abre el pool (creando la base si falta) y corre las migraciones.
pub async fn abrir(url: &str) -> Resultado<SqlitePool> {
    let opciones = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opciones)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

pub async fn listar_suscripciones(pool: &SqlitePool) -> Resultado<Vec<Suscripcion>> {
    let filas = sqlx::query_as::<_, Suscripcion>(
        "SELECT channel_id, title FROM subscriptions ORDER BY title COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?;
    Ok(filas)
}

/// Alta idempotente de un canal (refresca el título si cambió).
pub async fn agregar_suscripcion(pool: &SqlitePool, channel_id: &str, title: &str) -> Resultado<()> {
    sqlx::query(
        "INSERT INTO subscriptions (channel_id, title) VALUES (?1, ?2)
         ON CONFLICT(channel_id) DO UPDATE SET title = ?2",
    )
    .bind(channel_id)
    .bind(title)
    .execute(pool)
    .await?;
    Ok(())
}

/// Importa una lista de (channel_id, title) del CSV de Takeout. Idempotente:
/// refresca el título si ya existía. Devuelve (nuevos, total) — los videos NO se
/// bajan aquí (serían cientos de feeds): eso lo hace «Actualizar» después, como
/// FreeTube.
pub async fn importar_suscripciones(pool: &SqlitePool, items: &[(String, String)]) -> Resultado<(usize, usize)> {
    let mut nuevos = 0usize;
    for (id, title) in items {
        let r = sqlx::query(
            "INSERT INTO subscriptions (channel_id, title) VALUES (?1, ?2)
             ON CONFLICT(channel_id) DO NOTHING",
        )
        .bind(id)
        .bind(title)
        .execute(pool)
        .await?;
        nuevos += r.rows_affected() as usize;
        // Refrescar el título aunque ya existiera (idempotente).
        sqlx::query("UPDATE subscriptions SET title = ?2 WHERE channel_id = ?1")
            .bind(id)
            .bind(title)
            .execute(pool)
            .await?;
    }
    Ok((nuevos, items.len()))
}

/// Quita un canal; sus videos caen en cascada (FK ON DELETE CASCADE).
pub async fn quitar_suscripcion(pool: &SqlitePool, channel_id: &str) -> Resultado<()> {
    sqlx::query("DELETE FROM subscriptions WHERE channel_id = ?1")
        .bind(channel_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Vuelca los videos de un feed a la caché. Idempotente: un video ya visto
/// refresca su título (los canales editan títulos) sin duplicar.
pub async fn upsert_videos(pool: &SqlitePool, videos: &[VideoNuevo]) -> Resultado<()> {
    for v in videos {
        sqlx::query(
            "INSERT INTO videos (video_id, channel_id, title, published, thumbnail)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(video_id) DO UPDATE SET title = ?3, thumbnail = ?5",
        )
        .bind(&v.video_id)
        .bind(&v.channel_id)
        .bind(&v.title)
        .bind(v.published)
        .bind(&v.thumbnail)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// El feed: los videos más recientes de TODAS las suscripciones. OCULTA los no
/// reproducibles: los marcados `no_disponible` (falló su reproducción) y los de
/// fecha FUTURA (premieres que aún no se emiten).
pub async fn feed(pool: &SqlitePool, limite: i64) -> Resultado<Vec<VideoFeed>> {
    let filas = sqlx::query_as::<_, VideoFeed>(
        "SELECT v.video_id, v.title, v.published, v.thumbnail,
                s.title AS channel_title
         FROM videos v
         JOIN subscriptions s ON s.channel_id = v.channel_id
         WHERE v.no_disponible = 0 AND v.published <= datetime('now')
         ORDER BY v.published DESC
         LIMIT ?1",
    )
    .bind(limite)
    .fetch_all(pool)
    .await?;
    Ok(filas)
}

/// Marca un video como no reproducible para ocultarlo del feed. Idempotente.
pub async fn marcar_no_disponible(pool: &SqlitePool, video_id: &str) -> Resultado<()> {
    sqlx::query("UPDATE videos SET no_disponible = 1 WHERE video_id = ?1")
        .bind(video_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Un video concreto (para la página de reproducción). None si no está en caché.
/// Lo usa el frontal WebView (`ver`); la UI GTK reproduce desde el `Item` de la
/// lista, sin releer la BD.
#[cfg_attr(not(feature = "webview"), allow(dead_code))]
pub async fn video(pool: &SqlitePool, video_id: &str) -> Resultado<Option<VideoFeed>> {
    let fila = sqlx::query_as::<_, VideoFeed>(
        "SELECT v.video_id, v.title, v.published, v.thumbnail,
                s.title AS channel_title
         FROM videos v
         JOIN subscriptions s ON s.channel_id = v.channel_id
         WHERE v.video_id = ?1",
    )
    .bind(video_id)
    .fetch_optional(pool)
    .await?;
    Ok(fila)
}

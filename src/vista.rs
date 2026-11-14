// SPDX-License-Identifier: AGPL-3.0-or-later
//! La pantalla: layout maud + estáticos vendorizados (htmx y boi, la MISMA
//! versión byte-a-byte que chasqui, informes, medico, inventario y chuspa —
//! directiva 26). `include_str!` compila el activo dentro del binario: si falta
//! uno, el build FALLA en vez de servir un 404 en producción.

use crate::db::{Suscripcion, VideoFeed};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use maud::{html, Markup, DOCTYPE};

const HTMX_JS: &str = include_str!("../estaticos/htmx.min.js");
const BOI_JS: &str = include_str!("../estaticos/boi.js");
const ESTILO_CSS: &str = include_str!("../estaticos/estilo.css");

fn recurso(cuerpo: &'static str, tipo: &'static str) -> Response {
    ([(header::CONTENT_TYPE, tipo)], cuerpo).into_response()
}
pub async fn htmx_js() -> Response {
    recurso(HTMX_JS, "application/javascript; charset=utf-8")
}
pub async fn boi_js() -> Response {
    recurso(BOI_JS, "application/javascript; charset=utf-8")
}
pub async fn estilo_css() -> Response {
    recurso(ESTILO_CSS, "text/css; charset=utf-8")
}

const SECCIONES: &[(&str, &str)] =
    &[("/", "Feed"), ("/buscar", "Buscar"), ("/suscripciones", "Suscripciones")];

/// Percent-encode para meter texto en un query param (título/canal de un
/// resultado de búsqueda en el enlace a /ver). Conserva los no reservados.
pub fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Envoltura común: `<html>` con htmx + boi + css, barra de navegación y el
/// cuerpo de la página. `activa` marca la sección actual.
pub fn envoltura(titulo: &str, activa: &str, cuerpo: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="es" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (titulo) " · mirador" }
                link rel="stylesheet" href="/estilo.css";
                script src="/htmx.min.js" {}
                script type="module" src="/boi.js" {}
            }
            body {
                nav.barra {
                    a.marca href="/" { "mirador" }
                    span.espacio {}
                    @for (ruta, nombre) in SECCIONES {
                        a href=(ruta) class=(if *ruta == activa { "activo" } else { "" }) { (nombre) }
                    }
                }
                main.contenido { (cuerpo) }
            }
        }
    }
}

fn fecha(d: chrono::DateTime<chrono::Utc>) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// El feed: rejilla de los videos más recientes de las suscripciones.
pub fn feed_pagina(videos: &[VideoFeed]) -> Markup {
    envoltura(
        "Feed",
        "/",
        html! {
            div.cabecera {
                h1 { "Feed" }
                form method="post" action="/actualizar" {
                    button.boton type="submit" { "Actualizar" }
                }
            }
            @if videos.is_empty() {
                p.vacio {
                    "No hay videos todavía. Añade canales en "
                    a href="/suscripciones" { "Suscripciones" }
                    " y pulsa Actualizar."
                }
            } @else {
                div.rejilla {
                    @for v in videos {
                        a.tarjeta href=(format!("/ver/{}", v.video_id)) {
                            @if let Some(t) = &v.thumbnail {
                                img.miniatura src=(t) loading="lazy" alt="";
                            } @else {
                                div.miniatura.sinimg {}
                            }
                            div.meta {
                                span.titulo { (v.title) }
                                span.canal { (v.channel_title) }
                                span.fecha { (fecha(v.published)) }
                            }
                        }
                    }
                }
            }
        },
    )
}

/// Suscripciones: alta por id/URL/handle, y la lista con su botón de quitar.
pub fn suscripciones_pagina(subs: &[Suscripcion], mensaje: Option<&str>, error: bool) -> Markup {
    envoltura(
        "Suscripciones",
        "/suscripciones",
        html! {
            h1 { "Suscripciones" }
            @if let Some(m) = mensaje {
                p class=(if error { "aviso error" } else { "aviso ok" }) { (m) }
            }
            form.alta method="post" action="/suscripciones" {
                input type="text" name="canal" placeholder="id UC…, URL del canal o @handle" required;
                button.boton type="submit" { "Añadir" }
            }
            form.importar method="post" action="/suscripciones/importar" enctype="multipart/form-data" {
                label.rotulo { "Importar de YouTube (Takeout " code { "subscriptions.csv" } "):" }
                input type="file" name="archivo" accept=".csv,text/csv" required;
                button.boton type="submit" { "Importar" }
            }
            @if subs.is_empty() {
                p.vacio { "Sin suscripciones. Añade un canal arriba." }
            } @else {
                ul.suscripciones {
                    @for s in subs {
                        li {
                            span.canal { (s.title) }
                            span.id { (s.channel_id) }
                            form method="post" action=(format!("/suscripciones/{}/borrar", s.channel_id)) {
                                button.boton.peligro type="submit" { "Quitar" }
                            }
                        }
                    }
                }
            }
        },
    )
}

/// Página de búsqueda: formulario + rejilla de resultados. Cada tarjeta enlaza a
/// `/ver/{id}` llevando título y canal (para pintarlos sin tenerlos en la BD).
/// La miniatura sale del patrón estable de YouTube (i.ytimg.com/vi/{id}).
pub fn buscar_pagina(query: &str, resultados: &[crate::extractor::ResultadoBusqueda]) -> Markup {
    envoltura(
        "Buscar",
        "/buscar",
        html! {
            h1 { "Buscar" }
            form.alta method="get" action="/buscar" {
                input type="text" name="q" value=(query) placeholder="Buscar videos en YouTube…" required;
                button.boton type="submit" { "Buscar" }
            }
            @if !query.is_empty() && resultados.is_empty() {
                p.vacio { "Sin resultados para «" (query) "»." }
            } @else if !resultados.is_empty() {
                div.rejilla {
                    @for r in resultados {
                        a.tarjeta href=(format!("/ver/{}?t={}&c={}", r.id, pct(&r.title), pct(&r.channel))) {
                            img.miniatura src=(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", r.id)) loading="lazy" alt="";
                            div.meta {
                                span.titulo { (r.title) }
                                span.canal { (r.channel) }
                            }
                        }
                    }
                }
            }
        },
    )
}

/// Página de reproducción: el `<video>` apunta a `/stream/{id}`, que resuelve con
/// yt-dlp. Reproducción nativa del navegador (controles, seek por Range).
pub fn ver_pagina(v: &VideoFeed) -> Markup {
    envoltura(
        &v.title,
        "/",
        html! {
            div.reproductor {
                video.video controls autoplay preload="auto" src=(format!("/stream/{}", v.video_id)) {
                    "Tu navegador no puede reproducir este video."
                }
                h1.titulo { (v.title) }
                p.canal { (v.channel_title) " · " (fecha(v.published)) }
                p { a href="/" { "← Volver al feed" } }
            }
        },
    )
}

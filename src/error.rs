// SPDX-License-Identifier: AGPL-3.0-or-later
//! Error único de mirador. En la UI nativa (GTK) se muestra como texto de estado;
//! en el frontal WebView (feature `webview`) cruza la frontera HTTP como una
//! página simple, no como un 500 mudo: un fallo de red al bajar un feed o de
//! yt-dlp al resolver un stream se ve, no se traga (directiva 20).

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("base de datos: {0}")]
    Bd(#[from] sqlx::Error),
    #[error("migración: {0}")]
    Migracion(#[from] sqlx::migrate::MigrateError),
    #[error("red: {0}")]
    Red(#[from] reqwest::Error),
    #[error("feed mal formado: {0}")]
    Feed(String),
    #[error("no se pudo resolver el video: {0}")]
    Resolver(String),
    #[error("entrada inválida: {0}")]
    Entrada(String),
}

pub type Resultado<T> = std::result::Result<T, Error>;

#[cfg(feature = "webview")]
use axum::http::StatusCode;
#[cfg(feature = "webview")]
use axum::response::{IntoResponse, Response};

#[cfg(feature = "webview")]
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let codigo = match &self {
            Error::Entrada(_) => StatusCode::BAD_REQUEST,
            Error::Resolver(_) | Error::Feed(_) => StatusCode::BAD_GATEWAY,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        tracing::warn!("error {codigo}: {self}");
        let cuerpo = maud::html! {
            (maud::DOCTYPE)
            html lang="es" {
                head { meta charset="utf-8"; title { "mirador — error" } }
                body {
                    h1 { "Algo falló" }
                    p { (self.to_string()) }
                    p { a href="/" { "← Volver al feed" } }
                }
            }
        };
        (codigo, cuerpo).into_response()
    }
}

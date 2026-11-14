// SPDX-License-Identifier: AGPL-3.0-or-later
//! mirador — reproductor de YouTube local: SOLO reproducción y suscripciones.
//! Un binario, base local, sin cuenta ni tracking. La extracción del stream la
//! hace yt-dlp (patrón nexus).
//!
//! Frontal por DEFECTO: **GTK4 nativo** (`escritorio`), el estándar de escritorio
//! (sin componentes de visualización web). El viejo frontal por WebView
//! (axum+maud+tao/wry) queda tras `--features webview` (`servidor`+`gui`+`vista`),
//! como referencia mientras se retira.

mod db;
mod error;
mod escritorio;
mod extractor;
mod importar;
mod nucleo;
mod rss;

#[cfg(feature = "webview")]
mod gui;
#[cfg(feature = "webview")]
mod servidor;
#[cfg(feature = "webview")]
mod vista;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();

    // El bucle de la ventana (GTK) corre en el hilo PRINCIPAL y es bloqueante; el
    // núcleo async corre en los hilos de fondo del runtime. Por eso no se usa
    // `#[tokio::main]`: se crea el runtime a mano y el hilo principal queda libre
    // para GTK. `rt` vive hasta que la ventana se cierra (mantiene los hilos).
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("no se pudo crear el runtime: {e}");
            std::process::exit(1);
        }
    };

    #[cfg(feature = "webview")]
    {
        servidor::correr(&rt);
        return;
    }

    #[cfg(not(feature = "webview"))]
    {
        // Prepara el núcleo (base, HTTP, caché) en el runtime; `_pot` mantiene
        // vivo el proveedor de PoToken mientras corra mirador.
        let (nucleo, _pot) = match rt.block_on(nucleo::preparar()) {
            Ok(n) => n,
            Err(e) => {
                tracing::error!("mirador no arrancó: {e}");
                std::process::exit(1);
            }
        };
        let handle = rt.handle().clone();
        let _codigo = escritorio::correr(nucleo, handle);
        // La ventana se cerró; el runtime y el PoToken se sueltan al retornar.
    }
}

# mirador

Configuración general en `~/.claude/CLAUDE.md` (se carga en toda sesión). Esto
es lo propio de **mirador**.

## Qué es

Reproductor de **YouTube local**: SOLO **reproducción de videos** y **manejo de
suscripciones**. Un binario, base local, sin cuenta ni tracking. Inspirado en el
conjunto de funciones de FreeTube (AGPL), **desde cero** con nuestro stack — sin
código derivado. Nació el 2026-08-20.

## Stack — es una app de ESCRITORIO: GTK4 puro (directiva del 2026-09-09)

mirador es de **escritorio**, así que el estándar es **GTK4 nativo** (gtk4-rs
0.9), **sin componentes de visualización web** — como boza y chuspa. El estándar
web (`maud`+htmx+`boi`) NO aplica aquí; era el error de origen (una ventana
WebKitGTK cargando un servidor local).

- **Por defecto (`cargo build`)**: interfaz GTK4 pura. Núcleo `SQLx`/SQLite +
  `reqwest` + yt-dlp; la UI (`escritorio.rs`) llama al núcleo directamente, sin
  servidor. El puente async→interfaz es `async-channel` + `glib::spawn_future_local`
  (mismo patrón que boza), para no congelar la ventana con una descarga de yt-dlp.
- **Reproducción**: `gtk::Video` (GtkMediaFile → **GStreamer**) reproduce el mp4
  que yt-dlp deja en la caché (fichero local; la URL de googlevideo GStreamer no
  la abre). Necesita GStreamer + plugins H.264/AAC en el sistema; sin ellos la
  ventana abre y la lista funciona, el video no decodifica.
- **Frontal VIEJO por WebView** (axum+maud+tao/wry): tras `--features webview`,
  como referencia mientras se retira. Necesita gtk3+webkit2gtk-4.1 para
  compilar. htmx/boi vendorizados en `estaticos/` (`include_str!`) solo se
  compilan con esa feature.

## La pieza externa: yt-dlp

La extracción del stream reproducible la hace **yt-dlp**, invocado como proceso
externo (`src/extractor.rs`) — **mismo patrón que nexus con nuclei/subfinder**.
Es deliberado: YouTube rompe los extractores a propósito (cifrado de firma,
throttling `n`, PoToken), y delegar en yt-dlp mantiene esa fragilidad FUERA de
nuestro código (directiva 1). Debe estar en el `PATH`. Es la única dependencia
externa.

## Módulos

- `db.rs` — pool, migraciones, suscripciones y caché de videos. **Núcleo.**
- `rss.rs` — feed Atom keyless de un canal; el **parseo es puro** (probado sin
  red, directiva 28). **Núcleo.**
- `extractor.rs` — yt-dlp: resolver channel_id (id UC / URL / @handle), buscar y
  descargar el mp4 a la caché. **Núcleo.**
- `importar.rs` — parseo del `subscriptions.csv` de Takeout (puro). **Núcleo.**
- `nucleo.rs` — `Nucleo` (pool, http, cache_dir, lock de descarga) + `preparar()`
  + `asegurar_local()`. Compartido por la UI GTK y el WebView.
- `escritorio.rs` — **la UI GTK4 nativa** (por defecto): feed, buscar,
  suscripciones, y reproductor con `gtk::Video`.
- `main.rs` — arranque: crea el runtime tokio y despacha a `escritorio` (GTK) o,
  con `--features webview`, a `servidor`.
- `servidor.rs` / `gui.rs` / `vista.rs` — el frontal WebView viejo, gateados tras
  `webview`. Solo `127.0.0.1`.

## Correr

`cargo run` → abre la ventana GTK4. Env: `MIRADOR_DB` (y `MIRADOR_PORT` solo con
`--features webview`). El WebView viejo: `cargo run --features webview` →
ventana WebKitGTK sobre `http://127.0.0.1:8600` (necesita gtk3+webkit2gtk).

## Límites v1 (por diseño)

- Formato **progresivo combinado ≤720p** (los DASH separan pistas y necesitarían
  dash.js, que el stack evita).
- Miniaturas desde `i.ytimg.com` directo (proxear es mejora futura).
- Sin búsqueda, historial ni playlists: fuera del alcance pedido.

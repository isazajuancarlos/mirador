<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# mirador

Reproductor de **YouTube local**: solo **reproducción de videos** y **manejo de
suscripciones**. Un binario, base local, **sin cuenta, sin API-key, sin
tracking**. Inspirado en el conjunto de funciones de
[FreeTube](https://github.com/FreeTubeApp/FreeTube), desarrollado **desde cero**
con nuestro stack.

## Qué hace

- **Ventana nativa GTK4** (sin componentes web): mirador es una app de
  escritorio, no una página. La interfaz llama al núcleo directamente.
- **Suscripciones**: añades un canal por su id `UC…`, la URL del canal o su
  `@handle`; o **importas** el `subscriptions.csv` de Google Takeout (como
  FreeTube). Los videos llegan por el **feed RSS** público de YouTube (keyless).
- **Feed**: lista con los videos más recientes de tus suscripciones.
- **Buscar**: en YouTube, keyless (yt-dlp `ytsearch`).
- **Reproducción**: widget `GtkVideo` (GStreamer), con el stream resuelto por
  **yt-dlp** y reproducido desde el mp4 local en caché.

## Stack — app de ESCRITORIO en GTK4 puro

mirador es de **escritorio**, así que el estándar es **GTK4 nativo** (gtk4-rs),
sin componentes de visualización web — como boza y chuspa. (El estándar web
`maud`+htmx+`boi` es para lo servido en navegador, no para esto.)

- Por defecto: interfaz GTK4 pura; núcleo `SQLx`/SQLite + `reqwest` + yt-dlp. El
  puente async→interfaz es `async-channel` + `glib::spawn_future_local`.
- **Reproducción**: `GtkVideo` usa GStreamer; necesita sus plugins H.264/AAC en
  el sistema. Sin ellos la ventana abre y la lista funciona, el video no
  decodifica.
- Frontal viejo por WebView (tao+wry): tras `--features webview`, en retirada.

Dependencias externas (en el `PATH`): **yt-dlp** (extracción), **ffmpeg**
(remux), **node** (retos JS de yt-dlp) y **bgutil-pot** (proveedor de PoToken
para DASH HD; opcional — sin él cae a HLS 1080p). mirador arranca el proveedor
solo si lo encuentra.

**Reproducción de alta calidad:** al abrir un video, mirador lo **descarga y
remuxea** (yt-dlp+ffmpeg) al mejor DASH H.264 disponible (con PoToken: 1080p+),
a un mp4 con `faststart` en **caché** (`~/.cache/mirador/`), y lo sirve con
`Range`. Resultado: **sync A/V perfecto, seek total, y offline** (re-ver
instantáneo). La primera reproducción espera la descarga.

## Correr

```bash
./mirador                       # abre la ventana GTK4; auto-detecta el navegador para las cookies
MIRADOR_YTDLP_BROWSER=firefox ./mirador   # forzar navegador (si la autodetección no acierta)
MIRADOR_DB=sqlite://otra.db ./mirador     # otra base

# Frontal WebView viejo (en retirada; necesita gtk3 + webkit2gtk-4.1):
cargo run --features webview                    # ventana WebKitGTK
MIRADOR_HEADLESS=1 cargo run --features webview # solo servidor, sin ventana
MIRADOR_PORT=9000 cargo run --features webview  # otro puerto
```

Auto-detecta el navegador con perfil (Brave, Chrome, Chromium o Firefox, en ese
orden) para leer sus cookies — necesarias contra el bot-check de YouTube. Si
quieres forzar otro o un `cookies.txt`, usa `MIRADOR_YTDLP_BROWSER` o
`MIRADOR_YTDLP_COOKIES`.

Es un reproductor personal: todo es local, nada se expone (con `--features
webview` el servidor solo escucha en `127.0.0.1`). Para **importar** tus
suscripciones: en YouTube → tu foto → *Tus datos de YouTube* → Google Takeout →
descarga, y en mirador (**Suscripciones → Importar**) sube el
`subscriptions.csv`; luego **Actualizar**.

## Instalar (RPM, openSUSE / Fedora)

```bash
cargo build --release && cargo generate-rpm
sudo rpm -Uvh target/generate-rpm/mirador-*.x86_64.rpm
```

El número de versión no se escribe aquí a propósito: envejece. Sale de
`Cargo.toml`. El RPM instala el binario (`/usr/bin/mirador`), el lanzador del
menú y el icono. Las dependencias **duras** son las librerías de **GTK4**
(libgtk-4, gdk, pango, cairo…), que el empaquetado detecta solo. **yt-dlp,
ffmpeg, node** y los **plugins GStreamer** (H.264/AAC, la decodificación de
`GtkVideo`) van como *recommends* (deps débiles): sin ellos mirador arranca y la
lista funciona, con ellos reproduce — no se declaran como `requires` porque en
Fedora/openSUSE ffmpeg y varios plugins no siempre están en los repos por
defecto y un requisito duro dejaría el paquete sin instalar.

### Cookies (bot-check de YouTube)

Desde muchas IPs (datacenter sobre todo) YouTube responde *«Sign in to confirm
you're not a bot»* y yt-dlp necesita **cookies** para extraer. Se aportan con tu
navegador logueado (yt-dlp las lee directamente; mirador nunca las ve):

```bash
MIRADOR_YTDLP_BROWSER=firefox cargo run      # o chrome, brave, chromium…
# o un archivo cookies.txt exportado:
MIRADOR_YTDLP_COOKIES=/ruta/cookies.txt cargo run
```

Sin esto puede funcionar en una IP residencial «limpia», pero si ves el error
anti-bot, configura una de las dos.

## Límites conocidos

- La reproducción **descarga y remuxea** el video (yt-dlp + ffmpeg) antes de
  reproducir, así que la **primera** vez hay una espera; luego queda en caché y
  se re-ve al instante y offline.
- Calidad: con **bgutil-pot** (PoToken) el cliente web expone DASH H.264 hasta
  1080p+; sin él cae a HLS 1080p. Siempre H.264/AAC (lo que los plugins GStreamer
  decodifican sin extras raros), nunca VP9/AV1.
- Miniaturas: la UI GTK v1 muestra filas de texto (título + canal + fecha); las
  miniaturas son una mejora futura.
- Videos no disponibles (premieres, solo-miembros, geo-bloqueados, borrados) no
  reproducen: el feed los oculta y la reproducción lo dice en vez de negro.
- Sin historial ni playlists: fuera del alcance pedido (reproducción,
  suscripciones y búsqueda).

## Licencia

AGPL-3.0-or-later © 2026 Isaza Arenas. Ver `LICENSE` y `NOTICE`.

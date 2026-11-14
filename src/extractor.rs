// SPDX-License-Identifier: AGPL-3.0-or-later
//! yt-dlp: la ÚNICA pieza externa. El servidor lo invoca —como nexus a nuclei o
//! subfinder— para (a) resolver el channel_id de una URL/handle y (b) obtener la
//! URL del stream reproducible. YouTube rompe los extractores a propósito;
//! delegar en yt-dlp, que sigue esos cambios, mantiene esa fragilidad FUERA de
//! nuestro código (directiva 1: no reimplementar lo que otro mantiene).

use crate::error::{Error, Resultado};
use tokio::process::Command;

const YTDLP: &str = "yt-dlp";

/// Argumentos de autenticación de yt-dlp, si el usuario los configuró. YouTube
/// exige cookies/PoToken contra su bot-check desde muchas IPs (datacenter sobre
/// todo): el usuario las aporta con su navegador logueado
/// (`MIRADOR_YTDLP_BROWSER=firefox`) o un `cookies.txt`
/// (`MIRADOR_YTDLP_COOKIES=/ruta`). yt-dlp las lee DIRECTAMENTE: mirador nunca
/// las ve ni las guarda. Sin config, se intenta sin cookies (funciona en IPs
/// residenciales «limpias»).
fn args_auth() -> Vec<String> {
    // 1) Config EXPLÍCITA del usuario, gana siempre.
    if let Ok(b) = std::env::var("MIRADOR_YTDLP_BROWSER")
        && !b.trim().is_empty()
    {
        return vec!["--cookies-from-browser".into(), b];
    }
    if let Ok(p) = std::env::var("MIRADOR_YTDLP_COOKIES")
        && !p.trim().is_empty()
    {
        return vec!["--cookies".into(), p];
    }
    // 2) AUTO-DETECTAR un navegador con perfil, para que baste `./mirador` sin
    //    prefijo (YouTube exige cookies contra el bot-check). El env de arriba
    //    permite forzar otro o un cookies.txt.
    if let Some(nav) = navegador_detectado() {
        return vec!["--cookies-from-browser".into(), nav.into()];
    }
    Vec::new()
}

/// Primer navegador con perfil en `$HOME` (Brave primero: es el habitual y sus
/// cookies bastan con `secretstorage`). None si no hay ninguno.
fn navegador_detectado() -> Option<&'static str> {
    let home = std::env::var_os("HOME")?;
    let home = std::path::Path::new(&home);
    let candidatos = [
        ("brave", ".config/BraveSoftware/Brave-Browser"),
        ("chrome", ".config/google-chrome"),
        ("chromium", ".config/chromium"),
        ("firefox", ".mozilla/firefox"),
    ];
    candidatos
        .into_iter()
        .find(|(_, ruta)| home.join(ruta).is_dir())
        .map(|(nombre, _)| nombre)
}

/// Habilita un runtime JS para yt-dlp si hay `node` en el PATH. SIN runtime JS,
/// yt-dlp no resuelve los retos de firma del cliente web de YouTube y cae al
/// cliente «tv downgraded», que devuelve «The page needs to be reloaded» (medido:
/// con `--js-runtimes node` el mismo video baja y GStreamer lo reproduce). deno
/// se usa por defecto solo; node hay que declararlo. Si no hay node, se deja
/// vacío (quien tenga deno funciona igual; quien no tenga ninguno verá el aviso
/// de yt-dlp en stderr).
fn args_js_runtime() -> Vec<String> {
    let hay_node = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path).any(|dir| {
                let p = dir.join("node");
                std::fs::metadata(&p).map(|m| m.is_file()).unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if hay_node {
        vec!["--js-runtimes".into(), "node".into()]
    } else {
        Vec::new()
    }
}

/// Traduce el fallo de yt-dlp a un error legible. El bot-check es el modo de
/// fallo esperado sin cookies, así que se nombra con la salida (directiva 20),
/// no se entierra en un stderr genérico.
fn error_ytdlp(contexto: &str, stderr: &[u8]) -> Error {
    let s = String::from_utf8_lossy(stderr);
    if s.contains("not a bot") || s.contains("Sign in to confirm") {
        Error::Resolver(
            "YouTube pide verificación anti-bot desde esta IP. Configura cookies: \
             MIRADOR_YTDLP_BROWSER=firefox (o chrome…), o MIRADOR_YTDLP_COOKIES=/ruta/cookies.txt"
                .into(),
        )
    } else {
        Error::Resolver(format!("{contexto}: {}", s.trim()))
    }
}

/// ¿Es ya un channel_id? `UC` + 22 de [A-Za-z0-9_-] = 24 chars ASCII.
pub fn es_channel_id(s: &str) -> bool {
    let s = s.trim();
    s.len() == 24
        && s.starts_with("UC")
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// ¿Es un id de video de YouTube? 11 chars de [A-Za-z0-9_-].
pub fn es_video_id(s: &str) -> bool {
    s.len() == 11 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Extrae un channel_id de una URL `/channel/UC…` SIN red. None si no lo lleva.
pub fn channel_id_de_url(s: &str) -> Option<String> {
    let marca = "/channel/";
    let i = s.find(marca)? + marca.len();
    let resto: String = s[i..]
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
        .map(|b| b as char)
        .collect();
    es_channel_id(&resto).then_some(resto)
}

/// Resuelve la entrada del usuario a un channel_id. Lo directo (id UC o URL
/// `/channel/`) NO toca la red; un handle (@x), `/c/`, `/user/` o URL suelta lo
/// resuelve yt-dlp con una sola llamada.
pub async fn resolver_channel_id(entrada: &str) -> Resultado<String> {
    let e = entrada.trim();
    if e.is_empty() {
        return Err(Error::Entrada("canal vacío".into()));
    }
    if es_channel_id(e) {
        return Ok(e.to_string());
    }
    if let Some(id) = channel_id_de_url(e) {
        return Ok(id);
    }
    let url = if e.starts_with("http") {
        e.to_string()
    } else if let Some(h) = e.strip_prefix('@') {
        format!("https://www.youtube.com/@{h}")
    } else {
        format!("https://www.youtube.com/@{e}")
    };
    let mut args = args_auth();
    args.extend(args_js_runtime());
    args.extend(
        ["--no-warnings", "--playlist-items", "0", "--print", "channel_id", url.as_str()]
            .map(String::from),
    );
    let salida = Command::new(YTDLP)
        .args(&args)
        .output()
        .await
        .map_err(|err| Error::Resolver(format!("no se pudo ejecutar yt-dlp: {err}")))?;
    if !salida.status.success() {
        return Err(error_ytdlp(&format!("no se resolvió el canal de «{entrada}»"), &salida.stderr));
    }
    let id = String::from_utf8_lossy(&salida.stdout).trim().to_string();
    es_channel_id(&id)
        .then_some(id.clone())
        .ok_or_else(|| Error::Resolver(format!("yt-dlp devolvió un id inesperado: {id}")))
}

/// Un resultado de búsqueda (metadatos, sin resolver el stream).
pub struct ResultadoBusqueda {
    pub id: String,
    pub title: String,
    pub channel: String,
}

/// Busca videos en YouTube (keyless) con `yt-dlp ytsearchN:` en modo
/// `--flat-playlist` (solo metadatos, rápido, no resuelve cada stream). Devuelve
/// hasta `n` resultados.
pub async fn buscar(query: &str, n: u32) -> Resultado<Vec<ResultadoBusqueda>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let mut cmd = Command::new(YTDLP);
    cmd.args(args_auth());
    cmd.args(args_js_runtime());
    cmd.arg("--no-warnings")
        .arg("--flat-playlist")
        .arg("--print")
        .arg("%(id)s\t%(title)s\t%(channel)s")
        .arg(format!("ytsearch{n}:{q}"));
    let salida = cmd
        .output()
        .await
        .map_err(|e| Error::Resolver(format!("no se pudo ejecutar yt-dlp: {e}")))?;
    if !salida.status.success() {
        return Err(error_ytdlp("búsqueda", &salida.stderr));
    }
    let mut out = Vec::new();
    for linea in String::from_utf8_lossy(&salida.stdout).lines() {
        let mut campos = linea.splitn(3, '\t');
        if let (Some(id), Some(title)) = (campos.next(), campos.next())
            && es_video_id(id)
        {
            out.push(ResultadoBusqueda {
                id: id.to_string(),
                title: title.to_string(),
                channel: campos.next().unwrap_or("").to_string(),
            });
        }
    }
    Ok(out)
}

/// Construye el comando yt-dlp que DESCARGA el stream a stdout (`-o -`), listo
/// para pipear a la respuesta. Que descargue yt-dlp —y no un fetch nuestro de la
/// URL— es lo que evita el **403**: googlevideo ata la URL al cliente que la
/// pidió (medido: la URL resuelta da 403 a curl/reqwest), así que solo yt-dlp,
/// con su contexto, la lee. Itags concretos (`22`=720p / `18`=360p, progresivos
/// H.264+AAC que WebKitGTK reproduce), NO `best[...]` —que enumera formatos con
/// el cliente web y dispara el bot-check «Sign in to confirm you're not a bot»—.
/// None si el id no es válido (no meter basura en la línea de órdenes).
pub async fn descargar(video_id: &str, cache_dir: &std::path::Path) -> Resultado<std::path::PathBuf> {
    if !es_video_id(video_id) {
        return Err(Error::Entrada(format!("id de video inválido: {video_id}")));
    }
    let url = format!("https://www.youtube.com/watch?v={video_id}");
    let plantilla = cache_dir.join(format!("{video_id}.%(ext)s"));
    let mut cmd = Command::new(YTDLP);
    cmd.args(args_auth());
    cmd.args(args_js_runtime());
    cmd.arg("--no-warnings")
        // Cliente `web` (DASH HD con PoToken) + `default` (HLS 96 de respaldo si
        // no hay PoToken). Con los dos, yt-dlp elige el mejor formato disponible.
        .arg("--extractor-args")
        .arg("youtube:player_client=web,default")
        .arg("--geo-bypass")
        .arg("-N")
        .arg("16")
        // Mejor DASH H.264 ≤1080p + AAC (calidad y sync limpios al muxear con
        // ffmpeg); si no, HLS muxeado, progresivo, o cualquiera. Siempre avc1
        // (WebKitGTK/GStreamer); nunca VP9/AV1.
        .arg("-f")
        .arg("bv*[vcodec^=avc1][height<=1080]+ba[acodec^=mp4a]/b[vcodec^=avc1][acodec^=mp4a]/22/18/b[ext=mp4]/b")
        .arg("--merge-output-format")
        .arg("mp4")
        // faststart: moov al FRENTE del mp4 → se reproduce y se hace SEEK desde
        // el primer byte servido (a diferencia del frag, sin deriva de A/V).
        .arg("--postprocessor-args")
        .arg("ffmpeg:-movflags +faststart")
        .arg("-o")
        .arg(&plantilla)
        .arg(&url);
    let salida = cmd
        .output()
        .await
        .map_err(|e| Error::Resolver(format!("no se pudo ejecutar yt-dlp: {e}")))?;
    if !salida.status.success() {
        return Err(error_ytdlp(&format!("no se pudo bajar {video_id}"), &salida.stderr));
    }
    let destino = cache_dir.join(format!("{video_id}.mp4"));
    if destino.exists() {
        Ok(destino)
    } else {
        Err(Error::Resolver(format!("{video_id}: yt-dlp terminó sin dejar el mp4")))
    }
}

/// Arranca el proveedor de PoToken (bgutil-pot) si hay binario y el puerto 4416
/// no responde. Devuelve el hijo para que el llamante lo mantenga vivo (muere
/// con mirador). Best-effort: si no hay binario, se sigue sin DASH (HLS 96).
pub fn arrancar_potoken() -> Option<tokio::process::Child> {
    // ¿Ya hay un proveedor escuchando? (systemd, otra instancia). No dupликar.
    if std::net::TcpStream::connect(("127.0.0.1", 4416)).is_ok() {
        tracing::info!("PoToken: proveedor ya activo en 4416; no se duplica");
        return None;
    }
    let bin = std::env::var("MIRADOR_POT_BIN").unwrap_or_else(|_| "bgutil-pot".into());
    match Command::new(&bin)
        .arg("server")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(hijo) => {
            tracing::info!("PoToken: proveedor bgutil-pot arrancado (DASH HD disponible)");
            Some(hijo)
        }
        Err(e) => {
            tracing::warn!("PoToken: sin proveedor ({e}); se reproducirá en HLS 1080p sin DASH");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconoce_channel_id_e_ids_de_video() {
        assert!(es_channel_id("UCBJycsmduvYEL83R_U4JriQ")); // 24 chars
        assert!(!es_channel_id("UCcorto"));
        assert!(!es_channel_id("XXBJycsmduvYEL83R_U4JriQ")); // no UC
        assert!(!es_channel_id("UCBJycsmduvYEL83R_U4Jri!")); // char inválido
        assert!(es_video_id("dQw4w9WgXcQ"));
        assert!(!es_video_id("corto"));
        assert!(!es_video_id("tiene espacio")); // 13, con espacio
    }

    #[test]
    fn extrae_channel_id_de_url_channel() {
        assert_eq!(
            channel_id_de_url("https://www.youtube.com/channel/UCBJycsmduvYEL83R_U4JriQ/videos"),
            Some("UCBJycsmduvYEL83R_U4JriQ".to_string())
        );
        // Un /@handle NO trae el id: no se inventa, se resuelve por red aparte.
        assert_eq!(channel_id_de_url("https://www.youtube.com/@mkbhd"), None);
        // Un /channel/ con id malformado tampoco cuela.
        assert_eq!(channel_id_de_url("https://youtube.com/channel/nomarcabien"), None);
    }
}

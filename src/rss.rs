// SPDX-License-Identifier: AGPL-3.0-or-later
//! El feed RSS (Atom) de un canal de YouTube: `feeds/videos.xml?channel_id=UC…`.
//! Keyless, sin cuenta, sin tracking. El PARSEO es puro (se prueba sin red,
//! directiva 28); `bajar_feed` solo añade el viaje HTTP.

use crate::db::VideoNuevo;
use crate::error::{Error, Resultado};
use chrono::{DateTime, Utc};
use quick_xml::events::Event;
use quick_xml::Reader;

pub struct FeedCanal {
    pub channel_title: String,
    pub videos: Vec<VideoNuevo>,
}

/// URL del feed Atom de un canal por su id UC…
pub fn url_feed(channel_id: &str) -> String {
    format!("https://www.youtube.com/feeds/videos.xml?channel_id={channel_id}")
}

/// Baja y parsea el feed de un canal.
pub async fn bajar_feed(client: &reqwest::Client, channel_id: &str) -> Resultado<FeedCanal> {
    let xml = client.get(url_feed(channel_id)).send().await?.error_for_status()?.text().await?;
    parsear_feed(&xml, channel_id)
}

/// Parsea el Atom de YouTube: el `<title>` de nivel-feed es el nombre del canal;
/// cada `<entry>` trae `yt:videoId`, su `title`, `published` y el primer
/// `media:thumbnail`. Puro. FALLA RUIDOSO si no reconoce ni el canal (directiva
/// 20): un feed vacío o de otra forma no se convierte en «canal sin nombre».
pub fn parsear_feed(xml: &str, channel_id: &str) -> Resultado<FeedCanal> {
    let mut reader = Reader::from_str(xml);
    // NO `trim_text(true)`: con el split de entidades de 0.41 recortaría cada trozo y
    // se perderían los espacios internos (`"a &amp; b"` → `"a&b"`). Se recorta el
    // acumulado completo al cerrar el elemento.
    reader.config_mut().trim_text(false);

    let mut channel_title: Option<String> = None;
    let mut videos = Vec::new();

    let mut en_entry = false;
    // Campos de la entry en curso.
    let mut cur_id: Option<String> = None;
    let mut cur_title: Option<String> = None;
    let mut cur_pub: Option<String> = None;
    let mut cur_thumb: Option<String> = None;
    // Qué texto estamos capturando dentro de una entry, y su acumulador. quick-xml
    // 0.41 parte el texto en las entidades (emite `Event::GeneralRef` por cada `&…;`),
    // así que el contenido se ACUMULA entre eventos y se asigna al cerrar el elemento.
    let mut capturando: Option<&'static str> = None;
    let mut buf = String::new();

    loop {
        match reader.read_event() {
            Err(e) => return Err(Error::Feed(format!("XML: {e}"))),
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => match e.name().as_ref() {
                b"entry" => {
                    en_entry = true;
                    cur_id = None;
                    cur_title = None;
                    cur_pub = None;
                    cur_thumb = None;
                }
                b"yt:videoId" if en_entry => capturando = Some("id"),
                b"title" => capturando = Some(if en_entry { "title" } else { "canal" }),
                b"published" if en_entry => capturando = Some("pub"),
                _ => {}
            },
            Ok(Event::Empty(e)) => {
                // media:thumbnail es un elemento vacío con atributo url.
                if en_entry && cur_thumb.is_none() && e.name().as_ref() == b"media:thumbnail" {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"url"
                            && let Ok(v) = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        {
                            cur_thumb = Some(v.into_owned());
                        }
                    }
                }
            }
            // El texto ya no lleva las entidades (van como GeneralRef); se decodifica
            // (bytes→str) y se acumula. `trim_text(true)` ya recorta los bordes.
            Ok(Event::Text(t)) => {
                if capturando.is_some()
                    && let Ok(d) = t.decode()
                {
                    buf.push_str(&d);
                }
            }
            // Una entidad dentro del texto capturado (`&amp;`, `&#39;`, …): se resuelve
            // reconstruyéndola y desescapándola con la tabla de la librería, y se acumula.
            Ok(Event::GeneralRef(r)) => {
                if capturando.is_some()
                    && let Ok(nombre) = std::str::from_utf8(&r)
                    && let Ok(u) = quick_xml::escape::unescape(&format!("&{nombre};"))
                {
                    buf.push_str(&u);
                }
            }
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"entry" => {
                    en_entry = false;
                    if let (Some(id), Some(title), Some(published)) =
                        (cur_id.take(), cur_title.take(), parsear_fecha(cur_pub.take()))
                    {
                        videos.push(VideoNuevo {
                            video_id: id,
                            channel_id: channel_id.to_string(),
                            title,
                            published,
                            thumbnail: cur_thumb.take(),
                        });
                    }
                }
                // Fin del elemento capturado: el acumulado es su contenido completo.
                _ => {
                    if let Some(destino) = capturando.take() {
                        let txt = std::mem::take(&mut buf).trim().to_string();
                        match destino {
                            "canal" if channel_title.is_none() => channel_title = Some(txt),
                            "id" => cur_id = Some(txt),
                            "title" => cur_title = Some(txt),
                            "pub" => cur_pub = Some(txt),
                            _ => {}
                        }
                    }
                }
            },
            _ => {}
        }
    }

    let channel_title = channel_title
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| Error::Feed(format!("el feed de {channel_id} no trae nombre de canal")))?;
    Ok(FeedCanal { channel_title, videos })
}

fn parsear_fecha(s: Option<String>) -> Option<DateTime<Utc>> {
    let s = s?;
    DateTime::parse_from_rfc3339(s.trim()).ok().map(|d| d.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ATOM: &str = r#"<?xml version="1.0"?>
    <feed xmlns:yt="http://www.youtube.com/xml/schemas/2015" xmlns:media="http://search.yahoo.com/mrss/">
      <title>Canal de Prueba</title>
      <entry>
        <yt:videoId>abc123DEF45</yt:videoId>
        <title>Primer video &amp; algo</title>
        <published>2026-08-12T14:00:35+00:00</published>
        <media:group><media:thumbnail url="https://i.ytimg.com/vi/abc123DEF45/hqdefault.jpg" width="480"/></media:group>
      </entry>
      <entry>
        <yt:videoId>xyz789GHI00</yt:videoId>
        <title>Segundo</title>
        <published>2026-08-01T00:05:00+00:00</published>
      </entry>
    </feed>"#;

    #[test]
    fn parsea_canal_y_entradas() {
        let f = parsear_feed(ATOM, "UCtest").unwrap();
        assert_eq!(f.channel_title, "Canal de Prueba", "el título de nivel-feed es el canal");
        assert_eq!(f.videos.len(), 2);
        let v0 = &f.videos[0];
        assert_eq!(v0.video_id, "abc123DEF45");
        assert_eq!(v0.title, "Primer video & algo", "las entidades se desescapan");
        assert_eq!(v0.channel_id, "UCtest");
        assert_eq!(v0.thumbnail.as_deref(), Some("https://i.ytimg.com/vi/abc123DEF45/hqdefault.jpg"));
        // El título del video NO se confunde con el del canal (el bug clásico de
        // capturar el primer <title> para todo).
        assert_ne!(v0.title, f.channel_title);
        // Segundo sin thumbnail: se acepta (None), no se descarta.
        assert_eq!(f.videos[1].thumbnail, None);
        assert_eq!(f.videos[1].video_id, "xyz789GHI00");
    }

    #[test]
    fn feed_sin_nombre_de_canal_falla_ruidoso() {
        // Sin <title> de nivel-feed → error, no «canal sin nombre» (directiva 20).
        let sin = r#"<feed xmlns:yt="x"><entry><yt:videoId>a</yt:videoId></entry></feed>"#;
        assert!(parsear_feed(sin, "UCx").is_err());
    }
}

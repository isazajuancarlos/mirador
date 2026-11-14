// SPDX-License-Identifier: AGPL-3.0-or-later
//! Importar suscripciones desde el `subscriptions.csv` que exporta Google
//! Takeout (`Takeout/YouTube and YouTube Music/subscriptions/`), como FreeTube.
//! Cabecera `Channel Id,Channel Url,Channel Title`; solo hace falta la columna 1
//! (el id `UC…`). El parseo es PURO (se prueba sin red ni archivos).

use crate::extractor::es_channel_id;

/// (channel_id, title) de las filas VÁLIDAS del CSV de Takeout. La cabecera y la
/// basura se descartan solas: `Channel Id` no es un id UC válido, así que el
/// filtro de `es_channel_id` la tira sin necesidad de comparar el encabezado
/// (que además tiene dos variantes de casing). Usa un lector RFC 4180 porque el
/// título puede llevar comas y comillas. Puro.
pub fn parsear_csv_youtube(texto: &str) -> Vec<(String, String)> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(texto.as_bytes());
    let mut out = Vec::new();
    for rec in rdr.records().flatten() {
        let id = rec.get(0).unwrap_or("").trim();
        if !es_channel_id(id) {
            continue; // cabecera, líneas vacías o basura
        }
        let title = rec
            .get(2)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(id)
            .to_string();
        out.push((id.to_string(), title));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn importa_takeout_descarta_cabecera_y_basura() {
        let csv = "Channel Id,Channel Url,Channel Title\n\
            UCBJycsmduvYEL83R_U4JriQ,http://www.youtube.com/channel/UCBJycsmduvYEL83R_U4JriQ,Marques Brownlee\n\
            UCXuqSBlHAE6Xw-yeJA0Tunw,http://www.youtube.com/channel/UCXuqSBlHAE6Xw-yeJA0Tunw,\"Linus Tech Tips, LLC\"\n\
            ,,\n\
            basura,no,es un id\n";
        let got = parsear_csv_youtube(csv);
        assert_eq!(got.len(), 2, "2 canales válidos; cabecera, vacía y basura fuera");
        assert_eq!(got[0], ("UCBJycsmduvYEL83R_U4JriQ".to_string(), "Marques Brownlee".to_string()));
        // El título con coma entre comillas se conserva ENTERO (RFC 4180).
        assert_eq!(got[1].1, "Linus Tech Tips, LLC");
    }

    #[test]
    fn tolera_variante_de_casing_de_la_cabecera() {
        // Algunas herramientas escriben "Channel ID"; da igual, la tira el filtro.
        let csv = "Channel ID,Channel URL,Channel title\nUCBJycsmduvYEL83R_U4JriQ,x,MKBHD\n";
        let got = parsear_csv_youtube(csv);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, "MKBHD");
    }

    #[test]
    fn sin_titulo_usa_el_id() {
        let csv = "UCBJycsmduvYEL83R_U4JriQ,url\n"; // solo 2 columnas
        let got = parsear_csv_youtube(csv);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, "UCBJycsmduvYEL83R_U4JriQ", "sin título, cae al id");
    }
}

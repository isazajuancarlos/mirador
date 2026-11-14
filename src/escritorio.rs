// SPDX-License-Identifier: AGPL-3.0-or-later
//! La interfaz de mirador en **GTK4 NATIVO** (estándar de escritorio: sin
//! componentes de visualización web). Reemplaza a la ventana tao+wry, que cargaba
//! el HTML del servidor local. Aquí no hay servidor: los widgets llaman al núcleo
//! (`db`, `rss`, `extractor`) directamente.
//!
//! El núcleo es `async` (tokio/SQLx/reqwest) y el bucle de GTK es del hilo
//! principal y NO puede bloquearse (una descarga de yt-dlp tarda segundos). Por
//! eso cada acción lanza su futuro en el runtime de tokio y el resultado vuelve al
//! hilo de la interfaz por un `async-channel` + `glib::spawn_future_local` —el
//! mismo patrón que boza usa para su hilo de copia—.
//!
//! REPRODUCCIÓN: `gtk::Video` usa `GtkMediaFile`, cuyo backend es **GStreamer**.
//! Reproduce el mp4 que yt-dlp deja en la caché (un fichero local, no la URL de
//! googlevideo —que GStreamer no abre—). Necesita GStreamer + plugins H.264/AAC
//! instalados en el sistema; sin ellos la ventana abre y la lista funciona, pero
//! el video no decodifica.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib, Application, ApplicationWindow};
use tokio::runtime::Handle;

use crate::nucleo::{self, Nucleo};
use crate::{db, extractor, importar, rss};

const APP_ID: &str = "com.xiliux.mirador";

/// Una acción de interfaz sin argumentos (recargar el feed, refrescar la lista).
/// Con enlace tardío cuando necesita referirse a sí misma (ver el diálogo de
/// suscripciones): la closure se guarda dentro del holder que ella consulta.
type Accion = Rc<dyn Fn()>;
type HolderAccion = Rc<RefCell<Option<Accion>>>;

/// Un elemento de la lista visible (feed o resultados de búsqueda). Lleva lo
/// mínimo para pintar la fila y para reproducir (`video_id`).
#[derive(Clone)]
struct Item {
    video_id: String,
    title: String,
    subtitulo: String,
}

impl From<&db::VideoFeed> for Item {
    fn from(v: &db::VideoFeed) -> Self {
        Item {
            video_id: v.video_id.clone(),
            title: v.title.clone(),
            subtitulo: format!("{} · {}", v.channel_title, v.published.format("%Y-%m-%d")),
        }
    }
}

impl From<&extractor::ResultadoBusqueda> for Item {
    fn from(r: &extractor::ResultadoBusqueda) -> Self {
        Item {
            video_id: r.id.clone(),
            title: r.title.clone(),
            subtitulo: r.channel.clone(),
        }
    }
}

/// Lanza `fut` en el runtime de tokio y ejecuta `en_ui` con su resultado EN EL
/// HILO de la interfaz. Es el puente entre el mundo async del núcleo y GTK.
fn ejecutar<T, F>(rt: &Handle, fut: F, en_ui: impl FnOnce(T) + 'static)
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = async_channel::bounded::<T>(1);
    rt.spawn(async move {
        let _ = tx.send(fut.await).await;
    });
    glib::spawn_future_local(async move {
        if let Ok(v) = rx.recv().await {
            en_ui(v);
        }
    });
}

/// Arranca la aplicación GTK. Bloquea hasta que se cierra la ventana.
pub fn correr(nucleo: Nucleo, rt: Handle) -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| construir(app, nucleo.clone(), rt.clone()));
    // GTK, por defecto, parsea `std::env::args`; mirador se configura por ENV
    // (MIRADOR_*), no por argumentos, así que se le pasa solo el nombre para que
    // no rechace nada.
    let args: [&str; 1] = ["mirador"];
    app.run_with_args(&args)
}

fn construir(app: &Application, nucleo: Nucleo, rt: Handle) {
    // El modelo de la lista visible; el índice de fila mapea a este vector.
    let items: Rc<RefCell<Vec<Item>>> = Rc::new(RefCell::new(Vec::new()));

    // --- Cabecera con acciones ---
    let cabecera = gtk::HeaderBar::new();
    let btn_actualizar = gtk::Button::from_icon_name("view-refresh-symbolic");
    btn_actualizar.set_tooltip_text(Some("Actualizar feeds"));
    let btn_subs = gtk::Button::from_icon_name("system-users-symbolic");
    btn_subs.set_tooltip_text(Some("Suscripciones"));
    let busqueda = gtk::SearchEntry::new();
    busqueda.set_placeholder_text(Some("Buscar en YouTube…"));
    busqueda.set_width_chars(28);
    cabecera.pack_start(&btn_actualizar);
    cabecera.pack_start(&btn_subs);
    cabecera.set_title_widget(Some(&busqueda));

    // --- Pila: lista de videos  |  reproductor ---
    let pila = gtk::Stack::new();
    pila.set_transition_type(gtk::StackTransitionType::Crossfade);

    // Lista.
    let lista = gtk::ListBox::new();
    lista.set_selection_mode(gtk::SelectionMode::None);
    lista.add_css_class("boxed-list");
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&lista)
        .build();
    let estado = gtk::Label::new(Some("Cargando el feed…"));
    estado.set_xalign(0.0);
    estado.set_margin_start(12);
    estado.set_margin_end(12);
    estado.set_margin_top(6);
    estado.set_margin_bottom(6);
    estado.set_wrap(true);
    let col_lista = gtk::Box::new(gtk::Orientation::Vertical, 0);
    col_lista.append(&scroll);
    col_lista.append(&estado);
    pila.add_named(&col_lista, Some("lista"));

    // Reproductor.
    let video = gtk::Video::new();
    video.set_autoplay(true);
    video.set_vexpand(true);
    video.set_hexpand(true);
    let btn_volver = gtk::Button::with_label("← Volver a la lista");
    btn_volver.set_halign(gtk::Align::Start);
    btn_volver.set_margin_start(12);
    btn_volver.set_margin_top(8);
    btn_volver.set_margin_bottom(8);
    let titulo_repro = gtk::Label::new(None);
    titulo_repro.set_xalign(0.0);
    titulo_repro.set_hexpand(true);
    titulo_repro.set_ellipsize(gtk::pango::EllipsizeMode::End);
    titulo_repro.set_margin_end(12);
    let barra_repro = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    barra_repro.append(&btn_volver);
    barra_repro.append(&titulo_repro);
    let col_repro = gtk::Box::new(gtk::Orientation::Vertical, 0);
    col_repro.append(&barra_repro);
    col_repro.append(&video);
    pila.add_named(&col_repro, Some("reproductor"));

    pila.set_visible_child_name("lista");

    let ventana = ApplicationWindow::builder()
        .application(app)
        .title("mirador")
        .default_width(1100)
        .default_height(780)
        .child(&pila)
        .build();
    ventana.set_titlebar(Some(&cabecera));

    // --- Reproducir un video: descargar (async) y apuntar GtkVideo al mp4 local ---
    let reproducir: Rc<dyn Fn(Item)> = {
        let nucleo = nucleo.clone();
        let rt = rt.clone();
        let pila = pila.clone();
        let video = video.clone();
        let estado = estado.clone();
        let titulo_repro = titulo_repro.clone();
        Rc::new(move |item: Item| {
            estado.set_text(&format!("Descargando «{}»… (yt-dlp)", item.title));
            let nucleo = nucleo.clone();
            let vid = item.video_id.clone();
            let titulo = item.title.clone();
            let pila = pila.clone();
            let video = video.clone();
            let estado = estado.clone();
            let titulo_repro = titulo_repro.clone();
            ejecutar(
                &rt,
                async move { nucleo::asegurar_local(&nucleo, &vid).await },
                move |res| match res {
                    Ok(ruta) => {
                        let archivo = gio::File::for_path(&ruta);
                        video.set_file(Some(&archivo));
                        titulo_repro.set_text(&titulo);
                        pila.set_visible_child_name("reproductor");
                        estado.set_text("Reproduciendo.");
                    }
                    Err(e) => {
                        estado.set_text(&format!("No se pudo reproducir: {e}"));
                    }
                },
            );
        })
    };

    // Fila activada → reproducir.
    lista.connect_row_activated(glib::clone!(
        #[strong] items,
        #[strong] reproducir,
        move |_, fila| {
            let idx = fila.index();
            if idx >= 0
                && let Some(item) = items.borrow().get(idx as usize).cloned()
            {
                reproducir(item);
            }
        }
    ));

    // Volver: soltar el video (para la reproducción) y mostrar la lista.
    btn_volver.connect_clicked(glib::clone!(
        #[weak] pila,
        #[weak] video,
        move |_| {
            video.set_file(None::<&gio::File>);
            pila.set_visible_child_name("lista");
        }
    ));

    // --- Cargar el feed (inicial y tras Actualizar / cerrar suscripciones) ---
    let cargar_feed: Rc<dyn Fn()> = {
        let nucleo = nucleo.clone();
        let rt = rt.clone();
        let lista = lista.clone();
        let estado = estado.clone();
        let items = items.clone();
        Rc::new(move || {
            estado.set_text("Cargando el feed…");
            let nucleo = nucleo.clone();
            let lista = lista.clone();
            let estado = estado.clone();
            let items = items.clone();
            ejecutar(
                &rt,
                async move { db::feed(&nucleo.pool, nucleo::LIMITE_FEED).await },
                move |res| match res {
                    Ok(videos) => {
                        let nuevos: Vec<Item> = videos.iter().map(Item::from).collect();
                        let n = nuevos.len();
                        poblar(&lista, &nuevos);
                        *items.borrow_mut() = nuevos;
                        estado.set_text(&if n == 0 {
                            "El feed está vacío. Añade suscripciones y pulsa Actualizar.".into()
                        } else {
                            format!("{n} videos en el feed.")
                        });
                    }
                    Err(e) => estado.set_text(&format!("Error al cargar el feed: {e}")),
                },
            );
        })
    };

    // Actualizar: bajar los feeds de todas las suscripciones y recargar.
    btn_actualizar.connect_clicked(glib::clone!(
        #[strong] nucleo,
        #[strong] rt,
        #[strong] estado,
        #[strong] cargar_feed,
        move |btn| {
            btn.set_sensitive(false);
            estado.set_text("Actualizando suscripciones…");
            let nucleo = nucleo.clone();
            let estado = estado.clone();
            let cargar_feed = cargar_feed.clone();
            let btn = btn.clone();
            ejecutar(
                &rt,
                async move { actualizar_todo(&nucleo).await },
                move |(ok, fallos)| {
                    estado.set_text(&format!("Actualizado: {ok} canales, {fallos} con fallo."));
                    btn.set_sensitive(true);
                    cargar_feed();
                },
            );
        }
    ));

    // Buscar: yt-dlp ytsearch, y poblar la lista con los resultados.
    busqueda.connect_activate(glib::clone!(
        #[strong] rt,
        #[strong] lista,
        #[strong] estado,
        #[strong] items,
        move |entry| {
            let q = entry.text().trim().to_string();
            if q.is_empty() {
                return;
            }
            estado.set_text(&format!("Buscando «{q}»…"));
            let lista = lista.clone();
            let estado = estado.clone();
            let items = items.clone();
            ejecutar(
                &rt,
                async move { extractor::buscar(&q, 24).await },
                move |res| match res {
                    Ok(resultados) => {
                        let nuevos: Vec<Item> = resultados.iter().map(Item::from).collect();
                        let n = nuevos.len();
                        poblar(&lista, &nuevos);
                        *items.borrow_mut() = nuevos;
                        estado.set_text(&format!("{n} resultados."));
                    }
                    Err(e) => estado.set_text(&format!("Error en la búsqueda: {e}")),
                },
            );
        }
    ));

    // Suscripciones: abre un diálogo propio.
    btn_subs.connect_clicked(glib::clone!(
        #[weak] ventana,
        #[strong] nucleo,
        #[strong] rt,
        #[strong] cargar_feed,
        move |_| {
            abrir_suscripciones(&ventana, nucleo.clone(), rt.clone(), cargar_feed.clone());
        }
    ));

    ventana.present();
    cargar_feed();
}

/// Vacía la ListBox y la puebla con las filas de `items`.
fn poblar(lista: &gtk::ListBox, items: &[Item]) {
    while let Some(hijo) = lista.first_child() {
        lista.remove(&hijo);
    }
    for it in items {
        let fila = gtk::Box::new(gtk::Orientation::Vertical, 2);
        fila.set_margin_top(8);
        fila.set_margin_bottom(8);
        fila.set_margin_start(10);
        fila.set_margin_end(10);
        let t = gtk::Label::new(Some(&it.title));
        t.set_xalign(0.0);
        t.set_wrap(true);
        t.add_css_class("heading");
        let s = gtk::Label::new(Some(&it.subtitulo));
        s.set_xalign(0.0);
        s.add_css_class("dim-label");
        fila.append(&t);
        fila.append(&s);
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&fila));
        row.set_activatable(true);
        lista.append(&row);
    }
}

/// Baja el feed de cada suscripción y lo vuelca. Falla SUAVE por canal (uno
/// caído no tumba el refresco). Devuelve (ok, fallos).
async fn actualizar_todo(nucleo: &Nucleo) -> (u32, u32) {
    let subs = match db::listar_suscripciones(&nucleo.pool).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("listar suscripciones: {e}");
            return (0, 0);
        }
    };
    let (mut ok, mut fallos) = (0u32, 0u32);
    for s in &subs {
        match rss::bajar_feed(&nucleo.http, &s.channel_id).await {
            Ok(feed) => match db::upsert_videos(&nucleo.pool, &feed.videos).await {
                Ok(()) => ok += 1,
                Err(e) => {
                    tracing::warn!("actualizar {}: {e}", s.channel_id);
                    fallos += 1;
                }
            },
            Err(e) => {
                tracing::warn!("actualizar {}: {e}", s.channel_id);
                fallos += 1;
            }
        }
    }
    (ok, fallos)
}

/// Diálogo de suscripciones: listar, añadir (por id/URL/@handle), quitar, e
/// importar el `subscriptions.csv` de Google Takeout.
fn abrir_suscripciones(
    padre: &ApplicationWindow,
    nucleo: Nucleo,
    rt: Handle,
    cargar_feed: Rc<dyn Fn()>,
) {
    let ventana = gtk::Window::builder()
        .title("Suscripciones")
        .transient_for(padre)
        .modal(true)
        .default_width(560)
        .default_height(520)
        .build();

    let raiz = gtk::Box::new(gtk::Orientation::Vertical, 10);
    raiz.set_margin_top(14);
    raiz.set_margin_bottom(14);
    raiz.set_margin_start(14);
    raiz.set_margin_end(14);

    // Añadir un canal.
    let fila_add = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let entrada = gtk::Entry::new();
    entrada.set_placeholder_text(Some("id UC…, URL o @handle"));
    entrada.set_hexpand(true);
    let btn_add = gtk::Button::with_label("Añadir");
    btn_add.add_css_class("suggested-action");
    fila_add.append(&entrada);
    fila_add.append(&btn_add);
    raiz.append(&fila_add);

    // Importar Takeout.
    let btn_import = gtk::Button::with_label("Importar subscriptions.csv (Takeout)…");
    raiz.append(&btn_import);

    // Lista de suscripciones.
    let subs_box = gtk::ListBox::new();
    subs_box.set_selection_mode(gtk::SelectionMode::None);
    subs_box.add_css_class("boxed-list");
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .child(&subs_box)
        .build();
    raiz.append(&scroll);

    let aviso = gtk::Label::new(None);
    aviso.set_xalign(0.0);
    aviso.set_wrap(true);
    raiz.append(&aviso);

    ventana.set_child(Some(&raiz));

    // `refrescar` rehace la lista, y cada botón "quitar" que crea necesita LLAMAR
    // a `refrescar` — una referencia a sí misma. Se resuelve con un holder de
    // enlace tardío: la closure se guarda dentro del `Rc<RefCell<Option<…>>>` que
    // ella misma consulta.
    let holder: HolderAccion = Rc::new(RefCell::new(None));
    let refrescar: Rc<dyn Fn()> = {
        let nucleo = nucleo.clone();
        let rt = rt.clone();
        let subs_box = subs_box.clone();
        let aviso = aviso.clone();
        let holder = holder.clone();
        Rc::new(move || {
            let nucleo = nucleo.clone();
            let rt_filas = rt.clone();
            let subs_box = subs_box.clone();
            let aviso = aviso.clone();
            let holder = holder.clone();
            let nucleo_q = nucleo.clone();
            ejecutar(
                &rt,
                async move { db::listar_suscripciones(&nucleo_q.pool).await },
                move |res| match res {
                    Ok(subs) => {
                        while let Some(h) = subs_box.first_child() {
                            subs_box.remove(&h);
                        }
                        for s in &subs {
                            let fila = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                            fila.set_margin_top(6);
                            fila.set_margin_bottom(6);
                            fila.set_margin_start(10);
                            fila.set_margin_end(10);
                            let lbl = gtk::Label::new(Some(&s.title));
                            lbl.set_xalign(0.0);
                            lbl.set_hexpand(true);
                            lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
                            let quitar = gtk::Button::from_icon_name("user-trash-symbolic");
                            quitar.set_tooltip_text(Some("Quitar canal"));
                            quitar.add_css_class("flat");
                            let channel_id = s.channel_id.clone();
                            quitar.connect_clicked(glib::clone!(
                                #[strong] nucleo,
                                #[strong] rt_filas,
                                #[strong] aviso,
                                #[strong] holder,
                                move |_| {
                                    let nucleo = nucleo.clone();
                                    let channel_id = channel_id.clone();
                                    let aviso = aviso.clone();
                                    let holder = holder.clone();
                                    ejecutar(
                                        &rt_filas,
                                        async move {
                                            db::quitar_suscripcion(&nucleo.pool, &channel_id).await
                                        },
                                        move |res| match res {
                                            Ok(()) => {
                                                aviso.set_text("Canal quitado.");
                                                if let Some(f) = holder.borrow().as_ref() {
                                                    f();
                                                }
                                            }
                                            Err(e) => aviso.set_text(&format!("Error al quitar: {e}")),
                                        },
                                    );
                                }
                            ));
                            fila.append(&lbl);
                            fila.append(&quitar);
                            let row = gtk::ListBoxRow::new();
                            row.set_child(Some(&fila));
                            row.set_activatable(false);
                            subs_box.append(&row);
                        }
                        aviso.set_text(&format!("{} suscripciones.", subs.len()));
                    }
                    Err(e) => aviso.set_text(&format!("Error: {e}")),
                },
            );
        })
    };
    *holder.borrow_mut() = Some(refrescar.clone());

    // Añadir un canal (resuelve, baja su feed, lo siembra).
    let hacer_add: Rc<dyn Fn()> = {
        let nucleo = nucleo.clone();
        let rt = rt.clone();
        let entrada = entrada.clone();
        let aviso = aviso.clone();
        let refrescar = refrescar.clone();
        Rc::new(move || {
            let texto = entrada.text().trim().to_string();
            if texto.is_empty() {
                return;
            }
            aviso.set_text("Añadiendo…");
            let nucleo = nucleo.clone();
            let entrada = entrada.clone();
            let aviso = aviso.clone();
            let refrescar = refrescar.clone();
            ejecutar(
                &rt,
                async move { agregar_canal(&nucleo, &texto).await },
                move |res| match res {
                    Ok(titulo) => {
                        aviso.set_text(&format!("Añadido: {titulo}"));
                        entrada.set_text("");
                        refrescar();
                    }
                    Err(e) => aviso.set_text(&format!("{e}")),
                },
            );
        })
    };
    btn_add.connect_clicked(glib::clone!(
        #[strong] hacer_add,
        move |_| hacer_add()
    ));
    entrada.connect_activate(glib::clone!(
        #[strong] hacer_add,
        move |_| hacer_add()
    ));

    // Importar CSV de Takeout.
    btn_import.connect_clicked(glib::clone!(
        #[weak] ventana,
        #[strong] nucleo,
        #[strong] rt,
        #[strong] aviso,
        #[strong] refrescar,
        move |_| {
            let dialog = gtk::FileDialog::builder()
                .title("Elegir subscriptions.csv de Google Takeout")
                .build();
            dialog.open(
                Some(&ventana),
                gio::Cancellable::NONE,
                glib::clone!(
                    #[strong] nucleo,
                    #[strong] rt,
                    #[strong] aviso,
                    #[strong] refrescar,
                    move |res| {
                        let Ok(archivo) = res else { return };
                        let Some(ruta) = archivo.path() else { return };
                        let texto = match std::fs::read_to_string(&ruta) {
                            Ok(t) => t,
                            Err(e) => {
                                aviso.set_text(&format!("No se pudo leer el archivo: {e}"));
                                return;
                            }
                        };
                        let items = importar::parsear_csv_youtube(&texto);
                        if items.is_empty() {
                            aviso.set_text(
                                "No se reconoció ningún canal (¿es el subscriptions.csv de Takeout?).",
                            );
                            return;
                        }
                        aviso.set_text(&format!("Importando {} canales…", items.len()));
                        let nucleo = nucleo.clone();
                        let aviso = aviso.clone();
                        let refrescar = refrescar.clone();
                        ejecutar(
                            &rt,
                            async move { db::importar_suscripciones(&nucleo.pool, &items).await },
                            move |res| match res {
                                Ok((nuevos, total)) => {
                                    aviso.set_text(&format!(
                                        "Importadas {total} ({nuevos} nuevas). Pulsa Actualizar para traer sus videos."
                                    ));
                                    refrescar();
                                }
                                Err(e) => aviso.set_text(&format!("Error al importar: {e}")),
                            },
                        );
                    }
                ),
            );
        }
    ));

    // Al cerrar el diálogo, recargar el feed principal (pudo cambiar).
    ventana.connect_close_request(glib::clone!(
        #[strong] cargar_feed,
        move |_| {
            cargar_feed();
            glib::Propagation::Proceed
        }
    ));

    refrescar();
    ventana.present();
}

/// Resuelve el canal, baja su feed y lo siembra. Devuelve el título del canal.
async fn agregar_canal(nucleo: &Nucleo, entrada: &str) -> crate::error::Resultado<String> {
    let channel_id = extractor::resolver_channel_id(entrada).await?;
    let feed = rss::bajar_feed(&nucleo.http, &channel_id).await?;
    db::agregar_suscripcion(&nucleo.pool, &channel_id, &feed.channel_title).await?;
    db::upsert_videos(&nucleo.pool, &feed.videos).await?;
    Ok(feed.channel_title)
}

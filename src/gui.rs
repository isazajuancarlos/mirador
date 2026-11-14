// SPDX-License-Identifier: AGPL-3.0-or-later
//! La ventana propia (tao + wry, WebKitGTK en Linux), como chuspa: mirador NO
//! depende del navegador del usuario. La ventana carga la interfaz del servidor
//! local embebido (`http://127.0.0.1:{puerto}`); el `<video>` reproduce DENTRO
//! del WebView. Debe correr en el hilo PRINCIPAL (GTK lo exige).

use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

/// Abre la ventana y corre el bucle de eventos (bloquea el hilo principal hasta
/// que se cierra). No retorna: `EventLoop::run` termina el proceso.
pub fn abrir_ventana(puerto: u16) -> Result<(), Box<dyn std::error::Error>> {
    let evento = EventLoop::new();
    let ventana = WindowBuilder::new()
        .with_title("mirador")
        .with_inner_size(tao::dpi::LogicalSize::new(1100.0, 780.0))
        .build(&evento)?;

    let constructor = WebViewBuilder::new().with_url(format!("http://127.0.0.1:{puerto}/"));

    // En Linux el motor se monta sobre el contenedor GTK de la ventana, no sobre
    // el manejador genérico: con `build(&ventana)` el sistema responde
    // `UnsupportedWindowHandle` y la ventana no aparece (lección de chuspa).
    #[cfg(target_os = "linux")]
    let _webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        constructor.build_gtk(ventana.default_vbox().ok_or("la ventana no expone su contenedor GTK")?)?
    };
    #[cfg(not(target_os = "linux"))]
    let _webview = constructor.build(&ventana)?;

    evento.run(move |evento, _, control| {
        *control = ControlFlow::Wait;
        if let Event::WindowEvent { event: WindowEvent::CloseRequested, .. } = evento {
            *control = ControlFlow::Exit;
        }
    })
}

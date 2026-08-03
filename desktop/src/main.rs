use std::sync::Arc;

use gpui::*;
use gpui_component::button::*;
use gpui_component::*;
use gpui_platform::application;

pub struct HelloWorld;

impl Render for HelloWorld {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_2()
            .size_full()
            .items_center()
            .justify_center()
            .child("Hello, World!")
            .child(
                Button::new("ok")
                    .primary()
                    .label("Let's Go!")
                    .on_click(|_ev, _window, _cx| println!("Clicked!")),
            )
    }
}

fn main() {
    // Initialize logging
    tracing_subscriber::fmt::init();

    application()
        .with_http_client(Arc::new(reqwest_client::ReqwestClient::new()))
        .run(move |cx| {
            gpui_component::init(cx);

            // Set up the window bounds
            let bounds = Bounds::centered(None, size(px(960.0), px(720.0)), cx);

            // Set up the window options
            let opts = WindowOptions {
                window_background: WindowBackgroundAppearance::Opaque,
                window_decorations: Some(WindowDecorations::Client),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                kind: WindowKind::Normal,
                app_id: Some("Signed".to_owned()),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::new_static("Signed Platform")),
                    traffic_light_position: Some(point(px(9.0), px(9.0))),
                    appears_transparent: true,
                }),
                ..Default::default()
            };

            cx.spawn(async move |cx| {
                cx.open_window(opts, |window, cx| {
                    let view = cx.new(|_| HelloWorld);
                    cx.new(|cx| Root::new(view, window, cx))
                })
                .ok();
            })
            .detach();

            // Bring the app to the foreground
            cx.activate(true);
        });
}

mod ai;
mod app;
mod calendar;
mod edit;
mod hl;
mod icloud;
mod ics;
mod md;
mod mirror;
mod model;
mod pengu;
mod theme;

fn main() -> eframe::Result {
    let opts = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([760.0, 480.0])
            .with_title("Penguin Notes")
            .with_app_id("penguin-notes")
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                    .expect("icon"),
            ),
        ..Default::default()
    };
    eframe::run_native(
        "Penguin Notes",
        opts,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}

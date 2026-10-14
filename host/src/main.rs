mod apps;
mod canvas;
mod dialogs;
mod grid;
mod icons;
mod outliner;
mod panel;
mod process;
mod tablet;
mod tabs;
mod toolbar;
mod widgets;
mod window;

fn main() -> eframe::Result<()> {
    let requested = apps::requested();
    let child = match apps::choose(apps::discover(), requested.as_deref()) {
        Some(app) => process::Running::start(&app.executable),
        None => process::Running::not_started(),
    };
    window::run(child)
}

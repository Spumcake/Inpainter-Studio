mod apps;
mod ask;
mod canvas;
mod chat;
mod dialogs;
mod doc;
mod grid;
mod icons;
mod outliner;
mod panel;
mod process;
mod route;
mod tablet;
mod tabs;
mod toolbar;
mod widgets;
mod window;
mod workspace;

fn main() -> eframe::Result<()> {
    // The lock is taken here, outside the window, so the window's own checks
    // never take it. The system releases it when this process ends.
    let _lock = match workspace::acquire_lock() {
        Ok(lock) => lock,
        Err(fail) => {
            if workspace::second_start_message(fail).is_some() {
                dialogs::already_open();
            } else {
                dialogs::unavailable();
            }
            return Ok(());
        }
    };
    let specs = apps::launch()
        .into_iter()
        .map(|app| window::AppSpec {
            id: app.manifest.id,
            name: app.manifest.name,
            executable: Some(app.executable),
        })
        .collect();
    window::run(specs)
}

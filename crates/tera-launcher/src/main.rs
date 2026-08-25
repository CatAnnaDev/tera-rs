#[cfg(not(windows))]
fn main() {
    eprintln!("tera-launcher only runs on Windows (inside the CrossOver bottle)");
}

#[cfg(any(windows, test))]
mod serverlist;

#[cfg(windows)]
mod app;

#[cfg(windows)]
fn main() {
    if let Err(error) = app::run() {
        eprintln!("tera-launcher: {error}");
        std::process::exit(1);
    }
}

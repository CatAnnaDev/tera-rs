#[cfg(not(windows))]
fn main() {
    eprintln!("tera-scanner must be built for a Windows target (e.g. cargo build -p tera-scanner --target x86_64-pc-windows-gnu) and run inside the TERA wine bottle");
    std::process::exit(2);
}

#[cfg(windows)]
mod scanner;

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    scanner::run()
}

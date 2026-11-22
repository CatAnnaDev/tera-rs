use tera_mapper::{composite_decrypt, composite_encrypt, CompositeMapper};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).expect("path");
    let original = std::fs::read(&path)?;
    let crypto = composite_encrypt(&composite_decrypt(&original));
    println!("crypto round-trip identical: {}", crypto == original);
    let mapper = CompositeMapper::read(&path)?;
    let rebuilt = mapper.to_bytes();
    println!("full read/write identical: {}", rebuilt == original);
    println!("entries: {}", mapper.len());
    Ok(())
}

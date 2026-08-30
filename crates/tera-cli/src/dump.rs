use anyhow::{bail, Result};
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct DumpArgs {
    #[arg(long, help = "Process to read, as reported by ps")]
    pub pid: i32,
    #[arg(long, short, help = "Write the regions here instead of listing them")]
    pub out: Option<PathBuf>,
    #[arg(long, help = "Only regions the process can execute")]
    pub executable: bool,
    #[arg(long, default_value_t = 4096, help = "Skip regions smaller than this")]
    pub min_size: usize,
    #[arg(long, help = "Report every PE image found mapped in the process")]
    pub images: bool,
}

struct Image {
    address: u64,
    machine: u16,
    sections: u16,
    size_of_image: u32,
    entry: u32,
}

fn read_image(address: u64, bytes: &[u8]) -> Option<Image> {
    if !bytes.starts_with(b"MZ") || bytes.len() < 0x40 {
        return None;
    }
    let at = u32::from_le_bytes(bytes.get(0x3c..0x40)?.try_into().ok()?) as usize;
    if bytes.get(at..at + 4)? != b"PE\0\0" {
        return None;
    }
    let machine = u16::from_le_bytes(bytes.get(at + 4..at + 6)?.try_into().ok()?);
    let sections = u16::from_le_bytes(bytes.get(at + 6..at + 8)?.try_into().ok()?);
    let optional = at + 24;
    let entry = u32::from_le_bytes(bytes.get(optional + 16..optional + 20)?.try_into().ok()?);
    let size_of_image = u32::from_le_bytes(bytes.get(optional + 56..optional + 60)?.try_into().ok()?);
    Some(Image {
        address,
        machine,
        sections,
        size_of_image,
        entry,
    })
}

#[cfg(target_os = "macos")]
pub fn run(args: DumpArgs) -> Result<()> {
    let regions = crate::macos_memory::regions(args.pid)?;
    let wanted: Vec<&crate::macos_memory::Region> = regions
        .iter()
        .filter(|region| region.bytes.len() >= args.min_size)
        .filter(|region| !args.executable || region.executable())
        .collect();
    if wanted.is_empty() {
        bail!("nothing readable in pid {}", args.pid);
    }

    if args.images {
        let mut found = 0usize;
        for region in &wanted {
            let Some(image) = read_image(region.address, &region.bytes) else {
                continue;
            };
            println!(
                "{:#018x}  {} sections  entry {:#x}  image {} bytes  machine {:#06x}",
                image.address, image.sections, image.entry, image.size_of_image, image.machine
            );
            found += 1;
        }
        println!("{found} mapped PE image(s)");
        return Ok(());
    }

    match &args.out {
        None => {
            let mut total = 0usize;
            for region in &wanted {
                println!(
                    "{:#018x}  {:>12} bytes  {}",
                    region.address,
                    region.bytes.len(),
                    region.flags()
                );
                total += region.bytes.len();
            }
            println!("{} regions, {total} bytes readable", wanted.len());
        }
        Some(out) => {
            std::fs::create_dir_all(out)?;
            let mut total = 0usize;
            for region in &wanted {
                let target = out.join(format!("{:016x}.{}.bin", region.address, region.flags()));
                std::fs::write(&target, &region.bytes)?;
                total += region.bytes.len();
            }
            println!(
                "wrote {} regions, {total} bytes to {}",
                wanted.len(),
                out.display()
            );
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn run(_args: DumpArgs) -> Result<()> {
    bail!("reading another process is only implemented on macOS")
}

#[cfg(test)]
mod tests {
    use super::read_image;

    fn pe(sections: u16, entry: u32, size_of_image: u32) -> Vec<u8> {
        let mut out = vec![0u8; 0x200];
        out[..2].copy_from_slice(b"MZ");
        let at = 0x80usize;
        out[0x3c..0x40].copy_from_slice(&(at as u32).to_le_bytes());
        out[at..at + 4].copy_from_slice(b"PE\0\0");
        out[at + 4..at + 6].copy_from_slice(&0x8664u16.to_le_bytes());
        out[at + 6..at + 8].copy_from_slice(&sections.to_le_bytes());
        let optional = at + 24;
        out[optional + 16..optional + 20].copy_from_slice(&entry.to_le_bytes());
        out[optional + 56..optional + 60].copy_from_slice(&size_of_image.to_le_bytes());
        out
    }

    #[test]
    fn a_mapped_pe_is_recognised_and_read() {
        let image = read_image(0x140000000, &pe(15, 0x3bdf058, 0x417b000)).expect("a PE");
        assert_eq!(image.address, 0x140000000);
        assert_eq!(image.machine, 0x8664);
        assert_eq!(image.sections, 15);
        assert_eq!(image.entry, 0x3bdf058);
        assert_eq!(image.size_of_image, 0x417b000);
    }

    #[test]
    fn anything_that_is_not_a_pe_is_skipped() {
        assert!(read_image(0, b"not a binary at all").is_none());
        assert!(read_image(0, b"MZ").is_none());
        let mut truncated = pe(1, 0, 0);
        truncated.truncate(0x84);
        assert!(read_image(0, &truncated).is_none());
    }

    #[test]
    fn a_pe_header_pointing_past_the_buffer_does_not_panic() {
        let mut broken = pe(1, 0, 0);
        broken[0x3c..0x40].copy_from_slice(&0xffff_0000u32.to_le_bytes());
        assert!(read_image(0, &broken).is_none());
    }
}

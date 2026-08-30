use anyhow::{bail, Result};
use clap::Args;
use iced_x86::{Decoder, DecoderOptions, Formatter, Instruction, IntelFormatter};
use std::io::Write;
use std::path::PathBuf;

#[derive(Args)]
pub struct DisasmArgs {
    #[arg(help = "Raw memory region, named <hex address>.<prot>.bin")]
    pub file: PathBuf,
    #[arg(long, help = "Virtual address of the first byte, if the name does not say")]
    pub base: Option<String>,
    #[arg(long, short, help = "Write here instead of standard output")]
    pub out: Option<PathBuf>,
    #[arg(long, help = "Stop after this many instructions")]
    pub limit: Option<usize>,
    #[arg(long, help = "Skip runs of zero or filler bytes")]
    pub skip_filler: bool,
}

fn base_from_name(path: &std::path::Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let head = name.split('.').next()?;
    u64::from_str_radix(head, 16).ok()
}

pub fn run(args: DisasmArgs) -> Result<()> {
    let bytes = std::fs::read(&args.file)?;
    let base = match &args.base {
        Some(text) => u64::from_str_radix(text.trim_start_matches("0x"), 16)?,
        None => match base_from_name(&args.file) {
            Some(base) => base,
            None => bail!("cannot tell the load address from the file name, pass --base"),
        },
    };

    let mut decoder = Decoder::with_ip(64, &bytes, base, DecoderOptions::NONE);
    let mut formatter = IntelFormatter::new();
    formatter.options_mut().set_digit_separator("`");
    formatter.options_mut().set_first_operand_char_index(10);

    let sink: Box<dyn Write> = match &args.out {
        Some(path) => Box::new(std::io::BufWriter::with_capacity(
            1 << 20,
            std::fs::File::create(path)?,
        )),
        None => Box::new(std::io::BufWriter::new(std::io::stdout())),
    };
    let mut sink = sink;

    let mut instruction = Instruction::default();
    let mut text = String::new();
    let mut written = 0usize;
    let mut filler = 0usize;
    while decoder.can_decode() {
        decoder.decode_out(&mut instruction);
        if args.skip_filler && instruction.is_invalid() {
            filler += 1;
            continue;
        }
        text.clear();
        formatter.format(&instruction, &mut text);
        let start = (instruction.ip() - base) as usize;
        let end = start + instruction.len();
        let raw: String = bytes[start..end]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        writeln!(sink, "{:016x}  {raw:<20}  {text}", instruction.ip())?;
        written += 1;
        if args.limit.map(|limit| written >= limit).unwrap_or(false) {
            break;
        }
    }
    sink.flush()?;
    if args.out.is_some() {
        println!("{written} instructions, {filler} filler bytes skipped");
    }
    Ok(())
}

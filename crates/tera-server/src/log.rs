use std::time::Instant;

pub struct Log {
    started: Instant,
    hex: bool,
}

impl Log {
    pub fn new(hex: bool) -> Self {
        Self {
            started: Instant::now(),
            hex,
        }
    }

    pub fn uptime_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn stamp(&self) -> String {
        format!("{:8.3}", self.started.elapsed().as_secs_f64())
    }

    pub fn line(&self, text: impl AsRef<str>) {
        println!("[{}] {}", self.stamp(), text.as_ref());
    }

    pub fn packet(&self, direction: &str, name: &str, opcode: u16, body: &[u8]) {
        println!(
            "[{}] {direction} {name} ({opcode}) {} bytes",
            self.stamp(),
            body.len()
        );
        if self.hex {
            for (index, chunk) in body.chunks(16).enumerate().take(16) {
                let hex: Vec<String> = chunk.iter().map(|byte| format!("{byte:02x}")).collect();
                let text: String = chunk
                    .iter()
                    .map(|byte| {
                        if byte.is_ascii_graphic() {
                            *byte as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                println!("           {:04x}  {:<47}  {}", index * 16, hex.join(" "), text);
            }
        }
    }
}

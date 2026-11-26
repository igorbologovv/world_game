use std::time::Instant;

#[derive(Default)]
pub struct FrameCounter {
    last: Option<Instant>,
    sample_start: Option<Instant>,
    frames: u32,
    fps: Option<f64>,
    milliseconds: Option<f64>,
}

impl FrameCounter {
    pub fn presented(&mut self, now: Instant) {
        if let Some(last) = self.last.replace(now) {
            let interval = (now - last).as_secs_f64();
            if interval > 0.0 {
                if self.fps.is_none() {
                    self.milliseconds = Some(interval * 1000.0);
                    self.fps = Some(1.0 / interval);
                }
            }
        }
        if self.sample_start.is_none() {self.sample_start=Some(now);return;}
        self.frames += 1;
        let start=self.sample_start.as_mut().unwrap();
        let elapsed=(now-*start).as_secs_f64();
        if elapsed>=0.5 {
            let fps=self.frames as f64/elapsed;
            self.fps=Some(fps);
            self.milliseconds=Some(1000.0/fps);
            self.frames=0;
            *start=now;
        }
    }

    pub fn glyphs(&self) -> [[u32; 4]; 8] {
        let value = |v: Option<f64>| match v {
            Some(v) => format!("{:>7.1}", v.min(99999.9)),
            None => "   --.-".to_owned(),
        };
        let lines = [
            format!("FPS   {}", value(self.fps)),
            format!("FRAME {} MS", value(self.milliseconds)),
        ];
        let mut glyphs = [[0; 4]; 8];
        for (line, text) in lines.iter().enumerate() {
            for (i, ch) in text.bytes().take(16).enumerate() {
                glyphs[line * 4 + i / 4][i % 4] = glyph(ch);
            }
        }
        glyphs
    }
}

// Six rows of four pixels, starting at the most significant nibble.
fn glyph(ch: u8) -> u32 {
    match ch {
        b'0' => 0b0110_1001_1001_1001_1001_0110,
        b'1' => 0b0010_0110_0010_0010_0010_0111,
        b'2' => 0b0110_1001_0001_0010_0100_1111,
        b'3' => 0b1110_0001_0110_0001_1001_0110,
        b'4' => 0b0010_0110_1010_1111_0010_0010,
        b'5' => 0b1111_1000_1110_0001_1001_0110,
        b'6' => 0b0111_1000_1110_1001_1001_0110,
        b'7' => 0b1111_0001_0010_0100_0100_0100,
        b'8' => 0b0110_1001_0110_1001_1001_0110,
        b'9' => 0b0110_1001_1001_0111_0001_1110,
        b'A' => 0b0110_1001_1001_1111_1001_1001,
        b'E' => 0b1111_1000_1110_1000_1000_1111,
        b'F' => 0b1111_1000_1110_1000_1000_1000,
        b'M' => 0b1001_1111_1111_1001_1001_1001,
        b'P' => 0b1110_1001_1001_1110_1000_1000,
        b'R' => 0b1110_1001_1001_1110_1010_1001,
        b'S' => 0b0111_1000_0110_0001_0001_1110,
        b'.' => 0b0000_0000_0000_0000_0000_0010,
        b'-' => 0b0000_0000_0000_0110_0000_0000,
        _ => 0,
    }
}

//! Écriture minimale de fichiers WAV PCM 16 bits.

use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

pub struct WavWriter {
    out: BufWriter<File>,
    channels: u16,
    frames: u32,
}

impl WavWriter {
    pub fn create(path: &Path, sample_rate: u32, channels: u16) -> io::Result<Self> {
        let mut out = BufWriter::new(File::create(path)?);
        let block_align = channels * 2;
        out.write_all(b"RIFF")?;
        out.write_all(&0u32.to_le_bytes())?; // taille, complétée à la fin
        out.write_all(b"WAVEfmt ")?;
        out.write_all(&16u32.to_le_bytes())?;
        out.write_all(&1u16.to_le_bytes())?; // PCM
        out.write_all(&channels.to_le_bytes())?;
        out.write_all(&sample_rate.to_le_bytes())?;
        out.write_all(&(sample_rate * block_align as u32).to_le_bytes())?;
        out.write_all(&block_align.to_le_bytes())?;
        out.write_all(&16u16.to_le_bytes())?;
        out.write_all(b"data")?;
        out.write_all(&0u32.to_le_bytes())?;
        Ok(Self { out, channels, frames: 0 })
    }

    /// Écrit des trames entrelacées (`samples.len()` multiple de `channels`).
    pub fn write_interleaved(&mut self, samples: &[f32]) -> io::Result<()> {
        for &s in samples {
            let v = (s * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
            self.out.write_all(&v.to_le_bytes())?;
        }
        self.frames += (samples.len() / self.channels as usize) as u32;
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<()> {
        let data_len = self.frames * self.channels as u32 * 2;
        self.out.flush()?;
        let f = self.out.get_mut();
        f.seek(SeekFrom::Start(4))?;
        f.write_all(&(36 + data_len).to_le_bytes())?;
        f.seek(SeekFrom::Start(40))?;
        f.write_all(&data_len.to_le_bytes())?;
        f.flush()
    }
}

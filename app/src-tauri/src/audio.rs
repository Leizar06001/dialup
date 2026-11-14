//! Haut-parleur : sortie audio multiplateforme via cpal.
//!
//! Le moteur pousse des échantillons 8 kHz dans un tampon partagé ; le
//! callback audio les rééchantillonne (interpolation linéaire) vers la
//! fréquence du périphérique. Sans périphérique audio, tout continue à
//! fonctionner, simplement sans son.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use v34_core::SAMPLE_RATE;

/// Tampon partagé moteur → carte son.
#[derive(Clone)]
pub struct Speaker {
    buf: Arc<Mutex<VecDeque<f32>>>,
    device_name: Arc<Mutex<Option<String>>>,
    /// Fréquence de la source (8000 Hz × échelle de temps), en bits d'un f64
    rate: Arc<std::sync::atomic::AtomicU64>,
}

impl Default for Speaker {
    fn default() -> Self {
        Self {
            buf: Default::default(),
            device_name: Default::default(),
            rate: Arc::new(std::sync::atomic::AtomicU64::new((SAMPLE_RATE as f64).to_bits())),
        }
    }
}

/// Latence max tolérée avant de jeter des échantillons (horloges carte son / système différentes).
const MAX_BUFFERED: usize = SAMPLE_RATE as usize * 3 / 10; // 300 ms
const TARGET_BUFFERED: usize = SAMPLE_RATE as usize / 10; // 100 ms

impl Speaker {
    /// Démarre le thread audio. Renvoie toujours un `Speaker` utilisable.
    pub fn start() -> Self {
        let sp = Speaker::default();
        let sp2 = sp.clone();
        std::thread::Builder::new()
            .name("v34-audio".into())
            .spawn(move || {
                // Le flux cpal n'est pas `Send` sur toutes les plateformes :
                // il vit et meurt dans ce thread.
                match open_stream(&sp2) {
                    Ok(stream) => {
                        if let Err(e) = stream.play() {
                            eprintln!("[audio] lecture impossible : {e}");
                            return;
                        }
                        loop {
                            std::thread::park();
                        }
                    }
                    Err(e) => eprintln!("[audio] pas de sortie audio ({e}) — l'appli continue sans son"),
                }
            })
            .ok();
        sp
    }

    /// Fréquence de la source : 8000 Hz en temps réel, 800 Hz au ralenti ×0,1
    /// (le son devient 10 fois plus grave, comme un disque ralenti).
    pub fn set_rate(&self, hz: f64) {
        self.rate.store(hz.to_bits(), std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut b) = self.buf.lock() {
            b.clear();
        }
    }

    fn rate(&self) -> f64 {
        f64::from_bits(self.rate.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Ajoute des échantillons (déjà mis à l'échelle du volume).
    pub fn push(&self, samples: &[f32]) {
        let k = self.rate() / SAMPLE_RATE as f64;
        let (max, target) = ((MAX_BUFFERED as f64 * k) as usize, (TARGET_BUFFERED as f64 * k) as usize);
        if let Ok(mut b) = self.buf.lock() {
            b.extend(samples.iter().copied());
            if b.len() > max.max(64) {
                let excess = b.len() - target.max(32);
                b.drain(..excess);
            }
        }
    }

    pub fn device_name(&self) -> Option<String> {
        self.device_name.lock().ok().and_then(|n| n.clone())
    }
}

fn open_stream(sp: &Speaker) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("aucun périphérique de sortie")?;
    let name = device.name().unwrap_or_else(|_| "sortie audio".into());
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    if let Ok(mut n) = sp.device_name.lock() {
        *n = Some(format!("{name} ({} Hz)", config.sample_rate.0));
    }
    match format {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, sp.buf.clone(), sp.rate.clone()),
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, sp.buf.clone(), sp.rate.clone()),
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, sp.buf.clone(), sp.rate.clone()),
        cpal::SampleFormat::I32 => build::<i32>(&device, &config, sp.buf.clone(), sp.rate.clone()),
        other => Err(format!("format d'échantillon non géré : {other:?}")),
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buf: Arc<Mutex<VecDeque<f32>>>,
    rate: Arc<std::sync::atomic::AtomicU64>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let dev_rate = config.sample_rate.0 as f64;
    let (mut prev, mut cur, mut frac) = (0.0f32, 0.0f32, 0.0f64);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let ratio = f64::from_bits(rate.load(std::sync::atomic::Ordering::Relaxed)) / dev_rate;
                let mut b = match buf.lock() {
                    Ok(b) => b,
                    Err(_) => return,
                };
                for frame in data.chunks_mut(channels) {
                    frac += ratio;
                    while frac >= 1.0 {
                        prev = cur;
                        cur = b.pop_front().unwrap_or(0.0);
                        frac -= 1.0;
                    }
                    let v = (prev + (cur - prev) * frac as f32).clamp(-1.0, 1.0);
                    let s = T::from_sample(v);
                    for out in frame.iter_mut() {
                        *out = s;
                    }
                }
            },
            |e| eprintln!("[audio] erreur de flux : {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

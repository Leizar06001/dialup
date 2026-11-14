//! Banc d'essai de « V.Hallucination » (sans réseau) :
//!     cargo run --release -p v34-server --example hallucine -- DOSSIER image.jpg… [formes=60 candidats=200 …]
//! Pour chaque image, écrit dans DOSSIER le SVG halluciné et le GIF du mode
//! normal, et affiche poids, nombre de formes, écart et temps de calcul.
//! Réglages : formes, octets, travail, candidats, age, mutations, alpha, flou.
use std::time::Instant;

use v34_server::web1998::hallu::{self, HalluOptions};
use v34_server::web1998::images::{self, GifOptions};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().expect("dossier de sortie"));
    let mut opts = HalluOptions::default();
    let mut files = Vec::new();
    for a in args {
        let Some((k, v)) = a.split_once('=') else {
            files.push(a);
            continue;
        };
        let n: f64 = v.parse().expect("valeur numérique");
        match k {
            "formes" => opts.max_shapes = n as usize,
            "octets" => opts.max_bytes = n as usize,
            "travail" => opts.work_size = n as u32,
            "candidats" => opts.candidates = n as usize,
            "age" => opts.max_age = n as usize,
            "mutations" => opts.max_mutations = n as usize,
            "alpha" => opts.alpha = n as f32,
            "flou" => opts.blur = n as f32,
            _ => panic!("réglage inconnu : {k}"),
        }
    }
    std::fs::create_dir_all(&dir).expect("dossier de sortie");
    for f in files {
        let data = std::fs::read(&f).expect("image lisible");
        let stem = std::path::Path::new(&f).file_stem().unwrap().to_string_lossy().into_owned();
        let t0 = Instant::now();
        let gif = images::to_gif(&data, &GifOptions::default()).expect("GIF");
        let t_gif = t0.elapsed();
        let t0 = Instant::now();
        let img = image::load_from_memory(&data).expect("image décodable");
        let t_dec = t0.elapsed();
        let t0 = Instant::now();
        let (d, dw, dh) = hallu::hallucinate(&img, &opts, hallu::seed(&f));
        let svg = d.svg(dw, dh, &opts);
        let t_hal = t0.elapsed();
        std::fs::write(dir.join(format!("{stem}.svg")), &svg).unwrap();
        std::fs::write(dir.join(format!("{stem}.gif")), &gif).unwrap();
        println!(
            "{stem}: {}×{} → {dw}×{dh} | GIF {} o ({:.0} ms) | SVG {} o, {} formes, écart {:.1}, {:.0} ms (+ décodage {:.0} ms) | {:.1}× plus léger",
            img.width(),
            img.height(),
            gif.len(),
            t_gif.as_secs_f64() * 1e3,
            svg.len(),
            d.shapes.len(),
            d.rmse,
            t_hal.as_secs_f64() * 1e3,
            t_dec.as_secs_f64() * 1e3,
            gif.len() as f64 / svg.len() as f64
        );
    }
}

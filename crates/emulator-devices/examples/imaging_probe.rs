//! Offline pixel-law probe. No CPU, device state, completion, or firmware substitution.
use fujimod_emulator_imaging::film::Film;
use fujimod_emulator_image_codecs::jpeg::{Codec, fdct};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::{self, BufRead},
};

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Film {
        words: Vec<(usize, u32)>,
        pixels: Vec<[u16; 3]>,
    },
    Dct {
        blocks: Vec<Vec<i32>>,
    },
    Jpeg {
        pixels: Vec<u8>,
        width: usize,
        height: usize,
        tables: Vec<u8>,
        restart: u16,
    },
}
fn probe(request: Request) -> Result<serde_json::Value, String> {
    match request {
        Request::Film { words, pixels } => {
            let words: BTreeMap<_, _> = words.into_iter().collect();
            let mut film = Film::default();
            film.update(|offset| words.get(&offset).copied().unwrap_or(0))?;
            Ok(serde_json::json!(
                pixels
                    .into_iter()
                    .map(|p| film.develop(p))
                    .collect::<Vec<_>>()
            ))
        }
        Request::Dct { blocks } => {
            let mut result = Vec::new();
            for block in blocks {
                let block = block.try_into().map_err(|_| "DCT needs 64 samples")?;
                result.push(fdct(block).to_vec());
            }
            Ok(serde_json::json!(result))
        }
        Request::Jpeg {
            pixels,
            width,
            height,
            tables,
            restart,
        } => {
            let mut codec = Codec::default();
            codec.tables(&tables)?;
            let mut output = Vec::new();
            codec.encode(
                &pixels,
                width,
                height,
                restart,
                &mut output,
                64 * 1024 * 1024,
            )?;
            Ok(serde_json::json!(output))
        }
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    for line in io::stdin().lock().lines() {
        let request: Request = serde_json::from_str(&line?)?;
        println!("{}", probe(request).map_err(io::Error::other)?);
    }
    Ok(())
}

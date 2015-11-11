#![deny(warnings)]
#![deny(unused_import_braces, unused_qualifications)]

#[macro_use] extern crate clap;
#[macro_use] extern crate lazy_static;
#[macro_use] extern crate log;
extern crate env_logger;
extern crate arrayvec;

#[cfg(feature = "sdl2")]
extern crate sdl2;

#[cfg(feature = "glium")]
#[macro_use]
extern crate glium;

use std::env;
use std::fs::File;
use std::io::Read;

use rom::Rom;
use snes::Snes;

#[macro_use] mod byte_array;
#[macro_use] mod log_util;
#[macro_use] mod savestate;
mod apu;
mod cpu;
mod dma;
mod frontend;
mod input;
mod ppu;
mod rom;
mod snes;

fn main() {
    if env::var_os("RUST_LOG").is_none() {
        env::set_var("RUST_LOG", "sneeze=INFO");
    }
    env_logger::init().unwrap();

    let args = clap_app!(sneeze =>
        (version: option_env!("CARGO_PKG_VERSION").unwrap_or("<unknown version>"))
        (about: "SNES emulator")
        (@arg ROM_PATH: +required "The ROM file to execute")
        (@arg renderer: -R --renderer +takes_value "The renderer to use")
    ).get_matches();

    let renderer_name = args.value_of("renderer").unwrap_or(&*frontend::DEFAULT_RENDERER);

    let renderer = match frontend::RENDERER_MAP.get(renderer_name) {
        None => {
            println!("error: unknown renderer: {}", renderer_name);
            return
        }
        Some(&None) => {
            println!("error: renderer '{}' not compiled in", renderer_name);
            println!("(compile with `cargo build --features {}` to enable)", renderer_name);
            // NOTE: Make sure that renderer name always matches feature name!
            return
        }
        Some(&Some(renderer_fn)) => {
            renderer_fn()
        }
    };
    info!("using {} renderer", renderer_name);

    let filename = args.value_of("ROM_PATH").unwrap();
    let mut file = File::open(&filename).unwrap();
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).unwrap();

    let rom = Rom::from_bytes(&buf).unwrap();

    let mut snes = Snes::new(rom, renderer);
    snes.run();
}

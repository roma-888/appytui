#![allow(dead_code)]
mod config;
mod music;

fn main() {
    let cfg = config::Config::load(&config::Config::default_path()).expect("config");
    println!("{cfg:#?}");
}

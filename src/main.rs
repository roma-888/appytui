#![allow(dead_code)]
mod app;
mod config;
mod music;

fn main() {
    let cfg = config::Config::load(&config::Config::default_path()).expect("config");
    println!("{cfg:#?}");
}

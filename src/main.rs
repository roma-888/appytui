#![allow(dead_code)]
mod config;

fn main() {
    let cfg = config::Config::load(&config::Config::default_path()).expect("config");
    println!("{cfg:#?}");
}

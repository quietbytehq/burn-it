// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "--helper" {
        std::process::exit(burn_it_lib::run_helper(&args[2]));
    }
    burn_it_lib::run()
}

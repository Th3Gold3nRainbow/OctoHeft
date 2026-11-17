// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if octoheft_engine_lib::engine::native_host::should_run_as_native_host(&args) {
        let exit_code = octoheft_engine_lib::engine::native_host::run_native_host_stdio();
        std::process::exit(exit_code);
    }

    octoheft_engine_lib::run();
}


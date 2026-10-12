// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    kormilant_lib::adopt_login_path();
    kormilant_lib::run()
}

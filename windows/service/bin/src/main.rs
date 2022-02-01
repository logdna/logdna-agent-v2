#![no_main]

use std::os::raw::{c_char, c_int, c_void};
use std::sync::mpsc::Receiver;
use std::thread;

#[macro_use]
extern crate winservice;

#[allow(non_snake_case)]
#[no_mangle]
pub extern "system" fn WinMain(hInstance : *const c_void, hPrevInstance : *const c_void,
    lpCmdLine : *const c_char, nCmdShow : c_int) -> c_int
{
    Service!("LogDNA Agent", service_main)
}

fn service_main(args : Vec<String>, end : Receiver<()>) -> u32 {
    print!("begin");
    loop {
        thread::sleep_ms(200);
        if let Ok(_) = end.try_recv() { break; }
    }
    print!("end");
 0
}

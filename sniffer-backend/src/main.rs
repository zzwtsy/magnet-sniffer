use sniffer_common::logger;

fn main() {
    let _guard = logger::init("magnet-sniffer-server", None, None, None);
    println!("Hello, world!");
}

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).take(2).collect();
    let exit = match arguments.as_slice() {
        [argument] if argument == "--stdio" => peripheral_hub_lib::audio_worker::run_stdio(),
        [argument] if argument == "--help" => {
            println!("peripheral-hub-audio-worker --stdio (private, bounded JSON protocol)");
            0
        }
        _ => {
            eprintln!("Argomenti non validi. Usare --help.");
            2
        }
    };
    std::process::exit(exit);
}

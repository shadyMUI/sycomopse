#[tokio::main]
async fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let disable_all = match arguments.as_slice() {
        [] => false,
        [argument] if argument == "--disable-all" => true,
        [argument] if argument == "--help" => {
            println!("peripheral-hub-remapping-agent [--disable-all]");
            return;
        }
        _ => {
            eprintln!("Argomenti non validi. Usare --help.");
            std::process::exit(2);
        }
    };
    if let Err(error) = peripheral_hub_lib::remapping::run_agent(disable_all).await {
        eprintln!("{}", error.message);
        std::process::exit(1);
    }
}

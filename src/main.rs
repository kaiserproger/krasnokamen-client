fn main() {
    if let Err(error) = krasnokamen_client::cli(std::env::args().skip(1)) {
        eprintln!("krasnokamen-client: {error}");
        std::process::exit(1);
    }
}

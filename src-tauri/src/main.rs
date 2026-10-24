// Verhindert das zusätzliche Konsolenfenster unter Windows im Release-Build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Headless-Modus für Evaluierung/CI: `private-wispr --selftest [audio.wav]`
    if let Some(pos) = args.iter().position(|a| a == "--selftest") {
        let wav = args.get(pos + 1).map(std::path::PathBuf::from);
        let code = private_wispr_lib::selftest::run(wav);
        std::process::exit(code);
    }
    private_wispr_lib::run()
}

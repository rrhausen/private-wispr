// Verhindert das zusätzliche Konsolenfenster unter Windows im Release-Build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Isolierter Engine-Kindprozess (specs 4.1): App startet sich selbst mit
    // diesem Schalter; Kommunikation über stdin/stdout (engine_worker.rs).
    if args.iter().any(|a| a == "--engine-worker") {
        let code = private_wispr_lib::engine_worker::run_worker();
        std::process::exit(code);
    }
    // Headless-Modus für Evaluierung/CI: `private-wispr --selftest [audio.wav]`
    if let Some(pos) = args.iter().position(|a| a == "--selftest") {
        let wav = args.get(pos + 1).map(std::path::PathBuf::from);
        let code = private_wispr_lib::selftest::run(wav);
        std::process::exit(code);
    }
    private_wispr_lib::run()
}

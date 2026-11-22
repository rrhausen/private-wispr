fn main() {
    // Silero-VAD-Modell fürs Bundling nach target/release spiegeln: Die
    // Resource (tauri.conf.json) muss bei JEDEM Build existieren — die
    // Engine-DLLs erzeugt der Sys-Crate-Build dort, das GGUF ist keine
    // Build-Ausgabe. Quelle ist die einmalige Dev-Bereitstellung unter
    // %LOCALAPPDATA%\PrivateWispr\models (scripts/download-vad-model.ps1);
    // `tauri build` lädt es notfalls selbst (beforeBundleCommand). Fehlt es
    // überall, bricht der Build mit klarer Meldung — kein stilles Bundle
    // ohne VAD.
    copy_vad_model();
    tauri_build::build()
}

const VAD_FILE: &str = "silero-vad-v6.2-F32.gguf";

fn copy_vad_model() {
    let Some(local) = dirs_next_data_local() else {
        return;
    };
    let src = local.join("PrivateWispr").join("models").join(VAD_FILE);
    if !src.exists() {
        return; // beforeBundleCommand lädt beim Bundling; sonst klare Fehlermeldung
    }
    let dest = std::path::Path::new("target").join("release").join(VAD_FILE);
    if dest.exists() {
        return;
    }
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::copy(&src, &dest) {
        Ok(_) => println!("cargo:rerun-if-changed={}", src.display()),
        Err(e) => println!("cargo:warning=VAD-Modell nicht nach target/release kopierbar ({e})"),
    }
}

/// Minimal-Ersatz für das dirs-Crate im Build-Skript (keine Extra-Abhängigkeit).
fn dirs_next_data_local() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)
}

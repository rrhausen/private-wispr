//! E2E: Der echte Worker-Prozess (die exe selbst mit `--engine-worker`)
//! lädt das Modell und transkribiert über das stdio-Protokoll — der Pfad,
//! den die App im Betrieb nutzt. Wird übersprungen, wenn kein Modell
//! heruntergeladen ist (z. B. frischer Rechner/CI).

use private_wispr_lib::stt::SttEngine;

#[test]
fn worker_prozess_transkribiert_ueber_protokoll() {
    if !private_wispr_lib::stt::default_model_path().exists() {
        eprintln!("skip: kein Modell unter {}", private_wispr_lib::stt::default_model_path().display());
        return;
    }
    // Der Supervisor startet normalerweise die eigene exe — im Test ist das
    // die Testbinary, daher die App-Binary explizit vorgeben.
    std::env::set_var("PW_ENGINE_WORKER_EXE", env!("CARGO_BIN_EXE_private-wispr"));
    let mut engine = private_wispr_lib::engine_worker::WorkerEngine::start_default()
        .expect("Worker-Start mit Modell-Load");

    let info = engine.info();
    assert!(info.loaded, "Worker meldet geladenes Modell: {info:?}");
    assert!(!info.backend.is_empty(), "Backend bekannt: {info:?}");

    // 1 s Stille transkribieren — Ergebnis darf leer sein, der Weg zählt
    // (Protokoll-Block, Antwort, Zeitmessung).
    let samples = vec![0.0f32; 16_000];
    let result = engine
        .transcribe(&samples, &[])
        .expect("Transkription über das Worker-Protokoll");
    assert!(result.transcription_ms > 0);
}

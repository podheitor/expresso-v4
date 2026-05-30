//! expresso-calendar binary — thin shim over the `expresso_calendar` library.
//! All wiring lives in `lib.rs::run`; this exists so the same modules can be
//! reached as a library (e.g. the `fuzzing` feature's `fuzz_entry`).

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    expresso_calendar::run().await
}

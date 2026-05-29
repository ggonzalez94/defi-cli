//! Thin `defi` binary: tokio runtime → `defi_app::run`.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let code = defi_app::run().await;
    ExitCode::from(defi_cli::process_exit_code(code))
}

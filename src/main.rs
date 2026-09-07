use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    tuido::diagnostics::install();
    let result = tuido::cli::run();
    if let Err(error) = &result {
        tuido::diagnostics::record_error("process exited with an error", error.as_ref());
    }
    result
}

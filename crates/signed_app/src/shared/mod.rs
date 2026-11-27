pub mod commit_diff;
pub mod labels;
pub mod widgets;

/// Logs a discarded `Result` with its source location instead of dropping it silently.
#[track_caller]
pub fn log_dropped<T>(result: Result<T, anyhow::Error>) {
    if let Err(error) = result {
        let location = std::panic::Location::caller();
        log::warn!(
            "{}:{}: dropped result: {error}",
            location.file(),
            location.line()
        );
    }
}

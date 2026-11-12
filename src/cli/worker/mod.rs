//! `quantforge worker` — a job's worker process, hidden from `--help` and
//! never run by hand: `serve run` starts it with a cleared environment and
//! hands it one job over its standard input. It runs before any async
//! runtime or other thread starts, so that the limits and sandbox it puts on
//! itself cover the whole process.

use anyhow::Result;

pub(crate) fn run() -> Result<()> {
    quantforge::server::run_worker(std::io::stdin(), std::io::stdout())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use quantforge::server::WORKER_COMMAND;

    #[test]
    fn the_worker_command_is_hidden_has_no_flags_and_is_named_as_the_server_starts_it() {
        let mut root = crate::cli::command();
        let worker = root
            .find_subcommand(WORKER_COMMAND)
            .expect("the worker command");
        assert!(worker.is_hide_set());
        assert_eq!(worker.get_arguments().count(), 0);
        let help = root.render_long_help().to_string();
        assert!(!help.contains(WORKER_COMMAND), "{help}");
    }
}

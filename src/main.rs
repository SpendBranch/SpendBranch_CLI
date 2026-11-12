#![forbid(unsafe_code)]

use std::io::IsTerminal;

fn main() {
    // `args_os`, not `args`: a non-UTF-8 argument must be a usage error, never a panic.
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let mut stdout = std::io::stdout();
    let interactive = stdout.is_terminal();
    // The hook reads the agent's stdin itself, with a timeout: it must not be locked here.
    let mut keyboard: Box<dyn std::io::BufRead> = if args.first().is_some_and(|a| a == "hook") {
        Box::new(std::io::empty())
    } else {
        Box::new(std::io::stdin().lock())
    };
    let mut io = spendbranch::cli::Io {
        out: &mut stdout,
        input: &mut keyboard,
        sleep: &std::thread::sleep,
        open_url: &spendbranch::infra::browser::open,
        interactive,
        system: &spendbranch::infra::system::Real,
    };
    let code = spendbranch::cli::run_with(&args, &|k| std::env::var(k).ok(), &mut io);
    std::process::exit(code);
}

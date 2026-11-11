#![forbid(unsafe_code)]

fn main() {
    // `args_os`, not `args`: a non-UTF-8 argument must be a usage error, never a panic.
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let code = spendbranch::cli::run(&args, &|k| std::env::var(k).ok(), &mut std::io::stdout());
    std::process::exit(code);
}

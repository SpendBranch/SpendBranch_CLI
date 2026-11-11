fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = spendbranch::cli::run(&args, &|k| std::env::var(k).ok(), &mut std::io::stdout());
    std::process::exit(code);
}

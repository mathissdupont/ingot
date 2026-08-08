fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    if let Some(arg) = args.next() {
        if arg == "--version" || arg == "-V" {
            println!("ingot-lsp {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        anyhow::bail!("unknown argument `{arg}`");
    }

    ingot_lsp::run_stdio()
}

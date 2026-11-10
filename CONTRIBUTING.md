# Contributing

Thanks for contributing to YopesCLI. These instructions cover building and running the project from a checkout.

## Install on PATH

From this directory:

```sh
cargo install --path .
yopes
```

Cargo installs the executable as `yopes` (`yopes.exe` on Windows) in `$HOME/.cargo/bin`, normally `%USERPROFILE%\.cargo\bin` on Windows. Keep that directory on PATH. Re-run `cargo install --path . --force` after updates.

## Run without installing

```sh
cargo run --release
```

### OpenAI-compatible

```sh
export OPENAI_API_KEY="your-key"
cargo run --release -- --provider openai --model gpt-4o-mini
```

PowerShell:

```powershell
$env:OPENAI_API_KEY = "your-key"
cargo run --release -- --provider openai --model gpt-4o-mini
```

For another compatible endpoint:

```sh
cargo run --release -- \
  --provider openai \
  --base-url https://example.com/v1 \
  --model your-model
```

### Ollama

Start Ollama, pull a model, then run:

```sh
ollama pull llama3.2
cargo run --release -- --provider ollama --model llama3.2
```

`--base-url` defaults to `http://localhost:11434` for Ollama and `https://api.openai.com/v1` for OpenAI-compatible APIs.

## Tests

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

GitHub Actions uses the **On-the-line** workflow as a compile-only gate on Linux and Windows before the **Production** workflow publishes release builds.

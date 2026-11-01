# Contributing

Bug reports, fixes and features are welcome. Before a large change, open an
issue so we can agree on the approach first.

## Building and testing

[README.md](README.md#build-and-run) covers building the web app, the
desktop apps and the Android app, and [ARCHITECTURE.md](ARCHITECTURE.md)
explains how the code fits together. A pull request runs the checks below.
They need the PDFium library and the built UI:

```sh
scripts/fetch-pdfium.sh
export BOWERBIRD_PDFIUM_DIR="$PWD/vendor/pdfium/lib"
npm --prefix ui ci && npm --prefix ui run build
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
npm --prefix ui run check
```

The end-to-end and live tests ([README.md](README.md#tests)) ask a real
model, so they need `DEEPSEEK_API_KEY`, and CI runs them daily rather than on
pull requests. Run them when you change answering, citations or the UI.

## Guidelines

- One change per pull request, with tests where the code around it has them.
- Bowerbird works with every model provider genai supports, so prompts and
  features stay provider-neutral.
- A new dependency needs a license that `cargo deny` accepts (`deny.toml`).
- Contributions are licensed under the project's [MIT license](LICENSE).

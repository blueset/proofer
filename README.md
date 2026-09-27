# Proofer

Proofer is a Rust command-line tool for generating font proof PDFs from YAML proof documents. It supports sample text, waterfalls, glyph grids, and other page layouts. The repository is a Cargo workspace containing the CLI and its font, layout, and rendering libraries.

## Build

Install a current stable [Rust toolchain](https://rustup.rs/) (including Cargo), then run from the repository root:

```sh
cargo build --workspace --locked
```

The executable is `target/debug/proofer` (or `target\debug\proofer.exe` on Windows). The build uses the checked-in `Cargo.lock` and the patched `vendor/subsetter` crate.

## Generate a proof

Generate a starter configuration:

```sh
cargo run --locked -p proofer-cli -- example > sample.proof.yaml
```

The starter configuration refers to `./MyFont-Regular.otf`. Change its `fonts[0].path` to a font on your machine (for example, `assets/fonts/InterVariable.ttf` if the YAML file is in the repository root). Font and referenced content paths are resolved relative to the YAML file.

Then generate the PDF:

```sh
cargo run --locked -p proofer-cli -- generate sample.proof.yaml
```

This writes `sample.proof.pdf` by default. Set a different output path with `--output`:

```sh
cargo run --locked -p proofer-cli -- generate sample.proof.yaml --output proof.pdf
```

To inspect the JSON Schema for the proof document format, run `cargo run --locked -p proofer-cli -- schema`.
For all configuration fields, examples, and current limitations, see the [proof configuration guide](CONFIG.md).

## Tests

Run the workspace tests with `cargo test --workspace --locked`. The integration tests currently load `C:\Windows\Fonts\arial.ttf`, so the full test suite requires that font at that path on Windows. The GitHub Actions workflow checks the workspace build without running these Windows-specific tests.

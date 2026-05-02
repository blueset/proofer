use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process;

#[derive(Parser)]
#[command(name = "proofer")]
#[command(about = "Font proof PDF generator")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate a PDF proof from a .proof.yaml file.
    Generate {
        /// Path to the .proof.yaml configuration file.
        input: PathBuf,
        /// Output PDF file path (default: <input>.pdf).
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Print the JSON Schema for the .proof.yaml format.
    Schema,
    /// Print an example .proof.yaml to stdout.
    Example,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Generate { input, output } => cmd_generate(&input, output.as_deref()),
        Command::Schema => cmd_schema(),
        Command::Example => cmd_example(),
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

fn cmd_generate(input: &std::path::Path, output: Option<&std::path::Path>) -> Result<(), Box<dyn std::error::Error>> {
    // Determine output path
    let output_path = match output {
        Some(p) => p.to_path_buf(),
        None => input.with_extension("pdf"),
    };

    // Load and parse the proof document
    let yaml_content = std::fs::read_to_string(input)
        .map_err(|e| format!("failed to read {}: {e}", input.display()))?;

    let mut doc = proof_model::from_yaml(&yaml_content)
        .map_err(|e| format!("failed to parse {}: {e}", input.display()))?;

    // Resolve relative paths against the input file's directory
    let base_dir = input
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    doc.resolve_paths(base_dir);

    // Validate
    let errors = doc.validate();
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("validation error: {e}");
        }
        return Err(format!("{} validation error(s)", errors.len()).into());
    }

    // Load fonts
    let mut registry = font_registry::FontRegistry::new();
    let mut font_ids = Vec::new();
    for (i, font_ref) in doc.fonts.iter().enumerate() {
        let id = registry
            .load_file(&font_ref.path, font_ref.face_index)
            .map_err(|e| format!("failed to load font {}: {e}", font_ref.path.display()))?;
        font_ids.push(id);

        let meta = registry.metadata(id)?;
        eprintln!(
            "  font[{i}]: {} {} ({})",
            meta.family,
            meta.style,
            font_ref.path.display()
        );
    }

    // Layout
    eprintln!("  laying out {} section(s)...", doc.sections.len());
    let layout_doc = page_layout::layout_document(&doc, &font_ids, &registry)
        .map_err(|e| format!("layout error: {e}"))?;

    eprintln!("  {} page(s) generated", layout_doc.page_count());

    // Render to PDF
    let pdf_bytes = pdf_renderer::render_to_pdf(&layout_doc, &registry)
        .map_err(|e| format!("render error: {e}"))?;

    // Write output
    std::fs::write(&output_path, &pdf_bytes)
        .map_err(|e| format!("failed to write {}: {e}", output_path.display()))?;

    eprintln!(
        "  wrote {} ({} bytes)",
        output_path.display(),
        pdf_bytes.len()
    );

    Ok(())
}

fn cmd_schema() -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", proof_model::json_schema_string());
    Ok(())
}

fn cmd_example() -> Result<(), Box<dyn std::error::Error>> {
    let doc = proof_model::example_document();
    let yaml = proof_model::to_yaml(&doc)?;
    println!("{yaml}");
    Ok(())
}

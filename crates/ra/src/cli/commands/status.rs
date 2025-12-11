//! Implementation of `ra status`.

use std::process::ExitCode;

use ra_config::{ConfigWarning, discover_config_files, format_path_for_display};
use ra_index::{detect_index_status, index_directory};

use crate::cli::{
    context::CommandContext,
    output::{dim, subheader, warning},
};

/// Shows configuration files, trees, index status, and validation warnings.
pub fn run(ctx: &CommandContext) -> ExitCode {
    let cwd = &ctx.cwd;

    let config_files = discover_config_files(cwd);
    if config_files.is_empty() {
        println!("{}", dim("No configuration files found."));
        println!();
        println!(
            "Run {} to create a configuration file.",
            subheader("ra init")
        );
        return ExitCode::SUCCESS;
    }

    println!("{}", subheader("Config files:"));
    for path in &config_files {
        let display_path = format_path_for_display(path, Some(cwd));
        println!("   {display_path}");
    }
    println!();

    let config = &ctx.config;

    println!("{}", subheader("Trees:"));
    if config.trees.is_empty() {
        println!("   {}", dim("(none defined)"));
    } else {
        for tree in &config.trees {
            let scope = if tree.is_global { "global" } else { "local" };
            let base = if tree.is_global {
                None
            } else {
                config.config_root.as_deref()
            };
            let display_path = format_path_for_display(&tree.path, base);
            if tree.path.exists() {
                println!(
                    "   {} {} {}",
                    tree.name,
                    dim(&format!("({scope})")),
                    dim(&format!("-> {display_path}"))
                );
            } else {
                println!(
                    "   {} {} {} {}",
                    tree.name,
                    dim(&format!("({scope})")),
                    dim(&format!("-> {display_path}")),
                    warning("[missing]")
                );
            }
        }
    }
    println!();

    if !config.trees.is_empty() {
        println!("{}", subheader("Patterns:"));
        for tree in &config.trees {
            println!("   {}:", tree.name);
            for pattern in &tree.include {
                println!("      + {pattern}");
            }
            for pattern in &tree.exclude {
                println!("      - {pattern}");
            }
        }
        println!();
    }

    let index_status = detect_index_status(config);
    let index_path = index_directory(config);
    print!("{}\n   {}", subheader("Index:"), index_status.description());
    if let Some(path) = &index_path {
        println!(" {}", dim(&format!("({})", path.display())));
    } else {
        println!();
    }
    println!();

    let warnings = config.validate();
    if warnings.is_empty() {
        println!("No issues found.");
        return ExitCode::SUCCESS;
    }

    println!("{}", subheader(&format!("Warnings ({}):", warnings.len())));
    for w in &warnings {
        println!("   {}", warning(&format_warning(w)));
    }
    println!();

    print_hints(&warnings);

    ExitCode::FAILURE
}

/// Formats a warning with helpful context.
fn format_warning(w: &ConfigWarning) -> String {
    w.to_string()
}

/// Prints hints for resolving common warnings.
fn print_hints(warnings: &[ConfigWarning]) {
    for w in warnings {
        match w {
            ConfigWarning::NoTreesDefined => {
                println!("{}", dim("Hint: add [tree.NAME] sections to .ra.toml"));
            }
            ConfigWarning::IncludePatternMatchesNothing { .. } => {
                println!("{}", dim("Hint: check include patterns or tree paths"));
            }
            _ => {}
        }
    }
}

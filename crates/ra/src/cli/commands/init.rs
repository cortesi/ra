//! Implementation of `ra init`.

use std::{
    fs,
    io::{self, Write},
    path::Path,
    process::ExitCode,
};

use ra_config::{CONFIG_FILENAME, global_config_path, global_template, local_template};
use ra_highlight::{Highlighter, indent_content, subheader};

use crate::cli::{args::InitCommand, context::CommandContext};

/// Initializes a `.ra.toml` configuration file.
pub fn run(ctx: &CommandContext, cmd: &InitCommand) -> ExitCode {
    let cwd = &ctx.cwd;

    let is_home_dir = global_config_path()
        .and_then(|p| p.parent().map(|h| h == cwd))
        .unwrap_or(false);

    let use_global = cmd.global || is_home_dir;

    let config_path = if use_global {
        match global_config_path() {
            Some(path) => path,
            None => {
                eprintln!("error: could not determine home directory");
                return ExitCode::FAILURE;
            }
        }
    } else {
        cwd.join(CONFIG_FILENAME)
    };

    if config_path.exists() && !cmd.force {
        eprintln!(
            "error: configuration file already exists: {}",
            config_path.display()
        );
        eprintln!("use --force to overwrite");
        return ExitCode::FAILURE;
    }

    let template = if use_global {
        global_template()
    } else {
        local_template()
    };

    if let Err(e) = fs::write(&config_path, &template) {
        eprintln!("error: failed to write {}: {e}", config_path.display());
        return ExitCode::FAILURE;
    }

    println!("Created {}", config_path.display());

    let highlighter = Highlighter::new();
    println!();
    println!("{}", subheader("Configuration written:"));
    let highlighted = highlighter.highlight(&template, "toml");
    println!("{}", indent_content(&highlighted));

    if !use_global && let Err(e) = update_gitignore(&config_path) {
        eprintln!("warning: could not update .gitignore: {e}");
    }

    ExitCode::SUCCESS
}

/// Adds `.ra/` to `.gitignore` if it exists and doesn't already contain it.
fn update_gitignore(config_path: &Path) -> io::Result<()> {
    let Some(parent) = config_path.parent() else {
        return Ok(());
    };

    let gitignore_path = parent.join(".gitignore");

    if !gitignore_path.exists() {
        return Ok(());
    }

    let contents = fs::read_to_string(&gitignore_path)?;

    let ra_pattern = ".ra/";
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed == ra_pattern || trimmed == ".ra" {
            return Ok(());
        }
    }

    let mut file = fs::OpenOptions::new().append(true).open(&gitignore_path)?;

    if !contents.is_empty() && !contents.ends_with('\n') {
        writeln!(file)?;
    }

    writeln!(file, "{ra_pattern}")?;
    println!("Added {ra_pattern} to .gitignore");

    Ok(())
}

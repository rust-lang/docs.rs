use cli_table::{Cell, Style, Table, format::Justify, print_stdout};
use docs_rs_rustwide::{ReleaseBuildResult, StepResult, StepResultExt, TargetBuildResult};
use humantime::format_duration;
use std::time::Duration;

pub(crate) fn print(
    result: &ReleaseBuildResult,
    duration: Duration,
    succeeded: bool,
    strict: bool,
) -> anyhow::Result<()> {
    println!();
    println!("docs.rs build summary");

    let mut rows = Vec::new();
    let mut totals = [Duration::ZERO; 4];
    for target in result.targets() {
        rows.push([
            format!(
                "{}{}",
                target.target,
                if target.is_default { " (default)" } else { "" }
            ),
            step_cell(&target.documentation),
            step_cell(&target.rustdoc_json),
            if target.is_default {
                step_cell(&target.coverage)
            } else {
                "skipped".into()
            },
            format_duration(target.duration()).to_string(),
        ]);
        totals[0] += target.documentation.duration();
        totals[1] += target.rustdoc_json.duration();
        totals[2] += target.coverage.duration();
        totals[3] += target.duration();
    }
    rows.push([
        "Total".to_owned(),
        format_duration(totals[0]).to_string(),
        format_duration(totals[1]).to_string(),
        format_duration(totals[2]).to_string(),
        format_duration(totals[3]).to_string(),
    ]);
    print_table(&rows)?;
    println!("Target totals include retries and work between steps.");
    println!("Full build duration: {}", format_duration(duration));

    match result.statistics.memory_peak_bytes() {
        Some(bytes) => println!(
            "  sandbox peak memory: {:.1} MiB",
            bytes as f64 / (1024.0 * 1024.0)
        ),
        None => println!("  sandbox peak memory: unavailable"),
    }

    println!();
    for target in result.targets() {
        println!("{}:", target.target);
        print_error("HTML", &target.documentation);
        print_error("rustdoc JSON", &target.rustdoc_json);
        print_error("coverage", &target.coverage);
        if let Some(step) = target.regenerate_lockfile() {
            print_error("lockfile regeneration", step);
        }
        if let Ok(output) = target.documentation() {
            println!("  HTML output: {}", output.path().display());
        }
        if let Ok(output) = target.rustdoc_json() {
            println!("  JSON output: {}", output.path().display());
        }
        for path in target.compiler_metrics.iter().flatten() {
            println!("  compiler metrics: {}", path.display());
        }
    }

    if !result.has_docs() {
        println!("  error: the default target produced no library documentation");
    }

    if succeeded {
        println!("docs.rs build succeeded");
    } else if strict && result.has_docs() {
        println!("docs.rs build failed because --strict treats auxiliary failures as fatal");
    } else {
        println!("docs.rs build failed");
    }
    Ok(())
}

fn target_fully_succeeded(target: &TargetBuildResult) -> bool {
    target.documentation_succeeded() && target.rustdoc_json.is_ok() && target.coverage.is_ok()
}

pub(crate) fn build_succeeded(result: &ReleaseBuildResult, strict: bool) -> bool {
    result.has_docs() && (!strict || result.targets().all(target_fully_succeeded))
}

fn step_cell<T>(step: &StepResult<T>) -> String {
    format!(
        "{} {}",
        if step.is_ok() { "ok" } else { "FAILED" },
        format_duration(step.duration())
    )
}

fn print_error<T>(name: &str, step: &StepResult<T>) {
    if let Err(report) = step {
        println!("  {name}: failed: {:#}", report.value);
        if let Some(log) = report.log.as_deref().filter(|log| !log.trim().is_empty()) {
            println!("    captured build log:");
            for line in log.lines() {
                println!("      {line}");
            }
        }
    }
}

fn print_table(rows: &[[String; 5]]) -> std::io::Result<()> {
    let headers = ["Target", "HTML", "rustdoc JSON", "Coverage", "Target total"];
    let table = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            row.iter()
                .enumerate()
                .map(|(column, value)| {
                    value
                        .cell()
                        .justify(if column == 0 {
                            Justify::Left
                        } else {
                            Justify::Right
                        })
                        .bold(index + 1 == rows.len())
                })
                .collect::<Vec<_>>()
        })
        .table()
        .title(headers.iter().map(|title| title.cell().bold(true)));
    print_stdout(table)
}

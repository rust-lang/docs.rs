use cli_table::{Cell, Style, Table, format::Justify, print_stdout};
use docs_rs_rustwide::{ReleaseBuildResult, StepResult, TargetBuildResult};
use std::time::Duration;

pub(crate) fn print(
    result: &ReleaseBuildResult,
    duration: Duration,
    strict: bool,
) -> anyhow::Result<bool> {
    println!();
    println!("docs.rs build summary");

    let mut rows = vec![[
        "Target".to_owned(),
        "HTML".to_owned(),
        "rustdoc JSON".to_owned(),
        "Coverage".to_owned(),
        "Target total".to_owned(),
    ]];
    let mut totals = [Duration::ZERO; 4];
    for target in &result.targets {
        rows.push([
            format!(
                "{}{}",
                target.target,
                if target.is_default { " (default)" } else { "" }
            ),
            step_cell(&target.documentation),
            step_cell(&target.rustdoc_json),
            step_cell(&target.coverage),
            format_duration(target.duration()),
        ]);
        totals[0] += target.documentation.duration;
        totals[1] += target.rustdoc_json.duration;
        totals[2] += target.coverage.duration;
        totals[3] += target.duration();
    }
    rows.push([
        "Total".to_owned(),
        format_duration(totals[0]),
        format_duration(totals[1]),
        format_duration(totals[2]),
        format_duration(totals[3]),
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
    for target in &result.targets {
        println!("{}:", target.target);
        print_error("HTML", &target.documentation);
        print_error("rustdoc JSON", &target.rustdoc_json);
        print_error("coverage", &target.coverage);
        if let Some(path) = &target.documentation.output {
            println!("  HTML output: {}", path.display());
        }
        if let Some(output) = &target.rustdoc_json.output {
            println!("  JSON output: {}", output.path().display());
        }
        for path in &target.compiler_metrics {
            println!("  compiler metrics: {}", path.display());
        }
    }

    let default_succeeded = result.successful() && result.has_docs();
    if !result.has_docs() {
        println!("  error: the default target produced no library documentation");
    }

    let auxiliary_succeeded = result.targets.iter().all(target_fully_succeeded);
    let succeeded = build_succeeded(default_succeeded, auxiliary_succeeded, strict);
    if succeeded {
        println!("docs.rs build succeeded");
    } else if strict && default_succeeded {
        println!("docs.rs build failed because --strict treats auxiliary failures as fatal");
    } else {
        println!("docs.rs build failed");
    }
    Ok(succeeded)
}

fn target_fully_succeeded(target: &TargetBuildResult) -> bool {
    target.successful() && target.rustdoc_json.successful() && target.coverage.successful()
}

fn build_succeeded(default_succeeded: bool, auxiliary_succeeded: bool, strict: bool) -> bool {
    default_succeeded && (!strict || auxiliary_succeeded)
}

fn format_duration(duration: Duration) -> String {
    format!("{:.2}s", duration.as_secs_f64())
}

fn step_cell<T>(step: &StepResult<T>) -> String {
    format!(
        "{} {}",
        if step.successful() { "ok" } else { "FAILED" },
        format_duration(step.duration)
    )
}

fn print_error<T>(name: &str, step: &StepResult<T>) {
    if let Some(error) = &step.error {
        println!("  {name}: failed: {error:#}");
        if !step.log.trim().is_empty() {
            println!("    captured build log:");
            for line in step.log.lines() {
                println!("      {line}");
            }
        }
    }
}

fn print_table(rows: &[[String; 5]]) -> std::io::Result<()> {
    let table = rows[1..]
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
                        .bold(index == rows.len() - 2)
                })
                .collect::<Vec<_>>()
        })
        .table()
        .title(rows[0].iter().map(|title| title.cell().bold(true)));
    print_stdout(table)
}

#[cfg(test)]
mod tests {
    use super::build_succeeded;

    #[test]
    fn default_build_is_always_required() {
        assert!(!build_succeeded(false, true, false));
        assert!(!build_succeeded(false, true, true));
    }

    #[test]
    fn auxiliary_failures_are_only_fatal_in_strict_mode() {
        assert!(build_succeeded(true, false, false));
        assert!(!build_succeeded(true, false, true));
        assert!(build_succeeded(true, true, true));
    }
}

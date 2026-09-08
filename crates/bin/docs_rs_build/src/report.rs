use docs_rs_rustwide::{ReleaseBuildResult, StepResult, TargetBuildResult};

pub(crate) fn print(result: &ReleaseBuildResult, strict: bool) -> bool {
    println!();
    println!("docs.rs build summary");

    for target in &result.targets {
        println!(
            "  {}{}",
            target.target,
            if target.is_default { " (default)" } else { "" }
        );
        print_step("HTML documentation", &target.documentation);
        if let Some(path) = &target.documentation.output {
            println!("      output: {}", path.display());
        }
        print_step("rustdoc JSON", &target.rustdoc_json);
        if let Some(output) = &target.rustdoc_json.output {
            println!("      output: {}", output.path().display());
        }
        print_step("documentation coverage", &target.coverage);
        println!("    cumulative duration: {:.2}s", target.duration().as_secs_f64());
        for path in &target.compiler_metrics {
            println!("    compiler metrics: {}", path.display());
        }
    }

    println!("  cumulative duration: {:.2}s", result.duration().as_secs_f64());

    match result.statistics.memory_peak_bytes() {
        Some(bytes) => println!(
            "  sandbox peak memory: {:.1} MiB",
            bytes as f64 / (1024.0 * 1024.0)
        ),
        None => println!("  sandbox peak memory: unavailable"),
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
    succeeded
}

fn target_fully_succeeded(target: &TargetBuildResult) -> bool {
    target.successful() && target.rustdoc_json.successful() && target.coverage.successful()
}

fn build_succeeded(default_succeeded: bool, auxiliary_succeeded: bool, strict: bool) -> bool {
    default_succeeded && (!strict || auxiliary_succeeded)
}

fn print_step<T>(name: &str, step: &StepResult<T>) {
    match (&step.output, &step.error) {
        (Some(_), None) => println!("    {name}: ok"),
        (None, None) => println!("    {name}: ok"),
        (_, Some(error)) => println!("    {name}: failed: {error:#}"),
    }
    println!("      duration: {:.2}s", step.duration.as_secs_f64());
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

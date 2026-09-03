use anyhow::Result;
use rustwide::cmd::{CommandError, SandboxImage};
use tracing::{debug, instrument};

pub(crate) fn args_contain_unstable_feature<S>(
    cargo_args: impl IntoIterator<Item = S>,
    feature: &str,
) -> bool
where
    S: AsRef<str>,
{
    let mut cargo_args = cargo_args.into_iter().peekable();

    while let Some(arg) = cargo_args.next() {
        let arg = arg.as_ref();
        if arg
            .strip_prefix("-Z")
            .is_some_and(|value| unstable_feature_matches(value, feature))
        {
            return true;
        }

        if arg == "-Z"
            && cargo_args
                .peek()
                .is_some_and(|next| unstable_feature_matches(next.as_ref(), feature))
        {
            return true;
        }
    }

    false
}

fn unstable_feature_matches(value: &str, feature: &str) -> bool {
    value == feature
        || value
            .strip_prefix(feature)
            .is_some_and(|suffix| suffix.starts_with('='))
}

/// Resolve a sandbox image name, preferring an existing local image and
/// falling back to a remote image that rustwide will pull when needed.
#[instrument(fields(image = name))]
pub fn resolve_sandbox_image(name: &str) -> Result<SandboxImage> {
    match SandboxImage::local(name) {
        Ok(image) => {
            debug!("using local sandbox image");
            Ok(image)
        }
        Err(CommandError::SandboxImageMissing(_)) => {
            debug!("local sandbox image is missing; resolving remote image");
            Ok(SandboxImage::remote(name)?)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(&[], "build-std" => false; "empty arguments")]
    #[test_case(&["-Zbuild-std"], "build-std" => true; "joined flag")]
    #[test_case(&["-Zbuild-std=core,alloc"], "build-std" => true; "joined flag with value")]
    #[test_case(&["-Z", "build-std"], "build-std" => true; "split flag")]
    #[test_case(&["-Z", "build-std=core,alloc"], "build-std" => true; "split flag with value")]
    #[test_case(&["rustdoc", "-Zbuild-std", "--lib"], "build-std" => true; "among other arguments")]
    #[test_case(&["build-std"], "build-std" => false; "missing z prefix")]
    #[test_case(&["-Z"], "build-std" => false; "z without feature")]
    #[test_case(&["-Zunstable-options"], "build-std" => false; "different feature")]
    #[test_case(&["-Zbuild-stdlib"], "build-std" => false; "feature name prefix")]
    #[test_case(&["-Zbuild-std-extra"], "build-std" => false; "feature name with suffix")]
    #[test_case(&["-Z", "build-stdlib"], "build-std" => false; "split feature name prefix")]
    #[test_case(&["-Zunstable-options"], "unstable-options" => true; "generic feature name")]
    fn detects_unstable_feature(args: &[&str], feature: &str) -> bool {
        args_contain_unstable_feature(args.iter().copied(), feature)
    }
}

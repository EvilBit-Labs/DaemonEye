#![forbid(unsafe_code)]

//! sentinel-lib: shared utilities for SentinelD.

/// Return a greeting message from the shared library for a specific component.
#[must_use]
pub fn greet(component: &str) -> String {
    format!("Hello from sentinel-lib to {component}!")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greet_returns_expected_message() {
        let got = greet("tests");
        assert_eq!(got, "Hello from sentinel-lib to tests!");
    }
}

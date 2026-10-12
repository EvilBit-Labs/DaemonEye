//! Network alert sinks for `daemoneye-agent`.
//!
//! This crate is the only workspace member that links the TLS stack. It holds the
//! webhook, syslog and email sinks and the one process-wide crypto-provider
//! bootstrap every TLS client in the agent relies on.

#![forbid(unsafe_code)]

/// Install `ring` as the process-wide rustls crypto provider.
///
/// Both `reqwest` and `lettre` read the process default; neither bundles a provider
/// in the feature set this workspace enables, so a client built before this call
/// would panic. A second call is a no-op: `install_default` refuses once a provider
/// is set, and that refusal is the success case here.
pub fn ensure_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // A race with another installer returns `Err` carrying the provider; either way a
        // provider is now installed, which is all this function promises.
        drop(rustls::crypto::ring::default_provider().install_default());
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::ensure_crypto_provider;

    /// The provider is installed by the first call and left alone by the second.
    #[test]
    fn provider_installs_once_and_tolerates_repeat_calls() {
        ensure_crypto_provider();
        let first = rustls::crypto::CryptoProvider::get_default().expect("installed");
        ensure_crypto_provider();
        let second = rustls::crypto::CryptoProvider::get_default().expect("still installed");
        assert!(
            std::sync::Arc::ptr_eq(first, second),
            "second call replaced the provider"
        );
    }
}

//! Maps transport errors to typed delivery errors.
//!
//! Only the class survives: a `reqwest::Error` can carry the request URL and a reply
//! fragment, so nothing from it is stored or rendered.

use daemoneye_lib::alerting::{DeliveryError, DeliveryErrorClass};
use std::error::Error as StdError;

/// Whether a rustls error means the peer's certificate was not accepted.
pub const fn is_certificate_error(error: &rustls::Error) -> bool {
    matches!(
        *error,
        rustls::Error::InvalidCertificate(_) | rustls::Error::NoCertificatesPresented
    )
}

/// The first rustls error in `error`'s source chain, if any.
pub fn tls_cause<'a>(error: &'a (dyn StdError + 'static)) -> Option<&'a rustls::Error> {
    let mut current = Some(error);
    while let Some(err) = current {
        if let Some(found) = err.downcast_ref::<rustls::Error>() {
            return Some(found);
        }
        // `io::Error::source` skips the wrapped error itself, so look inside it explicitly.
        if let Some(inner) = err
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .and_then(|inner| tls_cause(inner))
        {
            return Some(inner);
        }
        current = err.source();
    }
    None
}

/// Classify a failed request.
pub fn classify_reqwest(error: &reqwest::Error) -> DeliveryError {
    let transient = |class| DeliveryError::Transient {
        class,
        retry_after: None,
    };
    if error.is_timeout() {
        return transient(DeliveryErrorClass::Timeout);
    }
    if let Some(tls) = tls_cause(error) {
        return if is_certificate_error(tls) {
            DeliveryError::tls_verification()
        } else {
            transient(DeliveryErrorClass::Tls)
        };
    }
    if error.is_connect() {
        return transient(DeliveryErrorClass::Connect);
    }
    if error.is_body() || error.is_decode() || error.is_builder() {
        return DeliveryError::Permanent {
            class: DeliveryErrorClass::Payload,
        };
    }
    transient(DeliveryErrorClass::Io)
}

/// Classify a failed TLS handshake or stream error.
pub fn classify_tls_io(error: &std::io::Error) -> DeliveryError {
    if let Some(tls) = tls_cause(error) {
        return if is_certificate_error(tls) {
            DeliveryError::tls_verification()
        } else {
            DeliveryError::Transient {
                class: DeliveryErrorClass::Tls,
                retry_after: None,
            }
        };
    }
    DeliveryError::from(std::io::Error::from(error.kind()))
}

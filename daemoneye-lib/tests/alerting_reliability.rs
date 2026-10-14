//! Retry policy, circuit breaker and delivery-error mapping.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(feature = "alerting")]

use daemoneye_lib::alerting::{
    BreakerState, CircuitBreaker, DeliveryError, DeliveryErrorClass, Permit, RetryPolicy,
};
use rand::SeedableRng;
use rand::rngs::StdRng;
use std::io;
use std::time::Duration;

const DRAWS: usize = 200;

const fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

const fn transient() -> DeliveryError {
    DeliveryError::from_http_status(503, None)
}

const fn permanent() -> DeliveryError {
    DeliveryError::from_http_status(404, None)
}

#[test]
fn delay_for_stays_within_exponential_bound_and_max_grows() {
    let policy = RetryPolicy::new(5, secs(1), secs(60));
    let mut rng = StdRng::seed_from_u64(7);
    let mut previous_max = Duration::ZERO;
    for attempt in 0..4_u32 {
        let bound = secs(1 << attempt);
        let draws: Vec<Duration> = (0..DRAWS)
            .map(|_| policy.delay_for(attempt, &mut rng))
            .collect();
        let observed_max = draws.iter().copied().max().unwrap();
        assert!(draws.iter().all(|d| *d <= bound), "attempt {attempt}");
        assert!(
            observed_max > bound / 2,
            "attempt {attempt} not using range"
        );
        assert!(observed_max > previous_max, "max did not grow at {attempt}");
        previous_max = observed_max;
    }
}

#[test]
fn delay_for_never_exceeds_cap() {
    let policy = RetryPolicy::new(10, secs(30), secs(60));
    let mut rng = StdRng::seed_from_u64(1);
    for attempt in [2_u32, 3, 9, 40, u32::MAX] {
        for _ in 0..DRAWS {
            assert!(policy.delay_for(attempt, &mut rng) <= secs(60));
        }
    }
}

#[test]
fn retry_after_honours_delta_seconds_up_to_cap_and_ignores_other_forms() {
    let policy = RetryPolicy::new(3, secs(1), secs(60));
    assert_eq!(policy.retry_after("300"), Some(secs(60)));
    assert_eq!(policy.retry_after("5"), Some(secs(5)));
    assert_eq!(policy.retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
    assert_eq!(policy.retry_after("18446744073709551616"), None);
}

#[tokio::test(start_paused = true)]
async fn transient_failures_open_at_threshold_but_permanent_do_not() {
    let mut transient_breaker = CircuitBreaker::new(2, secs(30));
    transient_breaker.record_failure(&transient());
    assert_eq!(transient_breaker.state(), BreakerState::Closed);
    transient_breaker.record_failure(&transient());
    assert_eq!(transient_breaker.state(), BreakerState::Open);
    assert_eq!(transient_breaker.permit(), Permit::Deny);
    assert!(transient_breaker.closed_since().is_none());

    let mut permanent_breaker = CircuitBreaker::new(2, secs(30));
    permanent_breaker.record_failure(&permanent());
    permanent_breaker.record_failure(&permanent());
    assert_eq!(permanent_breaker.state(), BreakerState::Closed);
    assert_eq!(permanent_breaker.permit(), Permit::Allow);
}

#[tokio::test(start_paused = true)]
async fn misconfigured_failure_opens_immediately() {
    let mut breaker = CircuitBreaker::new(5, secs(30));
    breaker.record_failure(&DeliveryError::tls_verification());
    assert_eq!(breaker.state(), BreakerState::Open);
    assert_eq!(breaker.permit(), Permit::Deny);
}

#[tokio::test(start_paused = true)]
async fn success_resets_the_failure_count() {
    let mut breaker = CircuitBreaker::new(2, secs(30));
    breaker.record_failure(&transient());
    breaker.record_success();
    breaker.record_failure(&transient());
    assert_eq!(breaker.state(), BreakerState::Closed);
}

#[tokio::test(start_paused = true)]
async fn cooldown_admits_exactly_one_probe_and_success_closes() {
    let mut breaker = CircuitBreaker::new(1, secs(30));
    breaker.record_failure(&transient());
    tokio::time::advance(secs(29)).await;
    assert_eq!(breaker.permit(), Permit::Deny);
    tokio::time::advance(secs(1)).await;
    assert_eq!(breaker.permit(), Permit::AllowProbe);
    assert_eq!(breaker.state(), BreakerState::HalfOpen);
    assert_eq!(breaker.permit(), Permit::Deny);
    breaker.record_success();
    assert_eq!(breaker.state(), BreakerState::Closed);
    assert!(breaker.closed_since().is_some());
    assert_eq!(breaker.permit(), Permit::Allow);
}

#[tokio::test(start_paused = true)]
async fn failed_probe_reopens_with_a_full_cooldown() {
    let mut breaker = CircuitBreaker::new(1, secs(30));
    breaker.record_failure(&transient());
    tokio::time::advance(secs(30)).await;
    assert_eq!(breaker.permit(), Permit::AllowProbe);
    breaker.record_failure(&transient());
    assert_eq!(breaker.state(), BreakerState::Open);
    tokio::time::advance(secs(29)).await;
    assert_eq!(breaker.permit(), Permit::Deny);
    tokio::time::advance(secs(1)).await;
    assert_eq!(breaker.permit(), Permit::AllowProbe);
}

#[tokio::test(start_paused = true)]
async fn closed_since_is_creation_time_initially() {
    let before = tokio::time::Instant::now();
    let breaker = CircuitBreaker::new(1, secs(30));
    assert_eq!(breaker.closed_since(), Some(before));
}

#[test]
fn http_status_mapping() {
    let rate = DeliveryError::from_http_status(429, Some(secs(7)));
    assert_eq!(
        rate,
        DeliveryError::Transient {
            class: DeliveryErrorClass::HttpStatus(429),
            retry_after: Some(secs(7))
        }
    );
    assert!(matches!(
        DeliveryError::from_http_status(404, None),
        DeliveryError::Permanent { .. }
    ));
    assert_eq!(
        DeliveryError::from_http_status(404, None).class(),
        &DeliveryErrorClass::HttpStatus(404)
    );
    for status in [401_u16, 403] {
        let err = DeliveryError::from_http_status(status, None);
        assert!(matches!(err, DeliveryError::Misconfigured { .. }));
        assert_eq!(err.class(), &DeliveryErrorClass::Auth);
    }
    for status in [408_u16, 500, 503] {
        let err = DeliveryError::from_http_status(status, None);
        assert!(matches!(err, DeliveryError::Transient { .. }), "{status}");
        assert_eq!(err.class(), &DeliveryErrorClass::HttpStatus(status));
    }
}

#[test]
fn smtp_status_mapping() {
    let soft = DeliveryError::from_smtp_status(450);
    assert!(matches!(
        soft,
        DeliveryError::Transient {
            retry_after: None,
            ..
        }
    ));
    assert_eq!(soft.class(), &DeliveryErrorClass::SmtpStatus(450));
    let auth = DeliveryError::from_smtp_status(535);
    assert!(matches!(auth, DeliveryError::Misconfigured { .. }));
    assert_eq!(auth.class(), &DeliveryErrorClass::Auth);
    let hard = DeliveryError::from_smtp_status(550);
    assert!(matches!(hard, DeliveryError::Permanent { .. }));
    assert_eq!(hard.class(), &DeliveryErrorClass::SmtpStatus(550));
}

#[test]
fn io_and_tls_mapping() {
    let cases = [
        (
            io::ErrorKind::ConnectionRefused,
            DeliveryErrorClass::Connect,
        ),
        (io::ErrorKind::ConnectionReset, DeliveryErrorClass::Connect),
        (io::ErrorKind::TimedOut, DeliveryErrorClass::Timeout),
        (io::ErrorKind::BrokenPipe, DeliveryErrorClass::Io),
    ];
    for (kind, class) in cases {
        let err = DeliveryError::from(io::Error::new(kind, "secret detail"));
        assert!(
            matches!(
                err,
                DeliveryError::Transient {
                    retry_after: None,
                    ..
                }
            ),
            "{kind:?}"
        );
        assert_eq!(err.class(), &class);
    }
    let err = DeliveryError::tls_verification();
    assert!(matches!(err, DeliveryError::Misconfigured { .. }));
    assert_eq!(err.class(), &DeliveryErrorClass::TlsVerification);
}

#[test]
fn display_shows_only_class_and_code() {
    let io_err = DeliveryError::from(io::Error::other("secret detail"));
    assert!(!io_err.to_string().contains("secret"));
    assert_eq!(
        DeliveryError::from_http_status(503, None).to_string(),
        "transient: http status 503"
    );
    assert_eq!(
        DeliveryError::from_smtp_status(550).to_string(),
        "permanent: smtp status 550"
    );
    assert_eq!(
        DeliveryError::tls_verification().to_string(),
        "misconfigured: tls verification"
    );
}

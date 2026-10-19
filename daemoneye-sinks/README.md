# daemoneye-sinks

Network alert sinks for `daemoneye-agent`: webhook (HTTPS), syslog (UDP, TCP, TLS, Unix datagram) and email (SMTP). This is the only workspace crate that links the TLS stack (rustls with the `ring` provider, reqwest, lettre), so procmond, collector-core and the CLI never carry it.

//! Where every HTTP client starts.

/// A client builder for requests that leave the machine. On Android, reqwest's
/// default certificate check calls the system's verifier through the JVM: it
/// panics unless the app sets that up, and even then calls a certificate
/// revoked when its authority's revocation list can't be fetched. So Android
/// checks certificates against the copy of Mozilla's root list built into the
/// app. A client built any other way fails only on phones.
pub fn client() -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();
    #[cfg(target_os = "android")]
    let builder = builder.tls_certs_only(
        webpki_root_certs::TLS_SERVER_ROOT_CERTS
            .iter()
            .filter_map(|cert| reqwest::Certificate::from_der(cert).ok()),
    );
    builder
}

use core_runtime::{
    validate_public_navigation_url_with, NavigationNetworkPolicy, NavigationValidationError,
};
use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

fn resolve(addresses: Vec<IpAddr>) -> impl Fn(&str, u16) -> io::Result<Vec<IpAddr>> {
    move |_, _| Ok(addresses.clone())
}

#[test]
fn canonicalizes_destination_and_builds_secret_free_projection() {
    let validated = validate_public_navigation_url_with(
        "HTTPS://Example.COM:443/path?q=secret#fragment",
        NavigationNetworkPolicy::PublicOnly,
        resolve(vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))]),
    )
    .unwrap();

    assert_eq!(
        validated.canonical_url(),
        "https://example.com/path?q=secret"
    );
    assert_eq!(validated.sanitized_projection(), "https://example.com/path");
    assert_eq!(validated.destination_digest().len(), 64);
    assert!(!validated.destination_digest().contains("secret"));
}

#[test]
fn removes_a_trailing_dns_dot_before_resolution_policy_identity_and_audit_projection() {
    let validated = validate_public_navigation_url_with(
        "https://Example.COM./path?q=secret#fragment",
        NavigationNetworkPolicy::PublicOnly,
        |host, _| {
            assert_eq!(host, "example.com");
            Ok(vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))])
        },
    )
    .unwrap();
    assert_eq!(
        validated.canonical_url(),
        "https://example.com/path?q=secret"
    );
    assert_eq!(validated.sanitized_projection(), "https://example.com/path");
}

#[test]
fn rejects_credentials_forbidden_schemes_relative_and_oversized_utf8() {
    let public = resolve(vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))]);
    for input in [
        "https://user:password@example.com/",
        "file:///etc/passwd",
        "data:text/plain,hello",
        "javascript:alert(1)",
        "about:blank",
        "/relative",
        "not a url",
        "",
    ] {
        assert!(
            validate_public_navigation_url_with(
                input,
                NavigationNetworkPolicy::PublicOnly,
                &public,
            )
            .is_err(),
            "accepted {input:?}"
        );
    }

    let oversized = format!("https://example.com/{}", "界".repeat(3000));
    assert!(matches!(
        validate_public_navigation_url_with(
            &oversized,
            NavigationNetworkPolicy::PublicOnly,
            &public,
        ),
        Err(NavigationValidationError::TooLong { .. })
    ));
}

#[test]
fn rejects_any_non_global_dns_answer_and_ipv4_mapped_ipv6() {
    let mixed = vec![
        IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
    ];
    assert!(matches!(
        validate_public_navigation_url_with(
            "https://example.com/",
            NavigationNetworkPolicy::PublicOnly,
            resolve(mixed),
        ),
        Err(NavigationValidationError::NonPublicAddress)
    ));

    let mapped = IpAddr::V6(Ipv6Addr::from(0xffff_7f00_0001u128));
    assert!(matches!(
        validate_public_navigation_url_with(
            "https://example.com/",
            NavigationNetworkPolicy::PublicOnly,
            resolve(vec![mapped]),
        ),
        Err(NavigationValidationError::NonPublicAddress)
    ));

    let mapped_public = IpAddr::V6(Ipv6Addr::from(0xffff_5db8_d822u128));
    assert!(matches!(
        validate_public_navigation_url_with(
            "https://example.com/",
            NavigationNetworkPolicy::PublicOnly,
            resolve(vec![mapped_public]),
        ),
        Err(NavigationValidationError::NonPublicAddress)
    ));

    let benchmark = "2001:2::1".parse().unwrap();
    assert!(matches!(
        validate_public_navigation_url_with(
            "https://example.com/",
            NavigationNetworkPolicy::PublicOnly,
            resolve(vec![benchmark]),
        ),
        Err(NavigationValidationError::NonPublicAddress)
    ));
}

#[test]
fn enforces_exact_8192_and_8193_utf8_byte_boundaries() {
    fn url_with_len(target: usize) -> String {
        let base = "https://example.com/";
        let remaining = target - base.len();
        format!(
            "{base}{}{}",
            "é".repeat(remaining / 2),
            "a".repeat(remaining % 2)
        )
    }
    let public = resolve(vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))]);
    let at_limit = url_with_len(8192);
    let over_limit = url_with_len(8193);
    assert_eq!(at_limit.len(), 8192);
    assert_eq!(over_limit.len(), 8193);
    assert!(validate_public_navigation_url_with(
        &at_limit,
        NavigationNetworkPolicy::PublicOnly,
        &public,
    )
    .is_ok());
    assert!(matches!(
        validate_public_navigation_url_with(
            &over_limit,
            NavigationNetworkPolicy::PublicOnly,
            &public,
        ),
        Err(NavigationValidationError::TooLong { .. })
    ));
}

#[test]
fn private_network_opt_in_skips_dns_publicness_gate() {
    let validated = validate_public_navigation_url_with(
        "http://127.0.0.1:8080/start",
        NavigationNetworkPolicy::AllowPrivate,
        |_, _| panic!("resolver should not be called for an IP literal with private opt-in"),
    )
    .unwrap();
    assert_eq!(validated.canonical_url(), "http://127.0.0.1:8080/start");
}

#[test]
fn rejects_resolution_failure_and_empty_answers_without_disclosure() {
    let failed = validate_public_navigation_url_with(
        "https://secret.example/path?q=token",
        NavigationNetworkPolicy::PublicOnly,
        |_, _| Err(io::Error::new(io::ErrorKind::NotFound, "resolver detail")),
    )
    .unwrap_err();
    assert!(matches!(
        failed,
        NavigationValidationError::ResolutionFailed
    ));
    assert!(!failed.to_string().contains("secret.example"));
    assert!(!failed.to_string().contains("token"));

    assert!(matches!(
        validate_public_navigation_url_with(
            "https://example.com/",
            NavigationNetworkPolicy::PublicOnly,
            resolve(vec![]),
        ),
        Err(NavigationValidationError::ResolutionFailed)
    ));
}

#[test]
fn rejects_representative_iana_special_purpose_ranges() {
    let non_global = [
        "0.1.2.3",
        "10.0.0.1",
        "100.64.0.1",
        "127.0.0.1",
        "169.254.1.1",
        "172.16.0.1",
        "192.0.0.8",
        "192.0.2.1",
        "192.88.99.1",
        "192.168.0.1",
        "198.18.0.1",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "240.0.0.1",
        "::192.0.2.1",
        "::ffff:0:192.0.2.1",
        "64:ff9b:1::1",
        "100::1",
        "2001:2::1",
        "2001:db8::1",
        "2002::1",
        "3fff::1",
        "5f00::1",
        "fc00::1",
        "fe80::1",
        "fec0::1",
        "ff00::1",
    ];
    for address in non_global {
        let address: IpAddr = address.parse().unwrap();
        assert!(
            matches!(
                validate_public_navigation_url_with(
                    "https://example.com/",
                    NavigationNetworkPolicy::PublicOnly,
                    resolve(vec![address]),
                ),
                Err(NavigationValidationError::NonPublicAddress)
            ),
            "accepted special-purpose address {address}"
        );
    }
}

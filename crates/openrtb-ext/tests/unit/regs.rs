//! Go: `openrtb_ext/regs_test.go`.

use pbs_openrtb_ext::openrtb_ext::{ExtBidDSATransparency, ExtRegsDSA};

use super::support::{TestResult, all_ok, ensure_eq};

/// Go: `TestExtRegsDSAClone`: the clone equals the original (Go also checks that its pointers
/// and slice are new ones; a Rust clone owns its own).
#[test]
fn ext_regs_dsa_clone() -> TestResult {
    let none: Option<ExtRegsDSA> = None;
    let clone = none.clone();
    ensure_eq(&clone, &none, "nil")?;
    let cases = [
        (
            "required_not_nil",
            ExtRegsDSA {
                required: Some(1),
                ..ExtRegsDSA::default()
            },
        ),
        (
            "pubrender_not_nil",
            ExtRegsDSA {
                pub_render: Some(1),
                ..ExtRegsDSA::default()
            },
        ),
        (
            "datatopub_not_nil",
            ExtRegsDSA {
                data_to_pub: Some(1),
                ..ExtRegsDSA::default()
            },
        ),
        (
            "transparency_empty",
            ExtRegsDSA {
                transparency: Some(Vec::new()),
                ..ExtRegsDSA::default()
            },
        ),
        (
            "transparency_with_nil_params",
            ExtRegsDSA {
                transparency: Some(vec![ExtBidDSATransparency {
                    domain: "domain1".to_owned(),
                    params: None,
                }]),
                ..ExtRegsDSA::default()
            },
        ),
        (
            "transparency_with_params",
            ExtRegsDSA {
                required: Some(1),
                pub_render: Some(1),
                data_to_pub: Some(1),
                transparency: Some(vec![
                    ExtBidDSATransparency {
                        domain: "domain1".to_owned(),
                        params: Some(vec![1, 2, 3]),
                    },
                    ExtBidDSATransparency {
                        domain: "domain2".to_owned(),
                        params: Some(vec![4, 5, 6]),
                    },
                ]),
            },
        ),
    ];
    all_ok(
        cases
            .into_iter()
            .map(|(name, dsa)| (name.to_owned(), ensure_eq(&dsa.clone(), &dsa, "clone"))),
    )
}

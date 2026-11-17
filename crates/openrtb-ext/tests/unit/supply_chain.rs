//! Go: `openrtb_ext/supplyChain_test.go`.

use pbs_gojson::RawJson;
use pbs_openrtb::openrtb2::{SupplyChain, SupplyChainNode};
use pbs_openrtb_ext::openrtb_ext::clone_supply_chain;

use super::support::{TestResult, ensure_eq};

fn general() -> SupplyChain {
    SupplyChain {
        complete: 2,
        nodes: Some(vec![
            SupplyChainNode {
                sid: "alpha".to_owned(),
                name: "Johnny".to_owned(),
                hp: Some(5),
                ext: RawJson::from_slice(b"{}"),
                ..SupplyChainNode::default()
            },
            SupplyChainNode {
                asi: "Oh my".to_owned(),
                name: "Johnny".to_owned(),
                hp: Some(5),
                ext: RawJson::from_slice(br#"{"samson"}"#),
                ..SupplyChainNode::default()
            },
        ]),
        ver: "v2.5".to_owned(),
        ext: RawJson::from_slice(br#"{"foo": "bar"}"#),
    }
}

/// Go: `TestCloneSupplyChain`: changing the original after cloning leaves the clone as the
/// original was. (Go's nil chain is the caller's `None`.)
#[test]
fn clone_supply_chain_copies() -> TestResult {
    let none: Option<SupplyChain> = None;
    ensure_eq(&none.as_ref().map(clone_supply_chain), &None, "Nil")?;

    let mut schain = general();
    let schain_copy = general();
    let clone = clone_supply_chain(&schain);
    let nodes = schain.nodes.get_or_insert_with(Vec::new);
    if let Some(node) = nodes.get_mut(0) {
        node.sid = "beta".to_owned();
        node.ext = RawJson::nil();
    }
    if let Some(node) = nodes.get_mut(1) {
        node.hp = None;
    }
    nodes.push(SupplyChainNode {
        sid: "Gamma".to_owned(),
        ..SupplyChainNode::default()
    });
    schain.complete = 0;
    schain.ext = RawJson::from_slice(b"{}");
    ensure_eq(&clone, &schain_copy, "General")
}

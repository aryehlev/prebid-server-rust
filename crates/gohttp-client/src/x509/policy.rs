//! Go 1.26's certificate policy validation (`policiesValid` in `crypto/x509/verify.go`): RFC
//! 5280 section 6.1 as RFC 9618 updates it, with a TLS client's options (every policy acceptable,
//! nothing inhibited or required up front).

use std::collections::{BTreeMap, BTreeSet};

use super::cert::Certificate;

/// The DER contents of anyPolicy (2.5.29.32.0).
const ANY_POLICY: &[u8] = &[0x55, 0x1D, 0x20, 0x00];

/// Go: `policyGraphNode`, in the graph's arena.
#[derive(Debug)]
struct Node {
    valid: Vec<u8>,
    expected: Vec<Vec<u8>>,
    parents: BTreeSet<usize>,
    children: BTreeSet<usize>,
}

/// Go: `policyGraph`.
#[derive(Debug)]
struct Graph {
    nodes: Vec<Node>,
    strata: Vec<BTreeMap<Vec<u8>, usize>>,
    /// The nodes of the previous stratum by each policy they expect.
    parent_index: BTreeMap<Vec<u8>, Vec<usize>>,
    depth: usize,
}

impl Graph {
    /// Go: `newPolicyGraph`: an anyPolicy root.
    fn new() -> Self {
        let root = Node {
            valid: ANY_POLICY.to_vec(),
            expected: vec![ANY_POLICY.to_vec()],
            parents: BTreeSet::new(),
            children: BTreeSet::new(),
        };
        Self {
            nodes: vec![root],
            strata: vec![BTreeMap::from([(ANY_POLICY.to_vec(), 0)])],
            parent_index: BTreeMap::new(),
            depth: 0,
        }
    }

    /// Go: `newPolicyGraphNode`, linked to its parents.
    fn new_node(&mut self, valid: &[u8], parents: &[usize]) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Node {
            valid: valid.to_vec(),
            expected: vec![valid.to_vec()],
            parents: parents.iter().copied().collect(),
            children: BTreeSet::new(),
        });
        for &p in parents {
            if let Some(parent) = self.nodes.get_mut(p) {
                let _new = parent.children.insert(id);
            }
        }
        id
    }

    /// Go: `insert`, into the current stratum.
    fn insert(&mut self, id: usize) {
        let Some(valid) = self.nodes.get(id).map(|n| n.valid.clone()) else {
            return;
        };
        if let Some(stratum) = self.strata.get_mut(self.depth) {
            let _replaced = stratum.insert(valid, id);
        }
    }

    fn stratum(&self, depth: usize) -> impl Iterator<Item = usize> + '_ {
        self.strata
            .get(depth)
            .into_iter()
            .flat_map(|s| s.values().copied())
    }

    /// Go: `parentsWithExpected`.
    fn parents_with_expected(&self, expected: &[u8]) -> Vec<usize> {
        if self.depth == 0 {
            return Vec::new();
        }
        self.parent_index.get(expected).cloned().unwrap_or_default()
    }

    /// Go: `leafWithPolicy`, or `parentWithAnyPolicy` with the previous depth.
    fn with_policy(&self, depth: usize, policy: &[u8]) -> Option<usize> {
        self.strata.get(depth)?.get(policy).copied()
    }

    /// Go: `deleteLeaf`.
    fn delete_leaf(&mut self, policy: &[u8]) {
        let Some(id) = self.with_policy(self.depth, policy) else {
            return;
        };
        let (parents, children) = self.nodes.get(id).map_or_else(Default::default, |n| {
            (n.parents.clone(), n.children.clone())
        });
        for p in parents {
            if let Some(parent) = self.nodes.get_mut(p) {
                let _removed = parent.children.remove(&id);
            }
        }
        for c in children {
            if let Some(child) = self.nodes.get_mut(c) {
                let _removed = child.parents.remove(&id);
            }
        }
        if let Some(stratum) = self.strata.get_mut(self.depth) {
            let _removed = stratum.remove(policy);
        }
    }

    /// Go: `validPolicyNodes`: the nodes other than anyPolicy whose only parent is anyPolicy.
    fn valid_policy_nodes(&self) -> Vec<usize> {
        (0..=self.depth)
            .rev()
            .flat_map(|depth| self.stratum(depth))
            .filter(|&id| {
                self.nodes.get(id).is_some_and(|n| {
                    n.valid != ANY_POLICY
                        && n.parents.len() == 1
                        && n.parents
                            .iter()
                            .all(|&p| self.nodes.get(p).is_some_and(|p| p.valid == ANY_POLICY))
                })
            })
            .collect()
    }

    /// Go: `prune`: drops the childless nodes above the leaves, deepest stratum first.
    fn prune(&mut self) {
        for depth in (1..self.depth).rev() {
            let childless: Vec<usize> = self
                .stratum(depth)
                .filter(|&id| self.nodes.get(id).is_some_and(|n| n.children.is_empty()))
                .collect();
            for id in childless {
                let (valid, parents) = self
                    .nodes
                    .get(id)
                    .map_or_else(Default::default, |n| (n.valid.clone(), n.parents.clone()));
                for p in parents {
                    if let Some(parent) = self.nodes.get_mut(p) {
                        let _removed = parent.children.remove(&id);
                    }
                }
                if let Some(stratum) = self.strata.get_mut(depth) {
                    let _removed = stratum.remove(&valid);
                }
            }
        }
    }

    /// Go: `incrDepth`.
    fn incr_depth(&mut self) {
        let mut index: BTreeMap<Vec<u8>, Vec<usize>> = BTreeMap::new();
        for id in self.stratum(self.depth) {
            for e in self
                .nodes
                .get(id)
                .into_iter()
                .flat_map(|node| &node.expected)
            {
                index.entry(e.clone()).or_default().push(id);
            }
        }
        self.parent_index = index;
        self.depth += 1;
        self.strata.push(BTreeMap::new());
    }

    /// Go: RFC 5280 section 6.1.3 (d) as RFC 9618 updates it: a stratum for the certificate's
    /// policies, anyPolicy's children when `any_allowed`, then the pruning.
    fn add_policies(&mut self, cert: &Certificate, any_allowed: bool) {
        self.incr_depth();
        for policy in &cert.policies {
            if policy == ANY_POLICY {
                continue;
            }
            let mut parents = self.parents_with_expected(policy);
            if parents.is_empty()
                && let Some(any) = self.with_policy(self.depth - 1, ANY_POLICY)
            {
                parents = vec![any];
            }
            if !parents.is_empty() {
                let id = self.new_node(policy, &parents);
                self.insert(id);
            }
        }
        if any_allowed && cert.policies.iter().any(|p| p == ANY_POLICY) {
            let mut missing: BTreeMap<Vec<u8>, Vec<usize>> = BTreeMap::new();
            for p in self.stratum(self.depth - 1) {
                for expected in self
                    .nodes
                    .get(p)
                    .into_iter()
                    .flat_map(|node| &node.expected)
                {
                    if self.with_policy(self.depth, expected).is_none() {
                        missing.entry(expected.clone()).or_default().push(p);
                    }
                }
            }
            for (policy, parents) in missing {
                let id = self.new_node(&policy, &parents);
                self.insert(id);
            }
        }
        self.prune();
    }

    /// Go: RFC 5280 section 6.1.4 (a) and (b) as RFC 9618 updates them: the certificate's policy
    /// mappings when `mapping_allowed`, else the mapped policies deleted. False for a mapping to
    /// or from anyPolicy, which fails the chain.
    fn map_policies(&mut self, cert: &Certificate, mapping_allowed: bool) -> bool {
        let mut mappings: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
        for (issuer, subject) in &cert.policy_mappings {
            if mapping_allowed {
                if issuer == ANY_POLICY || subject == ANY_POLICY {
                    return false;
                }
                mappings
                    .entry(issuer.clone())
                    .or_default()
                    .push(subject.clone());
            } else {
                self.delete_leaf(issuer);
            }
        }
        self.prune();
        for (issuer, subjects) in mappings {
            if let Some(id) = self.with_policy(self.depth, &issuer) {
                if let Some(node) = self.nodes.get_mut(id) {
                    node.expected = subjects;
                }
            } else if let Some(any) = self.with_policy(self.depth, ANY_POLICY) {
                let id = self.new_node(&issuer, &[any]);
                if let Some(node) = self.nodes.get_mut(id) {
                    node.expected = subjects;
                }
                self.insert(id);
            }
        }
        true
    }
}

/// Lowers `limit` to a policy constraint's value when the certificate sets one (Go: the field is
/// set, or its `...Zero` flag is).
const fn constrain(limit: &mut i64, value: Option<i64>) {
    if let Some(v) = value
        && v >= 0
        && v < *limit
    {
        *limit = v;
    }
}

/// Go: `policiesValid(chain, opts)` for a chain from the leaf to the root.
pub(super) fn policies_valid(chain: &[&Certificate]) -> bool {
    let Some(n) = chain.len().checked_sub(1).filter(|&n| n > 0) else {
        return true;
    };
    let mut graph = Some(Graph::new());
    let start = i64::try_from(n + 1).unwrap_or(i64::MAX);
    let (mut inhibit_any_policy, mut explicit_policy, mut policy_mapping) = (start, start, start);
    for i in (0..n).rev() {
        let Some(cert) = chain.get(i) else {
            return false;
        };
        let self_signed = cert.issuer == cert.subject;
        if cert.policies.is_empty() {
            graph = None;
        }
        if explicit_policy == 0 && graph.is_none() {
            return false;
        }
        if let Some(g) = graph.as_mut() {
            g.add_policies(cert, inhibit_any_policy > 0 || (i > 0 && self_signed));
            if i != 0
                && !cert.policy_mappings.is_empty()
                && !g.map_policies(cert, policy_mapping > 0)
            {
                return false;
            }
        }
        if i != 0 {
            if !self_signed {
                for limit in [
                    &mut explicit_policy,
                    &mut policy_mapping,
                    &mut inhibit_any_policy,
                ] {
                    if *limit > 0 {
                        *limit -= 1;
                    }
                }
            }
            constrain(&mut explicit_policy, cert.require_explicit_policy);
            constrain(&mut policy_mapping, cert.inhibit_policy_mapping);
            constrain(&mut inhibit_any_policy, cert.inhibit_any_policy);
        }
    }
    if explicit_policy > 0 {
        explicit_policy -= 1;
    }
    if chain
        .first()
        .is_some_and(|leaf| leaf.require_explicit_policy == Some(0))
    {
        explicit_policy = 0;
    }
    let any_valid = graph.as_ref().is_some_and(|g| {
        !g.valid_policy_nodes().is_empty() || g.with_policy(g.depth, ANY_POLICY).is_some()
    });
    explicit_policy != 0 || any_valid
}

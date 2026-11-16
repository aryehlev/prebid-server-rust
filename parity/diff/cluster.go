package diff

import (
	"sort"

	"github.com/prebid/prebid-server/v4/parity"
)

// Cluster groups every mismatch with one signature across cases: the same
// stage, domain, JSON path pattern and kind, which is one defect to fix.
type Cluster struct {
	Signature   string              `json:"signature"`
	Stage       parity.StageID      `json:"stage"`
	Domain      parity.Domain       `json:"domain"`
	Requirement string              `json:"requirement"`
	Kind        parity.MismatchKind `json:"kind"`
	// Classes are the lexical or wire classes seen in the cluster.
	Classes     []string `json:"classes"`
	PathPattern string   `json:"path_pattern"`
	// Count is the number of records; PrimaryCount of them were primary.
	Count        int `json:"count"`
	PrimaryCount int `json:"primary_count"`
	// Cases are the distinct case IDs, sorted.
	Cases []string `json:"cases"`
	// Example is the first record of the cluster (smallest case ID, then
	// JSON path), the representative for minimization and reports.
	Example parity.MismatchRecord `json:"example"`
}

// Primary reports whether any record of the cluster was primary, which makes
// the cluster a root cause rather than only a consequence.
func (c Cluster) Primary() bool { return c.PrimaryCount > 0 }

// BuildClusters groups the mismatches of many diff documents by signature.
// Clusters are ordered by primary count, then case count, then record count
// (all descending), then signature, so the most important root causes come
// first and the order is deterministic.
func BuildClusters(docs []parity.DiffDocument) []Cluster {
	bySig := map[string]*Cluster{}
	cases := map[string]map[string]bool{}
	classes := map[string]map[string]bool{}
	for _, doc := range docs {
		for _, m := range doc.Mismatches {
			c, ok := bySig[m.Signature]
			if !ok {
				c = &Cluster{
					Signature: m.Signature, Stage: m.Stage, Domain: m.Domain, Requirement: m.Requirement,
					Kind: m.Kind, PathPattern: PathPattern(m.JSONPath), Example: m,
				}
				bySig[m.Signature] = c
				cases[m.Signature] = map[string]bool{}
				classes[m.Signature] = map[string]bool{}
			}
			c.Count++
			if m.Primary {
				c.PrimaryCount++
			}
			cases[m.Signature][m.CaseID] = true
			if m.Class != "" {
				classes[m.Signature][m.Class] = true
			}
			if exampleLess(m, c.Example) {
				c.Example = m
			}
		}
	}
	out := make([]Cluster, 0, len(bySig))
	for sig, c := range bySig {
		c.Cases = sortedSet(cases[sig])
		c.Classes = sortedSet(classes[sig])
		out = append(out, *c)
	}
	sort.Slice(out, func(i, j int) bool {
		a, b := out[i], out[j]
		switch {
		case a.PrimaryCount != b.PrimaryCount:
			return a.PrimaryCount > b.PrimaryCount
		case len(a.Cases) != len(b.Cases):
			return len(a.Cases) > len(b.Cases)
		case a.Count != b.Count:
			return a.Count > b.Count
		}
		return a.Signature < b.Signature
	})
	return out
}

func exampleLess(a, b parity.MismatchRecord) bool {
	if a.CaseID != b.CaseID {
		return a.CaseID < b.CaseID
	}
	if a.JSONPath != b.JSONPath {
		return a.JSONPath < b.JSONPath
	}
	return a.Key < b.Key
}

// PrimaryClusters returns the clusters that contain at least one primary record.
func PrimaryClusters(clusters []Cluster) []Cluster {
	var out []Cluster
	for _, c := range clusters {
		if c.Primary() {
			out = append(out, c)
		}
	}
	return out
}

// PrimarySignatures returns the distinct signatures of a document's primary
// mismatches, sorted. The minimizer keeps an edit only while these reproduce.
func PrimarySignatures(doc parity.DiffDocument) []string {
	set := map[string]bool{}
	for _, m := range doc.Mismatches {
		if m.Primary {
			set[m.Signature] = true
		}
	}
	return sortedSet(set)
}

// CountByKind returns the number of mismatches per kind across documents, the
// totals CI ratchets against parity/baseline.json.
func CountByKind(docs []parity.DiffDocument) map[parity.MismatchKind]int {
	out := map[parity.MismatchKind]int{}
	for _, doc := range docs {
		for _, m := range doc.Mismatches {
			out[m.Kind]++
		}
	}
	return out
}

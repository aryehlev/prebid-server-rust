package report

import (
	"bytes"
	"encoding/xml"
	"fmt"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
)

// junitDetailRecords caps the mismatch lines listed in one failure.
const junitDetailRecords = 20

// SuiteFixtures is the JUnit suite name of the adapter fixture self-test.
const SuiteFixtures = "fixture-selftest"

type junitSuites struct {
	XMLName  xml.Name     `xml:"testsuites"`
	Name     string       `xml:"name,attr"`
	Tests    int          `xml:"tests,attr"`
	Failures int          `xml:"failures,attr"`
	Errors   int          `xml:"errors,attr"`
	Skipped  int          `xml:"skipped,attr"`
	Time     string       `xml:"time,attr"`
	Suites   []junitSuite `xml:"testsuite"`
}

type junitSuite struct {
	Name     string      `xml:"name,attr"`
	Tests    int         `xml:"tests,attr"`
	Failures int         `xml:"failures,attr"`
	Errors   int         `xml:"errors,attr"`
	Skipped  int         `xml:"skipped,attr"`
	Time     string      `xml:"time,attr"`
	Cases    []junitCase `xml:"testcase"`

	seconds float64
}

type junitCase struct {
	Name      string        `xml:"name,attr"`
	Classname string        `xml:"classname,attr"`
	Time      string        `xml:"time,attr"`
	Failure   *junitMessage `xml:"failure,omitempty"`
	Error     *junitMessage `xml:"error,omitempty"`
	Skipped   *junitMessage `xml:"skipped,omitempty"`
}

type junitMessage struct {
	Message string `xml:"message,attr"`
	Type    string `xml:"type,attr,omitempty"`
	Text    string `xml:",chardata"`
}

// JUnit renders junit.xml: one suite per corpus with one test case per parity
// case (mismatch is a failure, error an error, not-yet-instrumented, skipped
// and not-compared are skipped), plus the fixture self-test suite when
// present.
func JUnit(r *Run) ([]byte, error) {
	diffs := make(map[string]parity.DiffDocument, len(r.Diffs))
	for _, d := range r.Diffs {
		diffs[d.CaseID] = d
	}
	bySuite := map[string]*junitSuite{}
	for _, c := range r.Cases {
		name := c.Corpus
		if name == "" {
			name = "cases"
		}
		s := suiteFor(bySuite, name)
		s.add(caseResult(c, diffs[c.CaseID]), float64(c.DurationMS)/1000)
	}
	if len(r.Fixtures) > 0 {
		s := suiteFor(bySuite, SuiteFixtures)
		for _, f := range r.Fixtures {
			s.add(fixtureCase(f), 0)
		}
	}
	return encodeSuites(r.Summary, bySuite)
}

func suiteFor(m map[string]*junitSuite, name string) *junitSuite {
	if s, ok := m[name]; ok {
		return s
	}
	s := &junitSuite{Name: name}
	m[name] = s
	return s
}

func (s *junitSuite) add(c junitCase, seconds float64) {
	c.Time = seconds3(seconds)
	s.seconds += seconds
	s.Tests++
	switch {
	case c.Failure != nil:
		s.Failures++
	case c.Error != nil:
		s.Errors++
	case c.Skipped != nil:
		s.Skipped++
	}
	s.Cases = append(s.Cases, c)
}

func encodeSuites(summary RunSummary, bySuite map[string]*junitSuite) ([]byte, error) {
	names := make([]string, 0, len(bySuite))
	for n := range bySuite {
		names = append(names, n)
	}
	sort.Strings(names)
	doc := junitSuites{Name: "parity " + summary.RunID, Time: seconds3(float64(summary.DurationMS) / 1000)}
	for _, n := range names {
		s := *bySuite[n]
		s.Time = seconds3(s.seconds)
		doc.Tests += s.Tests
		doc.Failures += s.Failures
		doc.Errors += s.Errors
		doc.Skipped += s.Skipped
		doc.Suites = append(doc.Suites, s)
	}
	var b bytes.Buffer
	b.WriteString(xml.Header)
	enc := xml.NewEncoder(&b)
	enc.Indent("", "  ")
	if err := enc.Encode(doc); err != nil {
		return nil, fmt.Errorf("report: encoding junit.xml: %w", err)
	}
	b.WriteString("\n")
	return b.Bytes(), nil
}

// caseResult is the JUnit test case of one parity case.
func caseResult(c CaseSummary, doc parity.DiffDocument) junitCase {
	jc := junitCase{Name: c.CaseID, Classname: classname(c)}
	switch c.Status {
	case CaseMismatch:
		jc.Failure = &junitMessage{Type: string(CaseMismatch), Message: failureMessage(c, doc), Text: mismatchLines(doc)}
	case CaseError:
		jc.Error = &junitMessage{Type: string(CaseError), Message: c.Reason}
	case CaseNotYetInstrumented, CaseSkipped, CaseNotCompared:
		jc.Skipped = &junitMessage{Message: string(c.Status) + ": " + c.Reason}
	}
	return jc
}

// classname groups cases by endpoint and kind.
func classname(c CaseSummary) string {
	parts := []string{"parity"}
	for _, p := range []string{c.Corpus, c.Kind} {
		if p != "" {
			parts = append(parts, p)
		}
	}
	return strings.Join(parts, ".")
}

func failureMessage(c CaseSummary, doc parity.DiffDocument) string {
	msg := fmt.Sprintf("%d mismatches, %d primary", c.Mismatches, c.Primary)
	for _, m := range doc.Mismatches {
		if m.Primary {
			return msg + fmt.Sprintf("; first at %s %s %s", m.Stage, kindAndClass(m), m.JSONPath)
		}
	}
	return msg
}

// mismatchLines lists the primary records, then a count of the others.
func mismatchLines(doc parity.DiffDocument) string {
	var b strings.Builder
	shown, rest := 0, 0
	for _, m := range doc.Mismatches {
		if !m.Primary || shown == junitDetailRecords {
			rest++
			continue
		}
		fmt.Fprintf(&b, "%s[%s] %s %s (%s, %s)\n", m.Stage, m.Key, kindAndClass(m), m.JSONPath, m.Domain, m.Requirement)
		shown++
	}
	if rest > 0 {
		fmt.Fprintf(&b, "... %d more records (consequential or beyond the first %d)\n", rest, junitDetailRecords)
	}
	return b.String()
}

func kindAndClass(m parity.MismatchRecord) string {
	if m.Class == "" {
		return string(m.Kind)
	}
	return string(m.Kind) + "/" + m.Class
}

func fixtureCase(f FixtureResult) junitCase {
	jc := junitCase{Name: f.CaseID, Classname: "parity.fixtures." + f.Bidder}
	switch f.Status {
	case FixtureFailing:
		jc.Failure = &junitMessage{Type: FixtureFailing, Message: firstOr(f.Failures, "fixture expectation failed"), Text: strings.Join(f.Failures, "\n")}
	case FixtureExcused:
		jc.Skipped = &junitMessage{Message: "excused (" + f.Excuse + "): " + firstOr(f.Failures, "")}
	case FixtureNotRun:
		jc.Skipped = &junitMessage{Message: "not run: " + firstOr(f.Failures, "")}
	}
	return jc
}

func firstOr(list []string, def string) string {
	if len(list) == 0 {
		return def
	}
	return list[0]
}

func seconds3(s float64) string {
	return fmt.Sprintf("%.3f", s)
}

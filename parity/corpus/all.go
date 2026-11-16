package corpus

import (
	"fmt"
	"sort"
)

// Corpus is every seed corpus of the repository and the cases built from it.
type Corpus struct {
	Adapters *AdapterCorpus
	Samples  []*SampleRequest
	Exchange []*ExchangeSpec
	Params   *BidderParamsCorpus
	Seeds    []Seed
	// Cases are sorted by ID; IDs are unique.
	Cases []Case
	// Errors are per-file load or conversion errors. Loading continues past
	// them so one broken fixture never hides the rest.
	Errors []error
}

// Stats summarises a corpus for logs and tests.
type Stats struct {
	AdapterFixtures int            `json:"adapter_fixtures"`
	AdapterRuns     int            `json:"adapter_runs"`
	ResolvedRuns    int            `json:"resolved_runs"`
	SampleRequests  int            `json:"sample_requests"`
	ExchangeSpecs   int            `json:"exchange_specs"`
	ParamsSchemas   int            `json:"params_schemas"`
	ParamsExamples  int            `json:"params_examples"`
	Seeds           int            `json:"seeds"`
	CasesByKind     map[Kind]int   `json:"cases_by_kind"`
	CasesByTag      map[string]int `json:"cases_by_tag"`
	Errors          int            `json:"errors"`
}

// LoadAll loads every corpus and builds all cases. It returns an error only
// when a corpus cannot be read at all; per-file problems land in Errors.
func LoadAll(repo *Repo) (*Corpus, error) {
	c := &Corpus{}
	var err error
	if c.Adapters, err = LoadAdapterCorpus(repo); err != nil {
		return nil, fmt.Errorf("corpus: adapters: %w", err)
	}
	c.Errors = append(c.Errors, c.Adapters.Errors...)
	c.addCases(c.Adapters.AdapterCases())
	c.addCases(c.Adapters.LiftedCases())

	var errs []error
	c.Samples, errs = LoadSampleRequests(repo)
	c.Errors = append(c.Errors, errs...)
	c.addCases(SampleCases(repo, c.Samples))

	c.Exchange, errs = LoadExchangeSpecs(repo)
	c.Errors = append(c.Errors, errs...)
	keys, err := SyncerKeys(repo)
	if err != nil {
		return nil, err
	}
	c.addCases(ExchangeCases(c.Exchange, keys))

	if c.Params, err = LoadBidderParams(repo, c.Adapters); err != nil {
		return nil, fmt.Errorf("corpus: bidder params: %w", err)
	}
	c.addCases(c.Params.Cases(), nil)

	c.Seeds, errs = LoadSeeds(repo)
	c.Errors = append(c.Errors, errs...)
	c.Errors = append(c.Errors, c.sortCases()...)
	return c, nil
}

func (c *Corpus) addCases(cases []Case, errs []error) {
	c.Cases = append(c.Cases, cases...)
	c.Errors = append(c.Errors, errs...)
}

// sortCases orders cases by ID and reports duplicate IDs.
func (c *Corpus) sortCases() []error {
	sort.SliceStable(c.Cases, func(i, j int) bool { return c.Cases[i].ID < c.Cases[j].ID })
	var errs []error
	for i := 1; i < len(c.Cases); i++ {
		if c.Cases[i].ID == c.Cases[i-1].ID {
			errs = append(errs, fmt.Errorf("corpus: duplicate case id %q", c.Cases[i].ID))
		}
	}
	return errs
}

// Case returns the case with the given ID.
func (c *Corpus) Case(id string) (*Case, bool) {
	i := sort.Search(len(c.Cases), func(i int) bool { return c.Cases[i].ID >= id })
	if i < len(c.Cases) && c.Cases[i].ID == id {
		return &c.Cases[i], true
	}
	return nil, false
}

// Stats counts the corpus contents.
func (c *Corpus) Stats() Stats {
	s := Stats{CasesByKind: map[Kind]int{}, CasesByTag: map[string]int{}, Errors: len(c.Errors)}
	if c.Adapters != nil {
		s.AdapterFixtures = len(c.Adapters.Fixtures)
		s.AdapterRuns = len(c.Adapters.Runs)
		for _, run := range c.Adapters.Runs {
			if run.Resolved {
				s.ResolvedRuns++
			}
		}
	}
	s.SampleRequests = len(c.Samples)
	s.ExchangeSpecs = len(c.Exchange)
	if c.Params != nil {
		s.ParamsSchemas = len(c.Params.Schemas)
		s.ParamsExamples = len(c.Params.Examples)
	}
	s.Seeds = len(c.Seeds)
	for _, cs := range c.Cases {
		s.CasesByKind[cs.Kind]++
		for _, tag := range cs.Tags {
			s.CasesByTag[tag]++
		}
	}
	return s
}

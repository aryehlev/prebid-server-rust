package main

import (
	"net/http"
	"net/http/httptest"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promhttp"
)

// metricsSample is one counter value Prebid Server could reach.
type metricsSample struct {
	RequestType string  `json:"request_type"`
	Value       float64 `json:"value"`
}

type metricsDoc struct {
	Header  header          `json:"header"`
	Samples []metricsSample `json:"samples"`
	// Exposition is what the /metrics listener serves for them (server/prometheus.go:22-26).
	Exposition string `json:"exposition"`
}

// metricsGolden builds the requests counter as metrics/prometheus/prometheus.go:229-232 and
// newCounter (:648-658) do, adds the samples and reads the exposition through promhttp.
func metricsGolden() metricsDoc {
	doc := metricsDoc{
		Header: newHeader("github.com/prometheus/client_golang", "github.com/prometheus/common"),
		Samples: []metricsSample{
			{"amp", 1e6}, {"cookie_sync", 100000}, {"openrtb2-web", 1}, {"video", 123456789},
		},
	}
	registry := prometheus.NewRegistry()
	requests := prometheus.NewCounterVec(prometheus.CounterOpts{
		Namespace: "pbs",
		Name:      "requests",
		Help:      "Count of total requests to Prebid Server labeled by type and status.",
	}, []string{"request_type", "request_status"})
	registry.MustRegister(requests)
	for _, s := range doc.Samples {
		requests.WithLabelValues(s.RequestType, "ok").Add(s.Value)
	}
	rec := httptest.NewRecorder()
	promhttp.HandlerFor(registry, promhttp.HandlerOpts{}).ServeHTTP(rec, httptest.NewRequest(http.MethodGet, "/metrics", nil))
	doc.Exposition = rec.Body.String()
	return doc
}

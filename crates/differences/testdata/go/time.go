package main

import "time"

// formattedTime is one instant in Go's RFC3339 and RFC3339Nano layouts.
type formattedTime struct {
	Unix        int64  `json:"unix"`
	Nanos       int    `json:"nanos"`
	RFC3339     string `json:"rfc3339"`
	RFC3339Nano string `json:"rfc3339_nano"`
}

type timeDoc struct {
	Header header          `json:"header"`
	Times  []formattedTime `json:"times"`
}

// timeGolden formats instants as the HTTP stored-request events do with RFC3339
// (stored_requests/events/http/http.go:127).
func timeGolden() timeDoc {
	doc := timeDoc{Header: newHeader()}
	for _, nanos := range []int{250000000, 0, 123456789} {
		t := time.Date(2026, time.November, 18, 10, 20, 30, nanos, time.UTC)
		doc.Times = append(doc.Times, formattedTime{
			Unix:        t.Unix(),
			Nanos:       nanos,
			RFC3339:     t.Format(time.RFC3339),
			RFC3339Nano: t.Format(time.RFC3339Nano),
		})
	}
	return doc
}

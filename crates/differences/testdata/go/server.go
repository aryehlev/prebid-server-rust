package main

import (
	"net/http"
	"net/http/httptest"

	"github.com/prebid/prebid-server/v4/router"
)

// preflight is one CORS preflight and the answer router.SupportCORS gave it.
type preflight struct {
	RequestHeaders string              `json:"request_headers"`
	Status         int                 `json:"status"`
	Headers        map[string][]string `json:"headers"`
}

type serverDoc struct {
	Header        header      `json:"header"`
	Origin        string      `json:"origin"`
	RequestMethod string      `json:"request_method"`
	Path          string      `json:"path"`
	Preflights    []preflight `json:"preflights"`
}

// serverGolden sends preflights through router.SupportCORS (router/router.go:377-385), which
// main.go wraps around the whole router.
func serverGolden() serverDoc {
	doc := serverDoc{
		Header:        newHeader("github.com/rs/cors"),
		Origin:        "https://publisher.example",
		RequestMethod: http.MethodPost,
		Path:          "/openrtb2/auction",
	}
	handler := router.SupportCORS(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))
	// The lists a browser sends (lower case, comma-separated), the last one with a header
	// SupportCORS does not allow.
	for _, requested := range []string{"content-type", "content-type,x-requested-with", "x-custom"} {
		req := httptest.NewRequest(http.MethodOptions, "http://pbs.example"+doc.Path, nil)
		req.Header.Set("Origin", doc.Origin)
		req.Header.Set("Access-Control-Request-Method", doc.RequestMethod)
		req.Header.Set("Access-Control-Request-Headers", requested)
		rec := httptest.NewRecorder()
		handler.ServeHTTP(rec, req)
		doc.Preflights = append(doc.Preflights, preflight{
			RequestHeaders: requested,
			Status:         rec.Code,
			Headers:        rec.Header(),
		})
	}
	return doc
}

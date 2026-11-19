package main

import (
	"net/http"
	"time"
)

type cookieDoc struct {
	Header  header `json:"header"`
	Name    string `json:"name"`
	Value   string `json:"value"`
	Path    string `json:"path"`
	Domain  string `json:"domain"`
	Expires string `json:"expires"`
	// SetCookie is http.Cookie's String for these attributes, Secure and SameSite=None.
	SetCookie string `json:"set_cookie"`
}

// cookieGolden writes the uids cookie as usersync.WriteCookie builds it for a site cookie
// (usersync/cookie.go:93-113), with a fixed expiry. The value is base64 with padding, as an
// encoded uids cookie can be.
func cookieGolden() cookieDoc {
	expires := time.Date(2026, time.November, 18, 10, 20, 30, 0, time.UTC)
	doc := cookieDoc{
		Header:  newHeader(),
		Name:    "uids",
		Value:   "eyJ1aWRzIjp7fX0=",
		Path:    "/",
		Domain:  "pbs.example",
		Expires: expires.Format(time.RFC3339),
	}
	c := &http.Cookie{
		Name:     doc.Name,
		Value:    doc.Value,
		Expires:  expires,
		Path:     doc.Path,
		Domain:   doc.Domain,
		Secure:   true,
		SameSite: http.SameSiteNoneMode,
	}
	doc.SetCookie = c.String()
	return doc
}

package main

import (
	"net/url"

	"golang.org/x/net/publicsuffix"
)

// etldPlusOne is what publicsuffix.EffectiveTLDPlusOne made of one host.
type etldPlusOne struct {
	Host   string `json:"host"`
	Domain string `json:"domain"`
	Err    string `json:"err"`
}

// referrerDomain is what the auction endpoint makes of one referrer: url.Parse's Host, which
// becomes site.domain, and EffectiveTLDPlusOne of that Host, which becomes
// site.publisher.domain unless it fails (endpoints/openrtb2/auction.go:1607-1611).
type referrerDomain struct {
	Referrer string `json:"referrer"`
	etldPlusOne
}

type pslDoc struct {
	Header    header           `json:"header"`
	Hosts     []etldPlusOne    `json:"hosts"`
	Referrers []referrerDomain `json:"referrers"`
}

// eTLDPlusOne looks host up as the auction endpoint does.
func eTLDPlusOne(host string) etldPlusOne {
	e := etldPlusOne{Host: host}
	if d, err := publicsuffix.EffectiveTLDPlusOne(host); err != nil {
		e.Err = err.Error()
	} else {
		e.Domain = d
	}
	return e
}

// pslReferrers are referrers whose host has an empty label (a fully qualified name with its
// trailing dot, a leading dot, two dots in a row) or is an IP address, with and without a port.
var pslReferrers = []string{
	"http://example.com./article",
	"https://www.example.com./",
	"http://.example.com/",
	"http://example..com/",
	"http://a..b.example.co.uk/",
	"http://192.168.1.1/",
	"http://192.168.1.1:8080/",
	"http://[::1]:8080/",
}

// pslGolden looks up hosts as the auction endpoint derives site.publisher.domain from the
// referrer's url.Host, port included (endpoints/openrtb2/auction.go:1609).
func pslGolden() pslDoc {
	doc := pslDoc{Header: newHeader("golang.org/x/net")}
	for _, host := range []string{"WWW.EXAMPLE.CO.UK", "www.example.com:8080", "Example.COM", "www.Example.co.uk"} {
		doc.Hosts = append(doc.Hosts, eTLDPlusOne(host))
	}
	for _, referrer := range pslReferrers {
		u, err := url.Parse(referrer)
		if err != nil {
			panic(err)
		}
		doc.Referrers = append(doc.Referrers, referrerDomain{Referrer: referrer, etldPlusOne: eTLDPlusOne(u.Host)})
	}
	return doc
}

package main

import "golang.org/x/net/publicsuffix"

// etldPlusOne is what publicsuffix.EffectiveTLDPlusOne made of one host.
type etldPlusOne struct {
	Host   string `json:"host"`
	Domain string `json:"domain"`
	Err    string `json:"err"`
}

type pslDoc struct {
	Header header        `json:"header"`
	Hosts  []etldPlusOne `json:"hosts"`
}

// pslGolden looks up hosts as the auction endpoint derives site.publisher.domain from the
// referrer's url.Host, port included (endpoints/openrtb2/auction.go:1609).
func pslGolden() pslDoc {
	doc := pslDoc{Header: newHeader("golang.org/x/net")}
	for _, host := range []string{"WWW.EXAMPLE.CO.UK", "www.example.com:8080", "Example.COM", "www.Example.co.uk"} {
		e := etldPlusOne{Host: host}
		if d, err := publicsuffix.EffectiveTLDPlusOne(host); err != nil {
			e.Err = err.Error()
		} else {
			e.Domain = d
		}
		doc.Hosts = append(doc.Hosts, e)
	}
	return doc
}

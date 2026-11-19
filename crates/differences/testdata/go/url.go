package main

import "net/url"

// parsedURL is what url.Parse made of one URL.
type parsedURL struct {
	Input    string `json:"input"`
	Err      string `json:"err"`
	Host     string `json:"host"`
	Hostname string `json:"hostname"`
	Port     string `json:"port"`
	String   string `json:"string"`
}

type urlDoc struct {
	Header header      `json:"header"`
	URLs   []parsedURL `json:"urls"`
}

// urlGolden parses page URLs as the auction endpoint parses the referrer, whose Host becomes
// site.domain (endpoints/openrtb2/auction.go:1606-1608).
func urlGolden() urlDoc {
	doc := urlDoc{Header: newHeader()}
	for _, raw := range []string{
		"http://Example.COM:80/x",
		"http://bücher.example/",
		"https://Pub.Example:443/a?b=c",
		"http://example.com:8080/",
	} {
		p := parsedURL{Input: raw}
		u, err := url.Parse(raw)
		if err != nil {
			p.Err = err.Error()
		} else {
			p.Host = u.Host
			p.Hostname = u.Hostname()
			p.Port = u.Port()
			p.String = u.String()
		}
		doc.URLs = append(doc.URLs, p)
	}
	return doc
}

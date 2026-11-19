package main

import (
	"net/http"
	"net/url"
	"os"
	"sort"
	"strings"

	"golang.org/x/net/http/httpproxy"
)

// ProxyGolden is the proxy golden: the proxy httpproxy picks for each probe environment and URL,
// and what a loopback proxy receives from router.go's general Transport.
type ProxyGolden struct {
	Header header       `json:"header"`
	Probes []ProxyProbe `json:"probes"`
	Wire   []ProxyWire  `json:"wire"`
}

// ProxyWire is one request sent through a proxy that answers every request with Status: the
// head the proxy read (its port written as PROXY) and the client's error.
type ProxyWire struct {
	URL    string   `json:"url"`
	Status string   `json:"status"`
	Head   []string `json:"head"`
	Error  string   `json:"error"`
}

// ProxyProbe is one proxy environment: the Config httpproxy.FromEnvironment reads from it and
// what that Config's ProxyFunc returns for each probe URL. http.ProxyFromEnvironment is this
// function over the process environment (net/http vendors golang.org/x/net/http/httpproxy;
// the vendored copy in Go 1.26.2 differs from x/net v0.55.0 in comments only).
type ProxyProbe struct {
	Env     map[string]string `json:"env"`
	Config  ProxyConfig       `json:"config"`
	Results []ProxyResult     `json:"results"`
}

// ProxyConfig is an httpproxy.Config.
type ProxyConfig struct {
	HTTPProxy  string `json:"http_proxy"`
	HTTPSProxy string `json:"https_proxy"`
	NoProxy    string `json:"no_proxy"`
	CGI        bool   `json:"cgi"`
}

// ProxyResult is the proxy chosen for one URL: Proxy is the proxy URL's String() ("" for a
// direct connection) and Err the ProxyFunc error, or the url.Parse error prefixed by "parse: ".
type ProxyResult struct {
	URL   string `json:"url"`
	Proxy string `json:"proxy"`
	Err   string `json:"err"`
}

// proxyEnvNames are the variables httpproxy.FromEnvironment reads, and ALL_PROXY, which it does
// not read.
var proxyEnvNames = []string{
	"HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "NO_PROXY", "no_proxy", "REQUEST_METHOD",
	"ALL_PROXY", "all_proxy",
}

// proxyProbeEnvs are the probed environments: NO_PROXY rules under fixed proxies, then the
// variable precedence, proxy URL parsing and the CGI refusal.
func proxyProbeEnvs() []map[string]string {
	noProxy := []string{
		"", "*", "foo,*,bar", "example.com", ".example.com", "*.example.com", "example.com:8080",
		"sub.example.com:8080", "EXAMPLE.COM", " example.com , ,foo.bar.org ", "example.com.",
		"example.com:abc", "10.0.0.0/8", "1.2.3.4:8080,1.2.3.4:443", "[2001:db8::1]:443",
		"2001:db8::1", "2001:db8::/32,fe80::/10", "::ffff:10.0.0.0/104",
		// ParseCIDR masks the address: under a mask shorter than 96 bits the ::ffff: prefix
		// goes, and the network is an IPv6 one, which IPNet.Contains never matches an IPv4
		// address against.
		"::ffff:10.0.0.0/80", "::ffff:10.0.0.0/95", "::ffff:10.0.0.0/96", "127.0.0.1,localhost",
		"bücher.test", "BÜCHER.TEST", "\u00a0\u0130.test\u00a0", "xn--bcher-kva.test",
		":8080,[]:80,.,*.", "[fe80::1]", "fe80::1%en0",
	}
	var envs []map[string]string
	for _, np := range noProxy {
		envs = append(envs, map[string]string{
			"HTTP_PROXY":  "http://proxy.test:3128",
			"HTTPS_PROXY": "https://secure-proxy.test",
			"NO_PROXY":    np,
		})
	}
	return append(envs,
		map[string]string{"HTTP_PROXY": "http://upper.test:1", "http_proxy": "http://lower.test:2",
			"NO_PROXY": "", "no_proxy": "example.com"},
		map[string]string{"http_proxy": "lower.test:3128", "https_proxy": "secure.test:443"},
		map[string]string{"HTTP_PROXY": "socks5://user:pw@socks.test:1080", "HTTPS_PROXY": "socks5h://socks.test"},
		map[string]string{"HTTP_PROXY": "bogus url", "HTTPS_PROXY": ":3128"},
		map[string]string{"HTTP_PROXY": "http://[::1", "HTTPS_PROXY": "http://proxy.test:bad"},
		map[string]string{"HTTP_PROXY": "http://proxy.test:3128", "HTTPS_PROXY": "http://proxy.test:3128",
			"REQUEST_METHOD": "GET"},
		map[string]string{"HTTPS_PROXY": "https://u%40x:p%3Ass@secure.test:8443/path?q=1"},
		map[string]string{"HTTP_PROXY": "proxy.test", "HTTPS_PROXY": "//proxy.test:8080"},
		map[string]string{"ALL_PROXY": "http://all.test:1080", "all_proxy": "http://all.test:1080"},
		map[string]string{},
	)
}

// proxyProbeURLs are the request URLs every environment is probed with.
func proxyProbeURLs() []string {
	hosts := []string{
		"example.com", "EXAMPLE.COM", "sub.example.com", "example.com.", "notexample.com",
		"foo.bar.org", "localhost", "LocalHost", "127.0.0.1", "127.9.9.9", "[::1]",
		"[::1%25lo0]", "[fe80::1%25en0]", "[fe80::1]", "[::ffff:127.0.0.1]", "[::ffff:10.1.2.3]",
		"10.1.2.3", "1.2.3.4", "[2001:db8::1]", "[::a:b]", "bücher.test", "xn--bcher-kva.test",
		"i.test", "a.i.test",
	}
	var urls []string
	for _, h := range hosts {
		urls = append(urls, "http://"+h+"/", "http://"+h+":8080/p", "https://"+h+"/", "https://"+h+":443/")
	}
	return append(urls, "ftp://example.com/", "socks5://example.com/", "HTTP://example.com/",
		"http://[::1%25]/", "http://user:pw@example.com/")
}

// runProxyProbes evaluates every probe environment and restores the process environment.
func runProxyProbes() (probes []ProxyProbe, err error) {
	saved := map[string]string{}
	for _, k := range proxyEnvNames {
		if v, ok := os.LookupEnv(k); ok {
			saved[k] = v
		}
	}
	defer func() {
		if rerr := setProxyEnv(saved); rerr != nil && err == nil {
			err = rerr
		}
	}()
	for _, env := range proxyProbeEnvs() {
		if err := setProxyEnv(env); err != nil {
			return nil, err
		}
		cfg := httpproxy.FromEnvironment()
		proxyFor := cfg.ProxyFunc()
		probe := ProxyProbe{
			Env:    env,
			Config: ProxyConfig{HTTPProxy: cfg.HTTPProxy, HTTPSProxy: cfg.HTTPSProxy, NoProxy: cfg.NoProxy, CGI: cfg.CGI},
		}
		for _, raw := range proxyProbeURLs() {
			r := ProxyResult{URL: raw}
			u, err := url.Parse(raw)
			if err != nil {
				r.Err = "parse: " + err.Error()
			} else {
				proxy, err := proxyFor(u)
				r.Err = errText(err)
				if proxy != nil {
					r.Proxy = proxy.String()
				}
			}
			probe.Results = append(probe.Results, r)
		}
		probes = append(probes, probe)
	}
	return probes, nil
}

// setProxyEnv makes env the whole proxy environment.
func setProxyEnv(env map[string]string) error {
	for _, k := range proxyEnvNames {
		if err := os.Unsetenv(k); err != nil {
			return err
		}
	}
	keys := make([]string, 0, len(env))
	for k := range env {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, k := range keys {
		if err := os.Setenv(k, env[k]); err != nil {
			return err
		}
	}
	return nil
}

// proxyWire sends a GET for each URL through a loopback proxy that answers with status.
func proxyWire() []ProxyWire {
	var wires []ProxyWire
	for _, c := range []struct{ url, status string }{
		{"http://bidder.test/p?q=1", "200 OK"},
		{"https://bidder.test/p?q=1", "403 Forbidden"},
	} {
		heads := make(chan []string, 1)
		proxy := newRawServer(func(conn *rawConn) {
			req, ok := conn.readRequest()
			if !ok {
				panic("proxy: no request")
			}
			heads <- req.head
			conn.write("HTTP/1.1 " + c.status + "\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
		})
		proxyURL, err := url.Parse("http://" + proxy.addr())
		if err != nil {
			panic(err)
		}
		transport := generalTransport(nil)
		transport.Proxy = http.ProxyURL(proxyURL)
		req, err := http.NewRequest(http.MethodGet, c.url, nil)
		if err != nil {
			panic(err)
		}
		_, _, err = roundTrip(&http.Client{Transport: transport}, req)
		head := <-heads
		proxy.close()
		ports := map[string]string{proxy.port: "PROXY"}
		for i := range head {
			head[i] = withPorts(head[i], ports)
		}
		wires = append(wires, ProxyWire{URL: c.url, Status: c.status, Head: head,
			Error: strings.ReplaceAll(errText(err), proxy.port, "PROXY")})
	}
	return wires
}

func proxyGolden() ProxyGolden {
	probes, err := runProxyProbes()
	if err != nil {
		panic(err)
	}
	return ProxyGolden{Header: newHeader("golang.org/x/net"), Probes: probes, Wire: proxyWire()}
}

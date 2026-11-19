package main

import (
	"bufio"
	"io"
	"net"
	"net/http"
	"strconv"
	"strings"
)

// captureRequest serves one connection on a loopback listener and returns the head of the
// request send makes to the URL it is given: the request line and the header lines as they came
// over the wire, with the listener's port replaced by PORT.
func captureRequest(send func(url string) error) []string {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		panic(err)
	}
	defer ln.Close()
	port := strconv.Itoa(ln.Addr().(*net.TCPAddr).Port)
	heads := make(chan []string, 1)
	go func() {
		conn, err := ln.Accept()
		if err != nil {
			panic(err)
		}
		defer conn.Close()
		r := bufio.NewReader(conn)
		var lines []string
		length := 0
		for {
			line, err := r.ReadString('\n')
			if err != nil {
				panic(err)
			}
			line = strings.TrimRight(line, "\r\n")
			if line == "" {
				break
			}
			if name, value, ok := strings.Cut(line, ":"); ok && strings.EqualFold(name, "Content-Length") {
				if length, err = strconv.Atoi(strings.TrimSpace(value)); err != nil {
					panic(err)
				}
			}
			lines = append(lines, strings.ReplaceAll(line, ":"+port, ":PORT"))
		}
		if _, err := io.CopyN(io.Discard, r, int64(length)); err != nil {
			panic(err)
		}
		if _, err := conn.Write([]byte("HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")); err != nil {
			panic(err)
		}
		heads <- lines
	}()
	if err := send("http://127.0.0.1:" + port + "/headers"); err != nil {
		panic(err)
	}
	return <-heads
}

// prebidClient is an http.Client with a Transport like the ones router.go builds for the
// bidders and the cache (router/router.go:143-174); none of their settings changes the headers.
func prebidClient() *http.Client {
	return &http.Client{Transport: &http.Transport{Proxy: http.ProxyFromEnvironment}}
}

func roundTrip(req *http.Request) error {
	resp, err := prebidClient().Do(req)
	if err != nil {
		return err
	}
	if _, err := io.Copy(io.Discard, resp.Body); err != nil {
		return err
	}
	return resp.Body.Close()
}

type httpClientDoc struct {
	Header header `json:"header"`
	// Get is a GET with no header set, as the currency rates and vendor-list fetches send.
	Get []string `json:"get"`
	// Post is a bidder request: a JSON body with the Content-Type and Accept headers most
	// adapters set (adapters/appnexus/appnexus.go among them).
	Post     []string `json:"post"`
	PostBody string   `json:"post_body"`
}

func httpClientGolden() httpClientDoc {
	doc := httpClientDoc{Header: newHeader(), PostBody: `{"id":"r"}`}
	doc.Get = captureRequest(func(url string) error {
		req, err := http.NewRequest(http.MethodGet, url, nil)
		if err != nil {
			return err
		}
		return roundTrip(req)
	})
	doc.Post = captureRequest(func(url string) error {
		req, err := http.NewRequest(http.MethodPost, url, strings.NewReader(doc.PostBody))
		if err != nil {
			return err
		}
		req.Header.Set("Content-Type", "application/json;charset=utf-8")
		req.Header.Set("Accept", "application/json")
		return roundTrip(req)
	})
	return doc
}

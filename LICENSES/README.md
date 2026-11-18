# Third-party licenses

The port reimplements in Rust the behaviour of the Go code that Prebid Server runs: Prebid Server
itself, the parts of the Go standard library whose behaviour shows through it, and the Go modules
it depends on. [`NOTICE`](../NOTICE) credits every work the port derives from. This directory
holds the license texts those works require to be reproduced. Each `.txt` file is the work's
license file, unchanged, at the version the pinned Prebid Server uses. The files were read from
the Go module cache and from the Go 1.26.2 distribution.

The Apache-2.0 works share the license text in [`LICENSE`](../LICENSE), and `NOTICE` reproduces
the notices from their NOTICE files. The Rust crates the workspace depends on are not derived
works: each keeps its own license, and `cargo deny` checks those licenses against `deny.toml`.

| Work | Version | License | Copyright | Used by | Text |
| --- | --- | --- | --- | --- | --- |
| [Go](https://go.dev): the standard library | 1.26.2 | BSD-3-Clause | Copyright 2009 The Go Authors. | `pbs-gocompat`, `pbs-gojson`, `pbs-gogzip`, `pbs-gohttp-client`, `pbs-gohttp-server`, `pbs-util`, `pbs-bidder-params`, `pbs-logger` and others | [`go.txt`](go.txt) |
| Go: test data | 1.26.2 | BSD-3-Clause | Copyright 2009 The Go Authors. | `pbs-gogzip`: `testdata/flate/` holds `src/compress/flate/testdata`, `testdata/corpus/gettysburg.txt` is `src/compress/testdata/gettysburg.txt`, and `testdata/corpus/issue6550.gz` is `src/compress/gzip/testdata/issue6550.gz.base64`, decoded | [`go.txt`](go.txt) |
| [golang.org/x/net](https://pkg.go.dev/golang.org/x/net) | v0.55.0 | BSD-3-Clause | Copyright 2009 The Go Authors. | `pbs-gohttp-client` (`context/ctxhttp`, `http/httpguts`, `http/httpproxy`), `pbs-gohttp-server` (`http/httpguts`) | [`go.txt`](go.txt) (the same text) |
| [golang.org/x/text](https://pkg.go.dev/golang.org/x/text) | v0.37.0 | BSD-3-Clause | Copyright 2009 The Go Authors. | `pbs-currency` (`currency.ParseISO` and its ISO 4217 table, which x/text generates from CLDR 32) | [`go.txt`](go.txt) (the same text) |
| [github.com/chasex/glog](https://github.com/chasex/glog) | c62392af379c | ships no license file; its source says it is governed by Go's BSD-style license | Copyright 2009 The Go Authors. (its source header) | `pbs-logger` (`chasex`) | [`go.txt`](go.txt) |
| [github.com/golang/glog](https://github.com/golang/glog) | v1.2.5 | Apache-2.0 | Copyright 2023 Google Inc. All Rights Reserved. (its source headers) | `pbs-logger` (`glog`) | [`LICENSE`](../LICENSE) |
| [github.com/json-iterator/go](https://github.com/json-iterator/go) | v1.1.12 | MIT | Copyright (c) 2016 json-iterator | `pbs-gojson` (`jsoniter`) | [`json-iterator.txt`](json-iterator.txt) |
| [github.com/buger/jsonparser](https://github.com/buger/jsonparser) | v1.1.2 | MIT | Copyright (c) 2016 Leonid Bugaev | `pbs-gojson` (`jsonparser`) | [`buger-jsonparser.txt`](buger-jsonparser.txt) |
| [github.com/tidwall/gjson](https://github.com/tidwall/gjson) | v1.17.1 | MIT | Copyright (c) 2016 Josh Baker | `pbs-gojson` (`jsonutil`) | [`tidwall-gjson.txt`](tidwall-gjson.txt) |
| [gopkg.in/evanphx/json-patch.v5](https://github.com/evanphx/json-patch) | v5.9.0 | BSD-3-Clause | Copyright (c) 2014, Evan Phoenix | `pbs-gojson` (`mergepatch`) | [`evanphx-json-patch.txt`](evanphx-json-patch.txt) |
| [gopkg.in/yaml.v3](https://github.com/go-yaml/yaml) | v3.0.1 | MIT (the files ported from libyaml) and Apache-2.0 (the rest) | Copyright (c) 2006-2010 Kirill Simonov, Copyright (c) 2006-2011 Kirill Simonov; Copyright (c) 2011-2019 Canonical Ltd | `pbs-goyaml` (`yaml`) | [`go-yaml-v3.txt`](go-yaml-v3.txt), [`LICENSE`](../LICENSE) |
| [github.com/mitchellh/mapstructure](https://github.com/mitchellh/mapstructure) | v1.5.0 | MIT | Copyright (c) 2013 Mitchell Hashimoto | `pbs-goyaml` (`mapstructure`) | [`mitchellh-mapstructure.txt`](mitchellh-mapstructure.txt) |
| [github.com/spf13/viper](https://github.com/spf13/viper) | v1.12.0 | MIT | Copyright (c) 2014 Steve Francia | `pbs-goviper` | [`spf13-viper.txt`](spf13-viper.txt) |
| [github.com/spf13/cast](https://github.com/spf13/cast) | v1.5.0 | MIT | Copyright (c) 2014 Steve Francia | `pbs-goviper` (`cast`) | [`spf13-cast.txt`](spf13-cast.txt) |
| [github.com/coocood/freecache](https://github.com/coocood/freecache) | v1.2.1 | MIT | Copyright (c) 2015 Ewan Chou. | `pbs-freecache` | [`coocood-freecache.txt`](coocood-freecache.txt) |
| [github.com/cespare/xxhash/v2](https://github.com/cespare/xxhash) | v2.3.0 | MIT | Copyright (c) 2016 Caleb Spare | `pbs-freecache` (its XXH64) | [`cespare-xxhash.txt`](cespare-xxhash.txt) |
| [github.com/julienschmidt/httprouter](https://github.com/julienschmidt/httprouter) | v1.3.0 | BSD-3-Clause | Copyright (c) 2013, Julien Schmidt | `pbs-gohttp-server` (`httprouter`) | [`julienschmidt-httprouter.txt`](julienschmidt-httprouter.txt) |
| [github.com/rs/cors](https://github.com/rs/cors) | v1.11.0 | MIT | Copyright (c) 2014 Olivier Poitrey <rs@dailymotion.com> | `pbs-gohttp-server` (`cors`) | [`rs-cors.txt`](rs-cors.txt) |
| [github.com/gofrs/uuid](https://github.com/gofrs/uuid) | v4.2.0+incompatible | MIT | Copyright (C) 2013-2018 by Maxim Bublis <b@codemonkey.ru> | `pbs-util` (`uuidutil`: `NewV4().String()`) | [`gofrs-uuid.txt`](gofrs-uuid.txt) |
| [github.com/prebid/openrtb/v20](https://github.com/prebid/openrtb) | v20.3.0 | Apache-2.0 | Copyright 2017 PREBID.ORG, INC | `pbs-openrtb`: the generated types keep its documentation comments, which follow the IAB Tech Lab OpenRTB, AdCOM and Native specifications | [`LICENSE`](../LICENSE) |
| [github.com/prebid/go-gdpr](https://github.com/prebid/go-gdpr) | v1.12.0 | Apache-2.0 | Copyright 2017 PREBID.ORG, INC | `pbs-tcf2` | [`LICENSE`](../LICENSE) |
| [github.com/prebid/go-gpp](https://github.com/prebid/go-gpp) | v0.2.0 | Apache-2.0 | none in its license or sources | `pbs-gpp` | [`LICENSE`](../LICENSE) |
| [github.com/xeipuuv/gojsonschema](https://github.com/xeipuuv/gojsonschema) | v1.2.0 | Apache-2.0 | Copyright 2015 xeipuuv, Copyright 2017 and 2018 johandorland; `jsonContext.go`: Copyright 2013 MongoDB, Inc. | `pbs-bidder-params` (`gojsonschema`) | [`LICENSE`](../LICENSE) |
| [github.com/xeipuuv/gojsonreference](https://github.com/xeipuuv/gojsonreference) | bd5ef7bd5415 | Apache-2.0 | Copyright 2015 xeipuuv | `pbs-bidder-params` (`gojsonschema`) | [`LICENSE`](../LICENSE) |
| [github.com/prometheus/client_golang](https://github.com/prometheus/client_golang) | v1.12.1 | Apache-2.0 | Copyright 2012-2015 The Prometheus Authors | `pbs-metrics` (`prometheus`) | [`LICENSE`](../LICENSE), and its notice in `NOTICE` |
| [github.com/prometheus/common](https://github.com/prometheus/common) | v0.32.1 | Apache-2.0 | Copyright 2015 The Prometheus Authors | `pbs-metrics` (`prometheus::expfmt`, metric name rules) | [`LICENSE`](../LICENSE), and its notice in `NOTICE` |
| [github.com/prometheus/client_model](https://github.com/prometheus/client_model) | v0.2.0 | Apache-2.0 | Copyright 2012-2015 The Prometheus Authors | `pbs-metrics` (`prometheus::registry`) | [`LICENSE`](../LICENSE), and its notice in `NOTICE` |

The guard (`.github/port/scripts/guard.sh`) allows only `README.md` and `<name>.txt` license texts
here, and none of them may be a copy of an upstream file.

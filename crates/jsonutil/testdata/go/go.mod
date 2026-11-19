module github.com/aryehlev/prebid-server-rust/crates/jsonutil/testdata/go

go 1.25.0

require (
	github.com/buger/jsonparser v1.1.2
	github.com/json-iterator/go v1.1.12
	github.com/prebid/openrtb/v20 v20.3.0
	github.com/prebid/prebid-server/v4 v4.11.2-0.20261112141353-4e296450de2f
	github.com/tidwall/sjson v1.2.5
	gopkg.in/evanphx/json-patch.v5 v5.9.0
)

require (
	github.com/modern-go/concurrent v0.0.0-20180306012644-bacd9c7ef1dd // indirect
	github.com/modern-go/reflect2 v1.0.2 // indirect
	github.com/pkg/errors v0.9.1 // indirect
	github.com/tidwall/gjson v1.17.1 // indirect
	github.com/tidwall/match v1.1.1 // indirect
	github.com/tidwall/pretty v1.2.0 // indirect
)

replace github.com/prebid/prebid-server/v4 => ../../../../.upstream

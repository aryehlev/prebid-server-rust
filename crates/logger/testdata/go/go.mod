module github.com/aryehlev/prebid-server-rust/crates/logger/testdata/go

go 1.25.0

require (
	github.com/golang/glog v1.2.5
	github.com/prebid/prebid-server/v4 v4.11.2-0.20261112141353-4e296450de2f
)

replace github.com/prebid/prebid-server/v4 => ../../../../.upstream

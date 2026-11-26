module github.com/aryehlev/prebid-server-rust/crates/util/testdata/go

go 1.25.0

require (
	github.com/benbjohnson/clock v1.3.0
	github.com/gofrs/uuid v4.2.0+incompatible
	github.com/prebid/prebid-server/v4 v4.11.2-0.20261112141353-4e296450de2f
)

replace github.com/prebid/prebid-server/v4 => ../../../../.upstream

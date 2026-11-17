# Parity stored data

The runner copies this directory into every server working directory as
`stored/` and points all four filesystem backends at it:
`stored_requests.filesystem`, `stored_video_req.filesystem`,
`stored_responses.filesystem` and `accounts.filesystem`. Both servers of a pair
read the same files. The layout is the Go file fetcher's layout
(`stored_requests/backends/file_fetcher`), with one `<id>.json` file per ID:

| Directory | Used by |
|---|---|
| `stored_requests/` | `ext.prebid.storedrequest.id` on the request, AMP `tag_id`, video `storedrequestid` |
| `stored_imps/` | `imp.ext.prebid.storedrequest.id` |
| `stored_responses/` | `ext.prebid.storedauctionresponse` and `storedbidresponse` |
| `accounts/` | account resolution (`account_required`, account defaults merged over `account_defaults`) |

Every ID here starts with `parity-`, so it can never collide with the stored
data a corpus case brings along (`corpus.Case.Stored`). The runner adds each
case's stored data to the same tree, and splits cases into separate server
batches when two of them use one ID with different content.

| ID | What it is |
|---|---|
| `accounts/parity-account` | an enabled account with no settings |
| `accounts/parity-disabled-account` | a disabled account (`BlockedApp`/account-disabled paths) |
| `stored_requests/parity-stored-request` | a site request for `parity-account` with price-granularity targeting |
| `stored_imps/parity-stored-imp` | a 300x250 banner imp bidding to appnexus |
| `stored_responses/parity-stored-response` | one appnexus seatbid for `parity-stored-imp` |

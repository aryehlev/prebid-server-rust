# Parity coverage matrix

Run `selftest-snapshot`: implementation A `go` against implementation B `go`, finished 2026-11-17T00:53:11Z. Source: `parity/out/selftest-snapshot/index.json`.

8394 cases: 0 passing, 0 mismatch, 8390 not-yet-instrumented, 0 error, 4 skipped, 0 not compared.

## Domains

| Domain | Status | Exercised | Mismatching | Summary | Evidence |
|---|---|---|---|---|---|
| Endpoints (`endpoints`) | not-yet-instrumented | 0 | 0 | No case was compared: 8390 cases were not run because an implementation does not serve the parity contract yet. | none |
| Config and bootstrap (`config_bootstrap`) | not-yet-instrumented | 0 | 0 | No case was compared: 8390 cases were not run because an implementation does not serve the parity contract yet. | none |
| Stored data (`stored_data`) | not-yet-instrumented | 0 | 0 | No case was compared: 8390 cases were not run because an implementation does not serve the parity contract yet. | none |
| Auction lifecycle (`auction_lifecycle`) | not-yet-instrumented | 0 | 0 | No case was compared: 8390 cases were not run because an implementation does not serve the parity contract yet. | none |
| Policy, privacy and identity (`policy_privacy_identity`) | not-yet-instrumented | 0 | 0 | No case was compared: 8390 cases were not run because an implementation does not serve the parity contract yet. | none |
| Bidders and adapters (`bidders_adapters`) | not-yet-instrumented | 0 | 0 | No case was compared: 8390 cases were not run because an implementation does not serve the parity contract yet. | none |
| Hooks and modules (`hooks_modules`) | not-yet-instrumented | 0 | 0 | No case was compared: 8390 cases were not run because an implementation does not serve the parity contract yet. | none |
| Cache and support flows (`cache_support_flows`) | not-yet-instrumented | 0 | 0 | No case was compared: 8390 cases were not run because an implementation does not serve the parity contract yet. | none |
| Cutover proof (`cutover_proof`) | not-yet-instrumented | 0 | 0 | No case was compared: 8390 cases were not run because an implementation does not serve the parity contract yet. | none |

## Endpoints

| Endpoint | Status | Cases | Passing | Mismatch | Not yet instrumented | Error | Skipped | Not compared |
|---|---|---|---|---|---|---|---|---|
| `/__parity/adapter/{bidder}/requests` | not-yet-instrumented | 3696 | 0 | 0 | 3696 | 0 | 0 | 0 |
| `/__parity/config` | not-yet-instrumented | 42 | 0 | 0 | 42 | 0 | 0 | 0 |
| `/bidders/params` | not-yet-instrumented | 1 | 0 | 0 | 1 | 0 | 0 | 0 |
| `/openrtb2/amp` | not-yet-instrumented | 20 | 0 | 0 | 20 | 0 | 0 | 0 |
| `/openrtb2/auction` | not-yet-instrumented | 4626 | 0 | 0 | 4622 | 0 | 4 | 0 |
| `/openrtb2/video` | not-yet-instrumented | 9 | 0 | 0 | 9 | 0 | 0 | 0 |

## Bidders

Pass rates are passing over compared cases. Fixture self-test columns count implementation A against the adapter fixtures' own expectations.

| Bidder | Status | Adapter fixtures | Adapter pass % | Lifted auctions | Lifted pass % | needs_fake | Fixture self-test (pass / fail / excused) |
|---|---|---|---|---|---|---|---|
| `33across` | not-yet-instrumented | 14 | n/a | 14 | n/a | 0 | 0 / 0 / 0 |
| `aax` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `aceex` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `acuityads` | not-yet-instrumented | 14 | n/a | 14 | n/a | 0 | 0 / 0 / 0 |
| `adagio` | not-yet-instrumented | 19 | n/a | 19 | n/a | 0 | 0 / 0 / 0 |
| `adelement` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `adf` | not-yet-instrumented | 19 | n/a | 19 | n/a | 0 | 0 / 0 / 0 |
| `adgeneration` | not-yet-instrumented | 7 | n/a | 7 | n/a | 0 | 0 / 0 / 0 |
| `adhese` | not-yet-instrumented | 6 | n/a | 6 | n/a | 0 | 0 / 0 / 0 |
| `adkernel` | not-yet-instrumented | 3 | n/a | 3 | n/a | 0 | 0 / 0 / 0 |
| `adkernelAdn` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `adman` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `admatic` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `admixer` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `adnuntius` | not-yet-instrumented | 33 | n/a | 33 | n/a | 33 | 0 / 0 / 0 |
| `adot` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `adpone` | not-yet-instrumented | 5 | n/a | 5 | n/a | 0 | 0 / 0 / 0 |
| `adprime` | not-yet-instrumented | 14 | n/a | 14 | n/a | 0 | 0 / 0 / 0 |
| `adquery` | not-yet-instrumented | 18 | n/a | 18 | n/a | 0 | 0 / 0 / 0 |
| `adrino` | not-yet-instrumented | 3 | n/a | 3 | n/a | 0 | 0 / 0 / 0 |
| `adtarget` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `adtelligent` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `adtonos` | not-yet-instrumented | 4 | n/a | 4 | n/a | 0 | 0 / 0 / 0 |
| `adtrgtme` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `aduptech` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `advangelists` | not-yet-instrumented | 2 | n/a | 2 | n/a | 0 | 0 / 0 / 0 |
| `adverxo` | not-yet-instrumented | 14 | n/a | 14 | n/a | 0 | 0 / 0 / 0 |
| `adview` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `adxcg` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `adyoulike` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `afront` | not-yet-instrumented | 14 | n/a | 14 | n/a | 0 | 0 / 0 / 0 |
| `aidem` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `aja` | not-yet-instrumented | 20 | n/a | 19 | n/a | 0 | 0 / 0 / 0 |
| `akcelo` | not-yet-instrumented | 0 | n/a | 1 | n/a | 0 | 0 / 0 / 0 |
| `algorix` | not-yet-instrumented | 19 | n/a | 19 | n/a | 0 | 0 / 0 / 0 |
| `alkimi` | not-yet-instrumented | 7 | n/a | 7 | n/a | 0 | 0 / 0 / 0 |
| `alliance_gravity` | not-yet-instrumented | 16 | n/a | 16 | n/a | 0 | 0 / 0 / 0 |
| `amx` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `apacdex` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `appnexus` | not-yet-instrumented | 34 | n/a | 34 | n/a | 34 | 0 / 0 / 0 |
| `appush` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `aps` | not-yet-instrumented | 16 | n/a | 16 | n/a | 0 | 0 / 0 / 0 |
| `aso` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `audienceNetwork` | not-yet-instrumented | 35 | n/a | 35 | n/a | 0 | 0 / 0 / 0 |
| `automatad` | not-yet-instrumented | 4 | n/a | 4 | n/a | 0 | 0 / 0 / 0 |
| `avocet` | not-yet-instrumented | 2 | n/a | 2 | n/a | 0 | 0 / 0 / 0 |
| `axis` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `axonix` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `beachfront` | not-yet-instrumented | 44 | n/a | 44 | n/a | 0 | 0 / 0 / 0 |
| `beintoo` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `bematterfull` | not-yet-instrumented | 0 | n/a | 2 | n/a | 0 | 0 / 0 / 0 |
| `beop` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `between` | not-yet-instrumented | 14 | n/a | 14 | n/a | 0 | 0 / 0 / 0 |
| `beyondmedia` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `bidmachine` | not-yet-instrumented | 19 | n/a | 19 | n/a | 0 | 0 / 0 / 0 |
| `bidmatic` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `bidmyadz` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `bidscube` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `bidstack` | not-yet-instrumented | 5 | n/a | 5 | n/a | 0 | 0 / 0 / 0 |
| `bidtheatre` | not-yet-instrumented | 7 | n/a | 7 | n/a | 0 | 0 / 0 / 0 |
| `bigoad` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `blasto` | not-yet-instrumented | 14 | n/a | 14 | n/a | 0 | 0 / 0 / 0 |
| `bliink` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `blis` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `bluesea` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `bmtm` | not-yet-instrumented | 7 | n/a | 7 | n/a | 0 | 0 / 0 / 0 |
| `boldwin` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `boldwin_rapid` | not-yet-instrumented | 7 | n/a | 7 | n/a | 0 | 0 / 0 / 0 |
| `brave` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `bwx` | not-yet-instrumented | 14 | n/a | 14 | n/a | 0 | 0 / 0 / 0 |
| `cadent_aperture_mx` | not-yet-instrumented | 25 | n/a | 25 | n/a | 25 | 0 / 0 / 0 |
| `ccx` | not-yet-instrumented | 5 | n/a | 5 | n/a | 0 | 0 / 0 / 0 |
| `clydo` | not-yet-instrumented | 21 | n/a | 21 | n/a | 0 | 0 / 0 / 0 |
| `cointraffic` | not-yet-instrumented | 4 | n/a | 4 | n/a | 0 | 0 / 0 / 0 |
| `coinzilla` | not-yet-instrumented | 5 | n/a | 5 | n/a | 0 | 0 / 0 / 0 |
| `colossus` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `compass` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `concert` | not-yet-instrumented | 3 | n/a | 3 | n/a | 0 | 0 / 0 / 0 |
| `connatix` | not-yet-instrumented | 19 | n/a | 19 | n/a | 0 | 0 / 0 / 0 |
| `connectad` | not-yet-instrumented | 20 | n/a | 20 | n/a | 0 | 0 / 0 / 0 |
| `consumable` | not-yet-instrumented | 22 | n/a | 22 | n/a | 0 | 0 / 0 / 0 |
| `contxtful` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `conversant` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `copper6ssp` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `cpmstar` | not-yet-instrumented | 16 | n/a | 16 | n/a | 0 | 0 / 0 / 0 |
| `criteo` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `cwire` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `datablocks` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `decenterads` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `deepintent` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `definemedia` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `dianomi` | not-yet-instrumented | 16 | n/a | 16 | n/a | 0 | 0 / 0 / 0 |
| `displayio` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `dmx` | not-yet-instrumented | 6 | n/a | 6 | n/a | 0 | 0 / 0 / 0 |
| `driftpixel` | not-yet-instrumented | 16 | n/a | 16 | n/a | 0 | 0 / 0 / 0 |
| `e_volution` | not-yet-instrumented | 18 | n/a | 18 | n/a | 0 | 0 / 0 / 0 |
| `edge226` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `elementaltv` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `emtv` | not-yet-instrumented | 56 | n/a | 52 | n/a | 0 | 0 / 0 / 0 |
| `eplanning` | not-yet-instrumented | 37 | n/a | 37 | n/a | 37 | 0 / 0 / 0 |
| `epom` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `escalax` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `eskimi` | not-yet-instrumented | 29 | n/a | 29 | n/a | 0 | 0 / 0 / 0 |
| `exco` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `ezoic` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `feedad` | not-yet-instrumented | 7 | n/a | 7 | n/a | 0 | 0 / 0 / 0 |
| `ferio` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `flatads` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `flipp` | not-yet-instrumented | 10 | n/a | 10 | n/a | 10 | 0 / 0 / 0 |
| `floxis` | not-yet-instrumented | 23 | n/a | 23 | n/a | 0 | 0 / 0 / 0 |
| `freewheelssp` | not-yet-instrumented | 6 | n/a | 6 | n/a | 0 | 0 / 0 / 0 |
| `frvradn` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `fwssp` | not-yet-instrumented | 6 | n/a | 6 | n/a | 0 | 0 / 0 / 0 |
| `gamma` | not-yet-instrumented | 19 | n/a | 19 | n/a | 0 | 0 / 0 / 0 |
| `gamoshi` | not-yet-instrumented | 44 | n/a | 22 | n/a | 0 | 0 / 0 / 0 |
| `globalsun` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `goldbach` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `grid` | not-yet-instrumented | 22 | n/a | 22 | n/a | 0 | 0 / 0 / 0 |
| `gumgum` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `huaweiads` | not-yet-instrumented | 58 | n/a | 58 | n/a | 58 | 0 / 0 / 0 |
| `imds` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `impactify` | not-yet-instrumented | 16 | n/a | 16 | n/a | 0 | 0 / 0 / 0 |
| `improvedigital` | not-yet-instrumented | 19 | n/a | 19 | n/a | 0 | 0 / 0 / 0 |
| `inmobi` | not-yet-instrumented | 18 | n/a | 18 | n/a | 0 | 0 / 0 / 0 |
| `insticator` | not-yet-instrumented | 18 | n/a | 18 | n/a | 0 | 0 / 0 / 0 |
| `intenze` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `interactiveoffers` | not-yet-instrumented | 6 | n/a | 6 | n/a | 0 | 0 / 0 / 0 |
| `invibes` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `iqx` | not-yet-instrumented | 14 | n/a | 14 | n/a | 0 | 0 / 0 / 0 |
| `iqzone` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `ix` | not-yet-instrumented | 36 | n/a | 36 | n/a | 0 | 0 / 0 / 0 |
| `jixie` | not-yet-instrumented | 4 | n/a | 4 | n/a | 0 | 0 / 0 / 0 |
| `kargo` | not-yet-instrumented | 17 | n/a | 16 | n/a | 0 | 0 / 0 / 0 |
| `kayzen` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `kidoz` | not-yet-instrumented | 19 | n/a | 19 | n/a | 0 | 0 / 0 / 0 |
| `kiviads` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `kobler` | not-yet-instrumented | 0 | n/a | 1 | n/a | 0 | 0 / 0 / 0 |
| `krushmedia` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `kueezrtb` | not-yet-instrumented | 7 | n/a | 7 | n/a | 0 | 0 / 0 / 0 |
| `lemmadigital` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `limelightDigital` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `lm_kiviads` | not-yet-instrumented | 0 | n/a | 2 | n/a | 0 | 0 / 0 / 0 |
| `lockerdome` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `logan` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `logicad` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `loopme` | not-yet-instrumented | 18 | n/a | 18 | n/a | 0 | 0 / 0 / 0 |
| `loyal` | not-yet-instrumented | 0 | n/a | 1 | n/a | 0 | 0 / 0 / 0 |
| `lunamedia` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `mabidder` | not-yet-instrumented | 5 | n/a | 5 | n/a | 0 | 0 / 0 / 0 |
| `madsense` | not-yet-instrumented | 14 | n/a | 14 | n/a | 0 | 0 / 0 / 0 |
| `madvertise` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `marsmedia` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `matterfullrtb` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `mediago` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `medianet` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `mediasquare` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `melozen` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `metax` | not-yet-instrumented | 22 | n/a | 22 | n/a | 0 | 0 / 0 / 0 |
| `mgid` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `mgidX` | not-yet-instrumented | 0 | n/a | 1 | n/a | 0 | 0 / 0 / 0 |
| `minutemedia` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `missena` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `mobfoxpb` | not-yet-instrumented | 14 | n/a | 14 | n/a | 0 | 0 / 0 / 0 |
| `mobilefuse` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `mobkoi` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `motorik` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `msft` | not-yet-instrumented | 31 | n/a | 31 | n/a | 0 | 0 / 0 / 0 |
| `nativery` | not-yet-instrumented | 22 | n/a | 22 | n/a | 0 | 0 / 0 / 0 |
| `nativo` | not-yet-instrumented | 18 | n/a | 18 | n/a | 0 | 0 / 0 / 0 |
| `nextmillennium` | not-yet-instrumented | 16 | n/a | 16 | n/a | 0 | 0 / 0 / 0 |
| `nexx360` | not-yet-instrumented | 19 | n/a | 19 | n/a | 0 | 0 / 0 / 0 |
| `nobid` | not-yet-instrumented | 7 | n/a | 7 | n/a | 0 | 0 / 0 / 0 |
| `odeeo` | not-yet-instrumented | 27 | n/a | 27 | n/a | 0 | 0 / 0 / 0 |
| `ogury` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `oms` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `onetag` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `openweb` | not-yet-instrumented | 5 | n/a | 5 | n/a | 0 | 0 / 0 / 0 |
| `openx` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `operaads` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `optidigital` | not-yet-instrumented | 5 | n/a | 5 | n/a | 0 | 0 / 0 / 0 |
| `oraki` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `orbidder` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `outbrain` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `ownadx` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `pangle` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `panxo` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `peak226` | not-yet-instrumented | 23 | n/a | 23 | n/a | 0 | 0 / 0 / 0 |
| `pgamssp` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `pixfuture` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `playdigo` | not-yet-instrumented | 0 | n/a | 1 | n/a | 0 | 0 / 0 / 0 |
| `proxistore` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `pubmatic` | not-yet-instrumented | 23 | n/a | 23 | n/a | 0 | 0 / 0 / 0 |
| `pubnative` | not-yet-instrumented | 7 | n/a | 7 | n/a | 0 | 0 / 0 / 0 |
| `pubrise` | not-yet-instrumented | 0 | n/a | 1 | n/a | 0 | 0 / 0 / 0 |
| `pulsepoint` | not-yet-instrumented | 16 | n/a | 16 | n/a | 0 | 0 / 0 / 0 |
| `pwbid` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `qt` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `readpeak` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `rediads` | not-yet-instrumented | 17 | n/a | 17 | n/a | 0 | 0 / 0 / 0 |
| `relevantdigital` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `resetdigital` | not-yet-instrumented | 18 | n/a | 18 | n/a | 0 | 0 / 0 / 0 |
| `revcontent` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `richaudience` | not-yet-instrumented | 50 | n/a | 25 | n/a | 0 | 0 / 0 / 0 |
| `rise` | not-yet-instrumented | 18 | n/a | 18 | n/a | 0 | 0 / 0 / 0 |
| `roulax` | not-yet-instrumented | 4 | n/a | 4 | n/a | 0 | 0 / 0 / 0 |
| `rtbhouse` | not-yet-instrumented | 34 | n/a | 34 | n/a | 0 | 0 / 0 / 0 |
| `rtbstack` | not-yet-instrumented | 28 | n/a | 28 | n/a | 0 | 0 / 0 / 0 |
| `rubicon` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `sa_lunamedia` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `scalibur` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `seedingAlliance` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `seedtag` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `sharethrough` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `showheroes` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `silvermob` | not-yet-instrumented | 14 | n/a | 14 | n/a | 0 | 0 / 0 / 0 |
| `silverpush` | not-yet-instrumented | 25 | n/a | 25 | n/a | 0 | 0 / 0 / 0 |
| `smaato` | not-yet-instrumented | 41 | n/a | 41 | n/a | 41 | 0 / 0 / 0 |
| `smartadserver` | not-yet-instrumented | 31 | n/a | 29 | n/a | 0 | 0 / 0 / 0 |
| `smarthub` | not-yet-instrumented | 55 | n/a | 49 | n/a | 0 | 0 / 0 / 0 |
| `smartrtb` | not-yet-instrumented | 14 | n/a | 14 | n/a | 0 | 0 / 0 / 0 |
| `smartyads` | not-yet-instrumented | 0 | n/a | 2 | n/a | 0 | 0 / 0 / 0 |
| `smilewanted` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `smoot` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `smrtconnect` | not-yet-instrumented | 16 | n/a | 16 | n/a | 0 | 0 / 0 / 0 |
| `sonobi` | not-yet-instrumented | 4 | n/a | 4 | n/a | 0 | 0 / 0 / 0 |
| `sovrn` | not-yet-instrumented | 31 | n/a | 31 | n/a | 0 | 0 / 0 / 0 |
| `sovrnXsp` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `sparteo` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `sspBC` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `stackadapt` | not-yet-instrumented | 16 | n/a | 16 | n/a | 0 | 0 / 0 / 0 |
| `startio` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `stroeerCore` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `superedge` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `synapseHX` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `taboola` | not-yet-instrumented | 17 | n/a | 17 | n/a | 0 | 0 / 0 / 0 |
| `tadvertising_blis` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `tappx` | not-yet-instrumented | 15 | n/a | 15 | n/a | 15 | 0 / 0 / 0 |
| `targetVideo` | not-yet-instrumented | 8 | n/a | 8 | n/a | 0 | 0 / 0 / 0 |
| `teads` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `teal` | not-yet-instrumented | 23 | n/a | 23 | n/a | 0 | 0 / 0 / 0 |
| `telaria` | not-yet-instrumented | 14 | n/a | 14 | n/a | 0 | 0 / 0 / 0 |
| `teqblaze` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `theadx` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `thetradedesk` | not-yet-instrumented | 21 | n/a | 21 | n/a | 0 | 0 / 0 / 0 |
| `tpmn` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `tradplus` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `trafficgate` | not-yet-instrumented | 16 | n/a | 16 | n/a | 0 | 0 / 0 / 0 |
| `triplelift` | not-yet-instrumented | 16 | n/a | 16 | n/a | 0 | 0 / 0 / 0 |
| `triplelift_native` | not-yet-instrumented | 18 | n/a | 18 | n/a | 0 | 0 / 0 / 0 |
| `trustedstack` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `trustx` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `tunnl` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `undertone` | not-yet-instrumented | 5 | n/a | 5 | n/a | 0 | 0 / 0 / 0 |
| `unicorn` | not-yet-instrumented | 23 | n/a | 23 | n/a | 0 | 0 / 0 / 0 |
| `unruly` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `viant` | not-yet-instrumented | 24 | n/a | 24 | n/a | 0 | 0 / 0 / 0 |
| `vidazoo` | not-yet-instrumented | 7 | n/a | 7 | n/a | 0 | 0 / 0 / 0 |
| `videobyte` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `videoheroes` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `vidoomy` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `visiblemeasures` | not-yet-instrumented | 9 | n/a | 9 | n/a | 0 | 0 / 0 / 0 |
| `visx` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `vox` | not-yet-instrumented | 4 | n/a | 4 | n/a | 0 | 0 / 0 / 0 |
| `vrtcal` | not-yet-instrumented | 7 | n/a | 7 | n/a | 0 | 0 / 0 / 0 |
| `vungle` | not-yet-instrumented | 13 | n/a | 13 | n/a | 0 | 0 / 0 / 0 |
| `xeworks` | not-yet-instrumented | 0 | n/a | 2 | n/a | 0 | 0 / 0 / 0 |
| `yahooAds` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `yandex` | not-yet-instrumented | 15 | n/a | 15 | n/a | 0 | 0 / 0 / 0 |
| `yeahmobi` | not-yet-instrumented | 10 | n/a | 10 | n/a | 0 | 0 / 0 / 0 |
| `yieldlab` | not-yet-instrumented | 12 | n/a | 12 | n/a | 12 | 0 / 0 / 0 |
| `yieldmo` | not-yet-instrumented | 7 | n/a | 7 | n/a | 0 | 0 / 0 / 0 |
| `yieldone` | not-yet-instrumented | 6 | n/a | 6 | n/a | 0 | 0 / 0 / 0 |
| `zentotem` | not-yet-instrumented | 6 | n/a | 6 | n/a | 0 | 0 / 0 / 0 |
| `zeroclickfraud` | not-yet-instrumented | 12 | n/a | 12 | n/a | 0 | 0 / 0 / 0 |
| `zeta_global_ssp` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |
| `zmaticoo` | not-yet-instrumented | 11 | n/a | 11 | n/a | 0 | 0 / 0 / 0 |

## Stages

Counts are over compared cases. Implementation B reports `unimplemented` for stages it has no capture point for yet.

| # | Stage | Domain | Status (B) | A captured | B captured | B unimplemented | Mismatching cases | First divergence |
|---|---|---|---|---|---|---|---|---|
| 1 | `config.resolved` | config_bootstrap | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 2 | `bidders.resolved` | config_bootstrap | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 3 | `http.in` | endpoints | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 4 | `hooks.entrypoint` | hooks_modules | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 5 | `stored.fetch` | stored_data | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 6 | `account.resolved` | stored_data | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 7 | `hooks.raw_auction` | hooks_modules | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 8 | `request.merged` | stored_data | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 9 | `request.normalized` | auction_lifecycle | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 10 | `stored.responses` | cache_support_flows | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 11 | `request.validated` | auction_lifecycle | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 12 | `gdpr.context` | policy_privacy_identity | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 13 | `hooks.processed_auction` | hooks_modules | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 14 | `auction.setup` | auction_lifecycle | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 15 | `floors.enrich` | policy_privacy_identity | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 16 | `fpd.resolved` | policy_privacy_identity | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 17 | `privacy.labels` | policy_privacy_identity | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 18 | `privacy.bidder` | policy_privacy_identity | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 19 | `bidder.request` | bidders_adapters | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 20 | `hooks.bidder_request` | hooks_modules | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 21 | `adapter.requests` | bidders_adapters | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 22 | `adapter.requests_final` | bidders_adapters | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 23 | `http.out` | bidders_adapters | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 24 | `adapter.bids` | bidders_adapters | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 25 | `hooks.raw_bidder_response` | hooks_modules | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 26 | `bids.adjusted` | policy_privacy_identity | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 27 | `bidder.result` | auction_lifecycle | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 28 | `floors.enforce` | policy_privacy_identity | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 29 | `category.mapping` | policy_privacy_identity | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 30 | `events.tracking` | cache_support_flows | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 31 | `hooks.all_processed_bid_responses` | hooks_modules | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 32 | `auction.winners` | policy_privacy_identity | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 33 | `cache.result` | cache_support_flows | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 34 | `targeting` | policy_privacy_identity | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 35 | `seat_non_bids` | auction_lifecycle | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 36 | `exchange.response` | auction_lifecycle | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 37 | `video.pods` | endpoints | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 38 | `video.cache` | cache_support_flows | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 39 | `amp.targeting_response` | endpoints | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 40 | `hooks.auction_response` | hooks_modules | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 41 | `hooks.exitpoint` | hooks_modules | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 42 | `hook_outcomes` | hooks_modules | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 43 | `analytics` | cutover_proof | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 44 | `metrics_delta` | cutover_proof | not-exercised | 0 | 0 | 0 | 0 | 0 |
| 45 | `http.response` | endpoints | not-exercised | 0 | 0 | 0 | 0 | 0 |


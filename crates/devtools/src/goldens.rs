//! Cross-checks between the committed goldens and the Go tree they were generated from.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde_json::Value;

use crate::{
    error::{Result, ToolError},
    fixtures,
    godump::Dump,
    manifest::{self, MANIFEST_PATH, Manifest},
    repo::Repo,
    testutil::{check, check_eq},
};

fn golden(repo: &Repo, rel: &str) -> Result<Value> {
    let path = repo.path(rel);
    let bytes = fs::read(&path).map_err(|e| ToolError::io(&path, &e))?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn field<'v>(v: &'v Value, key: &str) -> Result<&'v Value> {
    v.get(key)
        .ok_or_else(|| ToolError::new(format!("golden has no {key}")))
}

fn array<'v>(v: &'v Value, key: &str) -> Result<&'v Vec<Value>> {
    field(v, key)?
        .as_array()
        .ok_or_else(|| ToolError::new(format!("{key} is not an array")))
}

fn str_of<'v>(v: &'v Value, key: &str) -> Result<&'v str> {
    field(v, key)?
        .as_str()
        .ok_or_else(|| ToolError::new(format!("{key} is not a string")))
}

/// Every golden records the commit the manifest was generated from, the one `UPSTREAM_SYNC`
/// pins.
#[test]
fn goldens_share_go_commit() -> Result<()> {
    let repo = Repo::locate(None)?;
    let manifest = golden(&repo, MANIFEST_PATH)?;
    let commit = str_of(&manifest, "go_commit")?;
    check_eq(
        commit,
        repo.pinned_commit()?.as_str(),
        "go_commit is the pin",
    )?;
    for dump in Dump::ALL {
        check_eq(
            str_of(&golden(&repo, dump.golden())?, "go_commit")?,
            commit,
            dump.golden(),
        )?;
    }
    Ok(())
}

/// The manifest generator's `BidderName` constants (Rust parser) equal the Go dump's (go/ast).
#[test]
fn bidder_constants_match_go_dump() -> Result<()> {
    let repo = Repo::locate(None)?;
    let rust = manifest::bidder_constants(&repo)?;
    let names = golden(&repo, Dump::BidderNames.golden())?;
    let go = array(&names, "constants")?
        .iter()
        .map(|c| {
            Ok((
                str_of(c, "name")?.to_owned(),
                str_of(c, "value")?.to_owned(),
            ))
        })
        .collect::<Result<BTreeMap<String, String>>>()?;
    check(go.len() > 250, "the Go dump lists the bidder constants")?;
    check_eq(&rust, &go, "BidderName constants")
}

/// `go_bidder_infos.json` covers exactly the files in `static/bidder-info`.
#[test]
fn bidder_infos_cover_static_yaml() -> Result<()> {
    let repo = Repo::locate(None)?;
    let dir = repo.go_path("static/bidder-info");
    let mut on_disk = BTreeSet::new();
    for entry in fs::read_dir(&dir).map_err(|e| ToolError::io(&dir, &e))? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if Path::new(&name).extension().is_some_and(|e| e == "yaml") {
            let _ = on_disk.insert(name);
        }
    }
    let infos = golden(&repo, Dump::BidderInfos.golden())?;
    let mut dumped = BTreeSet::new();
    for f in array(&infos, "files")? {
        let _ = dumped.insert(str_of(f, "name")?.to_owned());
    }
    check_eq(&dumped, &on_disk, "bidder-info files")?;
    let loaded = field(&infos, "bidder_infos")?
        .as_object()
        .ok_or_else(|| ToolError::new("bidder_infos is not an object"))?;
    let stems: BTreeSet<String> = on_disk
        .iter()
        .map(|n| n.trim_end_matches(".yaml").to_owned())
        .collect();
    let keys: BTreeSet<String> = loaded.keys().cloned().collect();
    check_eq(&keys, &stems, "bidder_infos keys are the file stems")
}

/// `config.SetupViper` calls `v.BindEnv(key)` for a few keys before `v.SetEnvPrefix("PBS")`, so
/// viper also reads them from the un-prefixed variable (`GDPR_DEFAULT_VALUE`, which the Go docs
/// use). The dump lists exactly those as verified fallbacks, and every other key has none.
#[test]
fn config_defaults_record_unprefixed_bind_env() -> Result<()> {
    let repo = Repo::locate(None)?;
    let path = repo.go_path("config/config.go");
    let src = fs::read_to_string(&path).map_err(|e| ToolError::io(&path, &e))?;
    let (before_prefix, _) = src
        .split_once("v.SetEnvPrefix(")
        .ok_or_else(|| ToolError::new("config.go no longer calls v.SetEnvPrefix"))?;
    let want = before_prefix
        .split("v.BindEnv(\"")
        .skip(1)
        .map(|call| {
            let (key, _) = call
                .split_once('"')
                .ok_or_else(|| ToolError::new("unterminated BindEnv key"))?;
            Ok((key.to_owned(), key.to_uppercase().replace('.', "_")))
        })
        .collect::<Result<BTreeMap<String, String>>>()?;
    check(
        want.contains_key("gdpr.default_value"),
        "gdpr.default_value is bound early",
    )?;

    let defaults = golden(&repo, Dump::ConfigDefaults.golden())?;
    check(
        field(&defaults, "empty_env_ignored")?.as_bool() == Some(true),
        "an empty variable counts as unset",
    )?;
    let mut got = Vec::new();
    for key in array(&defaults, "keys")? {
        let name = str_of(key, "key")?;
        check(
            field(key, "env_verified")?.as_bool() == Some(true),
            &format!("{name}: its PBS_ variable binds"),
        )?;
        for fallback in array(key, "env_fallbacks")? {
            check(
                field(fallback, "verified")?.as_bool() == Some(true),
                &format!("{name}: its fallback variable binds"),
            )?;
            got.push((name.to_owned(), str_of(fallback, "env")?.to_owned()));
        }
    }
    // The golden lists keys sorted, as the map iterates.
    check_eq(
        &got,
        &want.into_iter().collect::<Vec<_>>(),
        "keys with an un-prefixed variable",
    )
}

/// Every `cannot unmarshal pkg.Type.Field` named in an adapter fixture (Go's jsoniter error text)
/// names a type and field present in `go_struct_meta.json`, so ports can reproduce the message.
#[test]
fn struct_meta_resolves_fixture_unmarshal_errors() -> Result<()> {
    let repo = Repo::locate(None)?;
    let meta = golden(&repo, Dump::StructMeta.golden())?;
    let mut types: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for pkg in array(&meta, "packages")? {
        let pkg_name = str_of(pkg, "name")?;
        for ty in array(pkg, "types")? {
            let fields = types
                .entry((pkg_name.to_owned(), str_of(ty, "name")?.to_owned()))
                .or_default();
            for f in ty
                .get("fields")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice)
            {
                let _ = fields.insert(str_of(f, "name")?.to_owned());
            }
        }
    }
    let mut refs = BTreeSet::new();
    for path in fixtures::all_json(&repo.go_path("adapters"))? {
        let text = fs::read_to_string(&path).map_err(|e| ToolError::io(&path, &e))?;
        for rest in text.split("cannot unmarshal ").skip(1) {
            let word: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
                .collect();
            let word = word.trim_end_matches('.');
            if word.contains('.') {
                let _ = refs.insert(word.to_owned());
            }
        }
    }
    check(refs.len() >= 40, "the fixtures name unmarshal targets")?;
    let mut unresolved = Vec::new();
    for r in &refs {
        let parts: Vec<&str> = r.split('.').collect();
        let resolved = match parts.as_slice() {
            [pkg, ty, field, ..] => types
                .get(&((*pkg).to_owned(), (*ty).to_owned()))
                .is_some_and(|f| f.contains(*field)),
            [pkg, ty] => types.contains_key(&((*pkg).to_owned(), (*ty).to_owned())),
            _ => false,
        };
        if !resolved {
            unresolved.push(r.clone());
        }
    }
    check_eq(
        unresolved.as_slice(),
        &[][..],
        "unmarshal targets missing from go_struct_meta.json",
    )
}

/// The committed manifest equals a fresh generation from the Go tree (the `--check` gate).
#[test]
fn committed_manifest_is_current() -> Result<()> {
    let repo = Repo::locate(None)?;
    let path = repo.path(MANIFEST_PATH);
    let committed: Manifest =
        serde_json::from_slice(&fs::read(&path).map_err(|e| ToolError::io(&path, &e))?)?;
    let fresh = manifest::generate(&repo, &committed.go_commit)?;
    check(
        fresh == committed,
        "the committed manifest is stale: run `cargo run -p pbs-devtools -- adapter-manifest`",
    )
}

/// Spot checks of constructions the generator must follow, against the Go test sources.
#[test]
fn manifest_spot_checks() -> Result<()> {
    let repo = Repo::locate(None)?;
    let path = repo.path(MANIFEST_PATH);
    let m: Manifest =
        serde_json::from_slice(&fs::read(&path).map_err(|e| ToolError::io(&path, &e))?)?;
    check_eq(&m.totals.runs, &282, "runs")?;
    let run = |pkg: &str, test: &str| {
        m.runs
            .iter()
            .find(|r| r.go_pkg == pkg && r.go_test == test)
            .ok_or_else(|| ToolError::new(format!("no run {pkg}/{test}")))
    };

    // msft_test.go: two runs over different roots, one with ExtraAdapterInfo.
    let extra = run("msft", "TestJsonSamplesWithExtraInfo")?;
    check_eq(
        extra.fixture_root.as_str(),
        "adapters/msft/test-extrainfo",
        "msft extra-info root",
    )?;
    let extra_info = extra.adapter.as_ref().map(|a| a.extra_info.as_str());
    check_eq(
        &extra_info,
        &Some(r#"{"hb_source": 50, "hb_source_video": 60}"#),
        "msft extra info",
    )?;
    check(
        m.runs.iter().filter(|r| r.go_pkg == "msft").count() >= 2,
        "msft has several runs",
    )?;

    // gamoshi_test.go: Builder with an empty endpoint.
    let gamoshi = run("gamoshi", "TestJsonSamplesWithHardcodedURI")?;
    check_eq(
        &gamoshi.adapter.as_ref().map(|a| a.endpoint.as_str()),
        &Some(""),
        "gamoshi endpoint",
    )?;

    // rubicon_test.go: XAPI credentials.
    let rubicon = run("rubicon", "TestJsonSamples")?
        .adapter
        .as_ref()
        .ok_or_else(|| ToolError::new("rubicon adapter"))?;
    check_eq(
        &(
            rubicon.xapi.username.as_str(),
            rubicon.xapi.password.as_str(),
            rubicon.xapi.tracker.as_str(),
        ),
        &("xuser", "xpass", "pbs-test-tracker"),
        "rubicon xapi",
    )?;

    // facebook_test.go: PlatformID and AppSecret.
    let an = run("audienceNetwork", "TestJsonSamples")?
        .adapter
        .as_ref()
        .ok_or_else(|| ToolError::new("an adapter"))?;
    check_eq(
        &(an.platform_id.as_str(), an.app_secret.as_str()),
        &("test-platform-id", "test-app-secret"),
        "an",
    )?;

    // yieldlab_test.go: a struct literal with function fields supplied by hooks.
    let yieldlab = run("yieldlab", "TestJsonSamples")?;
    check_eq(
        &yieldlab.construct,
        &manifest::Construct::Direct,
        "yieldlab construct",
    )?;
    check_eq(&yieldlab.bidder_name, &None, "yieldlab has no bidder name")?;
    let direct = yieldlab
        .direct
        .as_ref()
        .ok_or_else(|| ToolError::new("yieldlab direct"))?;
    check_eq(
        &direct.fields.get("endpoint").map(String::as_str),
        &Some("https://ad.yieldlab.net/testing/"),
        "yieldlab endpoint",
    )?;
    check_eq(
        &yieldlab.hooks.week.as_deref(),
        &Some("33"),
        "yieldlab week hook",
    )?;

    // ix_test.go: Builder bound in an if statement's init.
    let ix = run("ix", "TestJsonSamples")?;
    check_eq(&ix.bidder_name.as_deref(), &Some("ix"), "ix bidder")?;
    check_eq(&ix.server.as_ref().map(|s| s.gvl_id), &Some(1), "ix server")?;
    check(ix.setup.is_empty(), "ix has no setup")?;

    // The eight runs with setup statements are exactly the hooked ones.
    let with_setup: BTreeSet<&str> = m
        .runs
        .iter()
        .filter(|r| !r.setup.is_empty() || r.direct.is_some())
        .map(|r| r.go_pkg.as_str())
        .collect();
    let want: BTreeSet<&str> = [
        "adnuntius",
        "appnexus",
        "cadent_aperture_mx",
        "eplanning",
        "flipp",
        "sharethrough",
        "smaato",
        "yieldlab",
    ]
    .into_iter()
    .collect();
    check_eq(&with_setup, &want, "runs needing hooks")
}

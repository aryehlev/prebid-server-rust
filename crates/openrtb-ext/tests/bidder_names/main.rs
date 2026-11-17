//! The bidder name registry against Go (`testdata/go_bidder_names.json`, dumped by
//! `pbs-devtools go-dump bidder-names`), before and after the aliases of `static/bidder-info`
//! are registered, and the Go tests that register aliases (`TestSetAliasBidderName`, the valid
//! case of `TestNewBidderParamsValidator`).
//!
//! Registering an alias changes the package's state for good, as in Go, so everything runs in
//! one test, in order, in a binary of its own.

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        env,
        fmt::Debug,
        fs,
        path::PathBuf,
        process,
    };

    use pbs_devtools::repo;
    use pbs_openrtb_ext::openrtb_ext::{
        BidderName, BidderParamValidator as _, build_bidder_map, build_bidder_name_hash_set,
        build_bidder_string_slice, core_bidder_names, get_alias_bidder_to_parent,
        is_bidder_name_reserved, is_potential_bidder, new_bidder_params_validator,
        normalize_bidder_name, set_alias_bidder_name,
    };
    use serde_json::Value;

    type TestResult<T = ()> = Result<T, String>;

    fn ensure_eq<T: PartialEq + Debug + ?Sized>(
        actual: &T,
        expected: &T,
        what: &str,
    ) -> TestResult {
        if actual == expected {
            Ok(())
        } else {
            Err(format!("{what}: got {actual:?}, want {expected:?}"))
        }
    }

    /// The Go root, whose `static/bidder-params` the validator loads (`$PBS_GO_ROOT`, else
    /// `.upstream` at the workspace root; `scripts/fetch-upstream.sh` makes it).
    fn go_root() -> TestResult<PathBuf> {
        repo::go_root().map_err(|e| e.to_string())
    }

    /// The workspace root, which holds the cross-crate goldens in `testdata/`.
    fn workspace_root() -> TestResult<PathBuf> {
        repo::workspace_root().map_err(|e| e.to_string())
    }

    fn field<'a>(value: &'a Value, key: &str) -> TestResult<&'a Value> {
        value
            .get(key)
            .ok_or_else(|| format!("go_bidder_names.json: no {key} in {value}"))
    }

    fn text<'a>(value: &'a Value, key: &str) -> TestResult<&'a str> {
        field(value, key)?
            .as_str()
            .ok_or_else(|| format!("go_bidder_names.json: {key} is not a string"))
    }

    fn flag(value: &Value, key: &str) -> TestResult<bool> {
        field(value, key)?
            .as_bool()
            .ok_or_else(|| format!("go_bidder_names.json: {key} is not a bool"))
    }

    fn list<'a>(value: &'a Value, key: &str) -> TestResult<&'a [Value]> {
        field(value, key)?
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| format!("go_bidder_names.json: {key} is not a list"))
    }

    fn strings(value: &Value, key: &str) -> TestResult<Vec<String>> {
        list(value, key)?
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("go_bidder_names.json: {key} holds a non-string"))
            })
            .collect()
    }

    fn table(value: &Value, key: &str) -> TestResult<BTreeMap<String, String>> {
        field(value, key)?
            .as_object()
            .ok_or_else(|| format!("go_bidder_names.json: {key} is not an object"))?
            .iter()
            .map(|(k, v)| {
                v.as_str()
                    .map(|v| (k.clone(), v.to_owned()))
                    .ok_or_else(|| format!("go_bidder_names.json: {key}.{k} is not a string"))
            })
            .collect()
    }

    fn names(names: &[BidderName]) -> Vec<String> {
        names.iter().map(|n| n.as_str().to_owned()).collect()
    }

    /// `NormalizeBidderName(name)` for each key of a table, and its upper-case spelling (Go:
    /// `table[strings.ToLower(name)]`).
    fn check_normalize(table: &BTreeMap<String, String>, what: &str) -> TestResult {
        for (lower, want) in table {
            for name in [lower.clone(), lower.to_uppercase()] {
                ensure_eq(
                    &normalize_bidder_name(&name).map(|n| n.as_str().to_owned()),
                    &Some(want.clone()),
                    &format!("{what}: NormalizeBidderName({name:?})"),
                )?;
            }
        }
        Ok(())
    }

    /// The package's functions of the names, before any alias.
    fn check_static(golden: &Value) -> TestResult {
        let core = strings(golden, "core_bidder_names")?;
        ensure_eq(&names(&core_bidder_names()), &core, "CoreBidderNames()")?;
        ensure_eq(
            &build_bidder_string_slice(),
            &core,
            "BuildBidderStringSlice()",
        )?;
        let map: BTreeMap<String, String> = build_bidder_map()
            .into_iter()
            .map(|(k, v)| (k, v.as_str().to_owned()))
            .collect();
        let want_map: BTreeMap<String, String> =
            core.iter().map(|n| (n.clone(), n.clone())).collect();
        ensure_eq(&map, &want_map, "BuildBidderMap()")?;
        let set: BTreeSet<String> = build_bidder_name_hash_set()
            .iter()
            .map(|name| name.as_str().to_owned())
            .collect();
        let want_set: BTreeSet<String> = core.iter().cloned().collect();
        ensure_eq(&set, &want_set, "BuildBidderNameHashSet()")?;
        let in_core: BTreeSet<String> = core.iter().cloned().collect();
        for c in list(golden, "constants")? {
            let value = text(c, "value")?;
            let name = text(c, "name")?;
            ensure_eq(
                &in_core.contains(value),
                &flag(c, "in_core_bidder_names")?,
                &format!("{name} in CoreBidderNames()"),
            )?;
            ensure_eq(
                &is_bidder_name_reserved(value),
                &flag(c, "is_bidder_name_reserved")?,
                &format!("IsBidderNameReserved({name})"),
            )?;
            ensure_eq(
                &is_potential_bidder(value),
                &flag(c, "is_potential_bidder")?,
                &format!("IsPotentialBidder({name})"),
            )?;
        }
        check_normalize(&table(golden, "normalize_static")?, "before aliases")?;
        ensure_eq(
            &get_alias_bidder_to_parent(),
            &BTreeMap::new(),
            "GetAliasBidderToParent() before aliases",
        )
    }

    /// Registers the aliases (as `config.LoadBidderInfoFromDisk` does), then checks the names.
    fn check_aliases(golden: &Value) -> TestResult<BTreeMap<String, String>> {
        let core = strings(golden, "core_bidder_names")?;
        let aliases = table(golden, "aliases")?;
        for (alias, parent) in &aliases {
            set_alias_bidder_name(alias, BidderName::from(parent.as_str()))
                .map_err(|e| format!("SetAliasBidderName({alias}): {e}"))?;
        }
        let after = names(&core_bidder_names());
        ensure_eq(
            after.get(..core.len()).unwrap_or_default(),
            core.as_slice(),
            "CoreBidderNames() keeps the core bidders first",
        )?;
        let mut appended: Vec<String> = after.get(core.len()..).unwrap_or_default().to_vec();
        appended.sort();
        ensure_eq(
            &appended,
            &strings(golden, "alias_names_appended_sorted")?,
            "the names SetAliasBidderName appends to CoreBidderNames()",
        )?;
        // The registry's copy of the set follows every alias (Go builds it from
        // CoreBidderNames on each call).
        let set: BTreeSet<String> = build_bidder_name_hash_set()
            .iter()
            .map(|name| name.as_str().to_owned())
            .collect();
        let want_set: BTreeSet<String> = after.iter().cloned().collect();
        ensure_eq(&set, &want_set, "BuildBidderNameHashSet() with aliases")?;
        for alias in aliases.keys() {
            if !build_bidder_name_hash_set().contains(alias.as_str()) {
                return Err(format!("BuildBidderNameHashSet() lacks the alias {alias}"));
            }
        }
        let parents: BTreeMap<String, String> = get_alias_bidder_to_parent()
            .into_iter()
            .map(|(a, p)| (a.as_str().to_owned(), p.as_str().to_owned()))
            .collect();
        ensure_eq(&parents, &aliases, "GetAliasBidderToParent()")?;
        check_normalize(&table(golden, "normalize_with_aliases")?, "with aliases")?;
        for probe in list(golden, "probes")? {
            let input = text(probe, "input")?;
            ensure_eq(
                &is_bidder_name_reserved(input),
                &flag(probe, "is_bidder_name_reserved")?,
                &format!("IsBidderNameReserved({input:?})"),
            )?;
            ensure_eq(
                &is_potential_bidder(input),
                &flag(probe, "is_potential_bidder")?,
                &format!("IsPotentialBidder({input:?})"),
            )?;
            let want = flag(probe, "normalized_found")?
                .then(|| text(probe, "normalized").map(str::to_owned))
                .transpose()?;
            ensure_eq(
                &normalize_bidder_name(input).map(|n| n.as_str().to_owned()),
                &want,
                &format!("NormalizeBidderName({input:?})"),
            )?;
        }
        Ok(aliases)
    }

    /// A directory of schema files, removed when dropped.
    struct TempDir(String);

    impl TempDir {
        fn new(name: &str, files: &[(&str, &str)]) -> TestResult<Self> {
            let dir = format!(
                "{}/pbs-openrtb-ext-{}-{name}",
                env::temp_dir().display(),
                process::id()
            );
            let _cleared = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).map_err(|e| format!("creating {dir}: {e}"))?;
            for (file, contents) in files {
                fs::write(format!("{dir}/{file}"), contents)
                    .map_err(|e| format!("writing {dir}/{file}: {e}"))?;
            }
            Ok(Self(dir))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _removed = fs::remove_dir_all(&self.0);
        }
    }

    /// Go: `TestNewBidderParamsValidator`, valid case, through the registry: an alias gets its
    /// parent's schema, and an alias whose parent has none gets none.
    fn check_validator(aliases: &BTreeMap<String, String>) -> TestResult {
        let mut by_parent = aliases.iter();
        let (alias, parent) = by_parent.next().ok_or("no aliases")?;
        let (other_alias, other_parent) = by_parent
            .find(|(_, p)| *p != parent)
            .ok_or("no alias of another parent")?;
        let file = format!("{parent}.json");
        let dir = TempDir::new("valid", &[(file.as_str(), "{}")])?;
        let validator = new_bidder_params_validator(&dir.0).map_err(|e| e.to_string())?;
        ensure_eq(validator.schema(parent), "{}", &format!("Schema({parent})"))?;
        ensure_eq(validator.schema(alias), "{}", &format!("Schema({alias})"))?;
        ensure_eq(
            validator.schema(other_alias),
            "",
            &format!("Schema({other_alias}) without {other_parent}.json"),
        )?;
        validator
            .validate(alias, b"{}")
            .map_err(|e| format!("Validate({alias}): {e}"))
    }

    /// The validator of `static/bidder-params` with every alias registered, as the server builds
    /// it: each alias has its parent's schema.
    fn check_static_validator(aliases: &BTreeMap<String, String>) -> TestResult {
        let dir = go_root()?.join("static/bidder-params");
        let validator =
            new_bidder_params_validator(&dir.to_string_lossy()).map_err(|e| e.to_string())?;
        for (alias, parent) in aliases {
            ensure_eq(
                validator.schema(alias),
                validator.schema(parent),
                &format!("Schema({alias}), an alias of {parent}"),
            )?;
        }
        Ok(())
    }

    /// Go: `TestSetAliasBidderName`.
    fn check_set_alias_bidder_name() -> TestResult {
        let parent_bidder = BidderName::from("pBidder");
        set_alias_bidder_name("aBidder", parent_bidder.clone()).map_err(|e| e.to_string())?;
        let alias = BidderName::from("aBidder");
        ensure_eq(
            &core_bidder_names().contains(&alias),
            &true,
            "CoreBidderNames() contains aBidder",
        )?;
        ensure_eq(
            &get_alias_bidder_to_parent().get(&alias),
            &Some(&parent_bidder),
            "aliasBidderToParent[aBidder]",
        )?;
        ensure_eq(
            &normalize_bidder_name("abidder"),
            &Some(alias),
            "bidderNameLookup[abidder]",
        )?;
        ensure_eq(
            &set_alias_bidder_name("all", parent_bidder).map_err(|e| e.to_string()),
            &Err("alias all is a reserved bidder name and cannot be used".to_owned()),
            "SetAliasBidderName(all)",
        )
    }

    #[test]
    fn bidder_names_match_go() -> TestResult {
        let path = workspace_root()?.join("testdata/go_bidder_names.json");
        let data = fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        let golden: Value = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
        check_static(&golden)?;
        let aliases = check_aliases(&golden)?;
        check_validator(&aliases)?;
        check_static_validator(&aliases)?;
        check_set_alias_bidder_name()
    }
}

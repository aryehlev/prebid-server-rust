//! Go: `openrtb_ext/bidders.go` (`BidderParamValidator`, `NewBidderParamsValidator`).
//!
//! The validator compiles every `static/bidder-params/<bidder>.json` schema once at startup.
//! Aliases share their parent's schema. `Validate` returns the concatenated gojsonschema
//! `ResultError.String()` values (no separator), which request validation wraps as
//! `request.imp[%d].ext.prebid.bidder.%s failed validation.\n%v`.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fmt,
    fs::{self, File},
    io,
    sync::Arc,
};

use crate::gojsonschema::{GoPanic, Schema, ValidateError, decode_json_using_number};

/// Go: `openrtb_ext.BidderParamValidator`: enforces `imp[i].ext.prebid.bidder.{bidder}` values.
pub trait BidderParamValidator: fmt::Debug + Send + Sync {
    /// Go: `Validate(name BidderName, ext json.RawMessage) error`.
    ///
    /// # Errors
    ///
    /// The params do not satisfy the bidder's schema, are not JSON, or hit a case where Go
    /// panics.
    fn validate(&self, name: &str, ext: &[u8]) -> Result<(), ParamsError>;

    /// Go: `Schema(name BidderName) string`: the schema file contents (empty when unknown).
    fn schema(&self, name: &str) -> &str;
}

/// The error Go's `Validate` returns (its `Error()` text is this type's `Display`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamsError {
    /// The params do not satisfy the schema: every `ResultError.String()` concatenated.
    Invalid(String),
    /// The params are not JSON: `serde_json`'s text (`EOF while parsing a value at line 1
    /// column 0`), where Go has its `encoding/json` decoder's (`unexpected EOF`;
    /// jsonschema.decode-errors).
    Decode(String),
    /// Go panics here instead of returning an error: `Validate` on the nil schema of a bidder
    /// without one. A Go server would abort the request.
    Panic(GoPanic),
}

impl fmt::Display for ParamsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(text) | Self::Decode(text) => f.write_str(text),
            Self::Panic(panic) => write!(f, "panic: {panic}"),
        }
    }
}

impl Error for ParamsError {}

/// A failure of [`new_bidder_params_validator`], with Go's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError(String);

impl LoadError {
    /// Go's error text.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for LoadError {}

/// Go: `openrtb_ext.bidderParamValidator`.
#[derive(Debug, Clone, Default)]
pub struct StandardBidderParamValidator {
    schema_contents: BTreeMap<String, String>,
    parsed_schemas: BTreeMap<String, Option<Arc<Schema>>>,
}

impl StandardBidderParamValidator {
    /// A validator over already compiled schemas and the texts [`BidderParamValidator::schema`]
    /// returns (Go: a `bidderParamValidator{parsedSchemas: ..., schemaContents: ...}` literal).
    #[must_use]
    pub fn from_schemas(
        parsed_schemas: BTreeMap<String, Schema>,
        schema_contents: BTreeMap<String, String>,
    ) -> Self {
        let parsed_schemas = parsed_schemas
            .into_iter()
            .map(|(name, schema)| (name, Some(Arc::new(schema))))
            .collect();
        Self {
            schema_contents,
            parsed_schemas,
        }
    }
}

impl BidderParamValidator for StandardBidderParamValidator {
    fn validate(&self, name: &str, ext: &[u8]) -> Result<(), ParamsError> {
        let Some(Some(schema)) = self.parsed_schemas.get(name) else {
            // Go calls Validate on a nil *Schema: LoadJSON runs first, then the nil root schema
            // is dereferenced.
            let _doc = decode_json_using_number(ext).map_err(ParamsError::Decode)?;
            return Err(ParamsError::Panic(GoPanic::nil_schema()));
        };
        let result = schema.validate(ext).map_err(|e| match e {
            ValidateError::Load(text) => ParamsError::Decode(text),
            ValidateError::Panic(panic) => ParamsError::Panic(panic),
        })?;
        if result.valid() {
            return Ok(());
        }
        let text: String = result.errors().iter().map(ToString::to_string).collect();
        Err(ParamsError::Invalid(text))
    }

    fn schema(&self, name: &str) -> &str {
        self.schema_contents.get(name).map_or("", String::as_str)
    }
}

/// Go: `openrtb_ext.NewBidderParamsValidator(schemaDirectory)`.
///
/// `bidder_names` and `alias_bidder_to_parent` stand for Go's package globals at the time of
/// the call: the keys of `BuildBidderMap()` (core bidders plus the aliases registered from
/// `static/bidder-info`) and `GetAliasBidderToParent()`.
///
/// # Errors
///
/// Go's messages: the directory cannot be read, a file name is not a bidder name
/// (`File %s/%s does not match a valid BidderName.`), or a schema fails to load
/// (`Failed to load json schema at %s: %v`).
pub fn new_bidder_params_validator(
    schema_directory: &str,
    bidder_names: &BTreeSet<String>,
    alias_bidder_to_parent: &BTreeMap<String, String>,
) -> Result<StandardBidderParamValidator, LoadError> {
    let entries = read_dir_sorted(schema_directory).map_err(|e| {
        LoadError(format!(
            "Failed to read JSON schemas from directory {schema_directory}. {e}"
        ))
    })?;
    let mut schema_contents = BTreeMap::new();
    let mut parsed_schemas = BTreeMap::new();
    for file_name in entries {
        let bidder_name = file_name
            .strip_suffix(".json")
            .unwrap_or(&file_name)
            .to_owned();
        if !bidder_names.contains(&bidder_name) {
            return Err(LoadError(format!(
                "File {schema_directory}/{file_name} does not match a valid BidderName."
            )));
        }
        // Go: filepath.Join (which cleans) then filepath.Abs; on failure Go prints the empty
        // result.
        let joined = if schema_directory.is_empty() {
            file_name.clone()
        } else {
            format!("{schema_directory}/{file_name}")
        };
        let to_open = go_abs(&joined).map_err(|e| {
            LoadError(format!(
                "Failed to get an absolute representation of the path: , {e}"
            ))
        })?;
        let schema = load_schema(&to_open)
            .map_err(|e| LoadError(format!("Failed to load json schema at {to_open}: {e}")))?;
        let file_path = format!("{schema_directory}/{file_name}");
        let contents = fs::read(&file_path).map_err(|e| {
            let reason = path_error("open", &file_path, &e);
            LoadError(format!("Failed to read file {file_path}: {reason}"))
        })?;
        let _replaced = parsed_schemas.insert(bidder_name.clone(), Some(Arc::new(schema)));
        let _previous =
            schema_contents.insert(bidder_name, String::from_utf8_lossy(&contents).into_owned());
    }
    // Set alias bidder params schema to its parent.
    for (alias, parent) in alias_bidder_to_parent {
        let parent_schema = parsed_schemas.get(parent).cloned().flatten();
        let parent_contents = schema_contents.get(parent).cloned().unwrap_or_default();
        let _replaced = parsed_schemas.insert(alias.clone(), parent_schema);
        let _previous = schema_contents.insert(alias.clone(), parent_contents);
    }
    Ok(StandardBidderParamValidator {
        schema_contents,
        parsed_schemas,
    })
}

/// Go: `os.ReadDir(name)` (entries sorted by file name), as file names.
fn read_dir_sorted(dir: &str) -> Result<Vec<String>, String> {
    let entries = fs::read_dir(dir).map_err(|e| path_error("open", dir, &e))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| path_error("readdirent", dir, &e))?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}

/// Go: `gojsonschema.NewSchema(gojsonschema.NewReferenceLoader("file:///" + toOpen))`.
fn load_schema(to_open: &str) -> Result<Schema, String> {
    let opened = go_reference_loader_path(to_open)?;
    // Go: `os.Open` then `ioutil.ReadAll`; the two failures are reported differently.
    let _opened_file = File::open(&opened).map_err(|e| path_error("open", &opened, &e))?;
    let bytes = fs::read(&opened).map_err(|e| path_error("read", &opened, &e))?;
    Schema::new(&bytes)
}

/// The path gojsonschema's reference loader opens for `"file:///" + abs`: the URL keeps the
/// extra slash, drops a fragment, and `url.QueryUnescape` decodes `%XX` and turns `+` into a
/// space.
fn go_reference_loader_path(abs: &str) -> Result<String, String> {
    let without_fragment = abs.split('#').next().unwrap_or(abs);
    let raw = format!("/{without_fragment}");
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = bytes.get(i + 1..i + 3).and_then(|h| str::from_utf8(h).ok());
                let value = hex.and_then(|h| u8::from_str_radix(h, 16).ok());
                let Some(value) = value else {
                    let bad: String = raw.chars().skip(i).take(3).collect();
                    return Err(format!(
                        "parse \"file:///{abs}\": invalid URL escape \"{bad}\""
                    ));
                };
                out.push(value);
                i += 2;
            }
            other => out.push(other),
        }
        i += 1;
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Go: `filepath.Abs(path)`: joins relative paths to the working directory, then `Clean`s.
fn go_abs(path: &str) -> Result<String, String> {
    if path.starts_with('/') {
        return Ok(go_clean(path));
    }
    let wd = env::current_dir().map_err(|e| path_error("getwd", ".", &e))?;
    Ok(go_clean(&format!("{}/{path}", wd.to_string_lossy())))
}

/// Go: `filepath.Clean` for Unix paths.
fn go_clean(path: &str) -> String {
    let rooted = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    let _parent = parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_owned(),
        (false, false) => joined,
    }
}

/// Go: `*fs.PathError.Error()`: `op path: errno text`.
///
/// Go's errno texts are the C library's `strerror` texts with a lower-case first letter; std
/// renders an OS error as `strerror (os error N)`.
fn path_error(op: &str, path: &str, err: &io::Error) -> String {
    let rendered = err.to_string();
    let text = err.raw_os_error().map_or_else(
        || rendered.clone(),
        |code| {
            let suffix = format!(" (os error {code})");
            let message = rendered.strip_suffix(&suffix).unwrap_or(&rendered);
            let mut chars = message.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_lowercase().chain(chars).collect::<String>()
            })
        },
    );
    format!("{op} {path}: {text}")
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        env, fs, process,
    };

    use super::{
        BidderParamValidator as _, ParamsError, StandardBidderParamValidator, go_abs, go_clean,
        go_reference_loader_path, new_bidder_params_validator,
    };
    use crate::gojsonschema::Schema;

    /// Go: `openrtb_ext.TestBidderParamValidatorValidate`.
    #[test]
    fn validate_matches_go() -> Result<(), String> {
        let schema = Schema::new(
            br#"{
                "$schema": "http://json-schema.org/draft-04/schema#",
                "title": "Test Params",
                "description": "Test Description",
                "type": "object",
                "properties": {
                  "placementId": {
                    "type": "integer",
                    "description": "An ID which identifies this placement of the impression."
                  },
                  "optionalText": {
                    "type": "string",
                    "description": "Optional text for testing."
                  }
                },
                "required": ["placementId"]
            }"#,
        )?;
        let validator = StandardBidderParamValidator::from_schemas(
            [("foo".to_owned(), schema)].into(),
            BTreeMap::new(),
        );
        let cases: [(&str, &[u8], Option<&str>); 4] = [
            ("Valid", br#"{"placementId":123}"#, None),
            (
                "Invalid - Wrong Type",
                br#"{"placementId":"stringInsteadOfInt"}"#,
                Some("placementId: Invalid type. Expected: integer, given: string"),
            ),
            (
                "Invalid - Empty Object",
                b"{}",
                Some("(root): placementId is required"),
            ),
            // Go: "invalid character 'm' looking for beginning of value"
            // (jsonschema.decode-errors).
            (
                "Malformed",
                b"malformedJSON",
                Some("expected value at line 1 column 1"),
            ),
        ];
        for (description, ext, want) in cases {
            let got = validator.validate("foo", ext).err().map(|e| e.to_string());
            if got.as_deref() != want {
                return Err(format!("{description}: got {got:?}, want {want:?}"));
            }
        }
        // Go dereferences the nil schema of an unknown bidder, after decoding the params.
        match (
            validator.validate("bar", b"{}"),
            validator.validate("bar", b"{"),
        ) {
            // Go: "unexpected EOF".
            (Err(ParamsError::Panic(_)), Err(ParamsError::Decode(text)))
                if text == "EOF while parsing an object at line 1 column 1" =>
            {
                Ok(())
            }
            other => Err(format!("unknown bidder: {other:?}")),
        }
    }

    /// Go: `openrtb_ext.TestBidderParamValidatorSchema`.
    #[test]
    fn schema_matches_go() -> Result<(), String> {
        let validator = StandardBidderParamValidator::from_schemas(
            BTreeMap::new(),
            [
                ("foo".to_owned(), "foo content".to_owned()),
                ("bar".to_owned(), "bar content".to_owned()),
            ]
            .into(),
        );
        match (validator.schema("bar"), validator.schema("baz")) {
            ("bar content", "") => Ok(()),
            other => Err(format!("got {other:?}")),
        }
    }

    /// A scratch directory for one test, removed when dropped.
    struct TempDir(String);

    impl TempDir {
        fn new(name: &str, files: &[(&str, &str)]) -> Result<Self, String> {
            let dir = format!(
                "{}/pbs-bidder-params-{}-{name}",
                env::temp_dir().display(),
                process::id()
            );
            let _cleared = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            for (file, contents) in files {
                fs::write(format!("{dir}/{file}"), contents).map_err(|e| e.to_string())?;
            }
            Ok(Self(dir))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _removed = fs::remove_dir_all(&self.0);
        }
    }

    fn bidders() -> BTreeSet<String> {
        ["appnexus", "rubicon"].map(str::to_owned).into()
    }

    fn aliases() -> BTreeMap<String, String> {
        [("rubicon".to_owned(), "appnexus".to_owned())].into()
    }

    /// Go: `openrtb_ext.TestNewBidderParamsValidator`, valid case: aliases share their parent's
    /// schema.
    #[test]
    fn new_validator_shares_parent_schemas() -> Result<(), String> {
        let dir = TempDir::new("valid", &[("appnexus.json", "{}")])?;
        let validator = new_bidder_params_validator(&dir.0, &bidders(), &aliases())
            .map_err(|e| e.to_string())?;
        if validator.schema("appnexus") != "{}" || validator.schema("rubicon") != "{}" {
            return Err("aliases must share their parent's schema".to_owned());
        }
        validator
            .validate("rubicon", b"{}")
            .map_err(|e| e.to_string())
    }

    /// Go: `openrtb_ext.TestNewBidderParamsValidator`, the failures a real file system can
    /// produce.
    #[test]
    fn new_validator_errors_match_go() -> Result<(), String> {
        let valid = TempDir::new("errors", &[])?;
        let missing = format!("{}-missing", valid.0);
        let missing_error = format!(
            "Failed to read JSON schemas from directory {missing}. open {missing}: {}",
            "no such file or directory"
        );
        let bad_name = TempDir::new("badname", &[("anyBidder.json", "{}")])?;
        let bad_name_error = format!(
            "File {}/anyBidder.json does not match a valid BidderName.",
            bad_name.0
        );
        let bad_schema = TempDir::new("badschema", &[("appnexus.json", r#"{"type":1}"#)])?;
        let abs = go_abs(&format!("{}/appnexus.json", bad_schema.0))?;
        let bad_schema_error = format!(
            "Failed to load json schema at {abs}: {}",
            "Invalid type. Expected: string/array of strings, given: type"
        );
        for (dir, want) in [
            (&missing, missing_error),
            (&bad_name.0, bad_name_error),
            (&bad_schema.0, bad_schema_error),
        ] {
            match new_bidder_params_validator(dir, &bidders(), &aliases()) {
                Err(e) if e.message() == want => {}
                other => return Err(format!("{dir}: got {other:?}, want {want}")),
            }
        }
        Ok(())
    }

    #[test]
    fn clean_matches_go() -> Result<(), String> {
        let cases = [
            ("/a/b/../c", "/a/c"),
            ("/a//b/./c/", "/a/b/c"),
            ("/../a", "/a"),
            ("a/../..", ".."),
            ("", "."),
        ];
        for (input, want) in cases {
            let got = go_clean(input);
            if got != want {
                return Err(format!("Clean({input:?}) = {got:?}, want {want:?}"));
            }
        }
        Ok(())
    }

    #[test]
    fn reference_loader_path_matches_go() -> Result<(), String> {
        let cases = [
            (
                "/srv/static/bidder-params/a.json",
                "//srv/static/bidder-params/a.json",
            ),
            ("/srv/a+b/x.json", "//srv/a b/x.json"),
            ("/srv/a%41/x.json", "//srv/aA/x.json"),
            ("/srv/a#b/x.json", "//srv/a"),
        ];
        for (input, want) in cases {
            let got = go_reference_loader_path(input)?;
            if got != want {
                return Err(format!("{input:?}: got {got:?}, want {want:?}"));
            }
        }
        Ok(())
    }
}

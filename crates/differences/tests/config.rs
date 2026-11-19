//! Pins of the `config` crate against Viper as `config.SetupViper` sets it up: `PBS_` variables
//! and `pbs.yaml` values read into a `bool`, and key case in `pbs.yaml`
//! (`testdata/golden/config.json`).

#[cfg(test)]
mod tests {
    use std::error::Error;

    use config::{
        Config, ConfigBuilder, Environment, File, FileStoredFormat, Format, Map, Value,
        builder::DefaultState,
    };
    use pbs_differences::{
        BoxError, TestResult, check_eq, check_table, golden, list, number, text, yaml_options,
    };
    use serde::Deserialize;

    /// YAML for `config` through `serde-saphyr`: the crate's own `yaml` feature would bring a
    /// second hashbrown (the workspace manifest's note on `config`).
    #[derive(Debug, Clone, Copy)]
    struct YamlFormat;

    impl Format for YamlFormat {
        fn parse(
            &self,
            _uri: Option<&String>,
            text: &str,
        ) -> Result<Map<String, Value>, Box<dyn Error + Send + Sync>> {
            Ok(serde_saphyr::from_str_with_options(text, yaml_options())?)
        }
    }

    impl FileStoredFormat for YamlFormat {
        fn file_extensions(&self) -> &'static [&'static str] {
            &["yaml", "yml"]
        }
    }

    /// The two keys of `config.Configuration` the pins read.
    #[derive(Debug, Deserialize)]
    struct Host {
        port: u16,
        unix_socket_enable: bool,
    }

    /// The defaults `config.SetupViper` gives the two keys.
    fn defaults() -> Result<ConfigBuilder<DefaultState>, BoxError> {
        Ok(Config::builder()
            .set_default("port", 8000)?
            .set_default("unix_socket_enable", false)?)
    }

    /// Viper reads a `PBS_` variable into a `bool` key with `cast.ToBool`
    /// (`SetTypeByDefaultValue`), which is `strconv.ParseBool` with its error dropped: `1`, `t`,
    /// `T`, `true`, `TRUE` and `True` are true and every other value, `yes` and `on` among them,
    /// is false without an error. `config` takes `1`, `true`, `on` and `yes` (and their false
    /// forms) in any case and fails the load on anything else, `t` and `f` included.
    // Go: config/config.go:948 (the unix_socket_enable default) and config/config.go:1318-1321
    // (SetEnvKeyReplacer, SetTypeByDefaultValue(true), SetEnvPrefix("PBS"), AutomaticEnv), read by
    // config/config.go:758 (v.Unmarshal)
    #[test]
    fn go_difference_env_bool_spellings() -> TestResult {
        let doc = golden("config.json")?;
        let var = text(&doc, "/env_var")?;
        check_eq(var, "PBS_UNIX_SOCKET_ENABLE", "the variable Viper read")?;
        let mut rows = Vec::new();
        for outcome in list(&doc, "/env_bools")? {
            let input = text(outcome, "/input")?;
            let env = Map::from([(var.to_owned(), input.to_owned())]);
            let host = defaults()?
                .add_source(Environment::with_prefix("PBS").source(Some(env)))
                .build()?
                .try_deserialize::<Host>();
            let go = if text(outcome, "/err")?.is_empty() {
                text(outcome, "/value")?.to_owned()
            } else {
                "error".to_owned()
            };
            rows.push(vec![
                input.to_owned(),
                go,
                host.map_or_else(
                    |_| "error".to_owned(),
                    |host| host.unix_socket_enable.to_string(),
                ),
            ]);
        }
        check_table(
            &rows,
            &[
                &["1", "true", "true"],
                &["t", "true", "error"],
                &["T", "true", "error"],
                &["true", "true", "true"],
                &["TRUE", "true", "true"],
                &["True", "true", "true"],
                &["tRuE", "false", "true"],
                &["yes", "false", "true"],
                &["YES", "false", "true"],
                &["on", "false", "true"],
                &["y", "false", "error"],
                &["0", "false", "false"],
                &["f", "false", "error"],
                &["F", "false", "error"],
                &["false", "false", "false"],
                &["FALSE", "false", "false"],
                &["False", "false", "false"],
                &["no", "false", "false"],
                &["off", "false", "false"],
                &["n", "false", "error"],
                &["maybe", "false", "error"],
            ],
            "PBS_UNIX_SOCKET_ENABLE, Viper, config",
        )?;
        let defaulted = defaults()?.build()?.try_deserialize::<Host>()?;
        check_eq(
            &Some(defaulted.unix_socket_enable),
            &doc.pointer("/default_unix_socket_enable")
                .and_then(serde_json::Value::as_bool),
            "the default with no variable",
        )
    }

    /// Viper lower-cases every key, so `Port:`, `port:` and `PORT:` in `pbs.yaml` all set
    /// `port`; `config` keeps file keys as written, so only `port:` does and the others are
    /// ignored (the default stays).
    // Go: config/config.go:936-940 (SetConfigName("pbs") with "." and "/etc/config"),
    // config/config.go:1322 (ReadInConfig) and main.go:59-66 (loadConfig)
    #[test]
    fn go_difference_key_case() -> TestResult {
        let doc = golden("config.json")?;
        check_eq(
            text(&doc, "/config_file")?,
            "pbs.yaml",
            "the file Viper read",
        )?;
        let defaulted = defaults()?.build()?.try_deserialize::<Host>()?;
        check_eq(
            &u64::from(defaulted.port),
            &number(&doc, "/default_port")?,
            "the default port",
        )?;
        let mut rows = Vec::new();
        for outcome in list(&doc, "/file_ports")? {
            let input = text(outcome, "/input")?;
            let host = defaults()?
                .add_source(File::from_str(input, YamlFormat))
                .build()?
                .try_deserialize::<Host>()?;
            rows.push(vec![
                input.trim_end().to_owned(),
                text(outcome, "/value")?.to_owned(),
                host.port.to_string(),
            ]);
        }
        check_table(
            &rows,
            &[
                &["Port: 9300", "9300", "8000"],
                &["port: 9300", "9300", "9300"],
                &["PORT: 9300", "9300", "8000"],
            ],
            "pbs.yaml, Viper's port, config's port",
        )
    }

    /// In `pbs.yaml`, Viper takes the value as yaml.v3 resolves it untyped (`true`, `True` and
    /// `TRUE` and their false forms are booleans, `yes`, `on`, `y` and `tRuE` stay strings, `1`
    /// is a number) and gives it the type of the key's default with `cast.ToBool`, so `t` and
    /// `1` are true and `yes`, `on`, `y`, `tRuE` and `maybe` are false without an error. Through
    /// `serde-saphyr` and `config`, `yes`, `on`, `y` and `tRuE` are true and `t`, `f` and
    /// `maybe` fail the load.
    // Go: config/config.go:948 (the unix_socket_enable default), config/config.go:1319
    // (SetTypeByDefaultValue(true)) and :1322 (ReadInConfig), read by config/config.go:758
    // (v.Unmarshal, whose AllSettings goes through Viper's Get and its cast.ToBool)
    #[test]
    fn go_difference_file_bool_spellings() -> TestResult {
        let doc = golden("config.json")?;
        let mut rows = Vec::new();
        for outcome in list(&doc, "/file_bools")? {
            let input = text(outcome, "/input")?;
            let host = defaults()?
                .add_source(File::from_str(input, YamlFormat))
                .build()?
                .try_deserialize::<Host>();
            let go = if text(outcome, "/err")?.is_empty() {
                text(outcome, "/value")?.to_owned()
            } else {
                "error".to_owned()
            };
            rows.push(vec![
                input.trim_end().to_owned(),
                go,
                host.map_or_else(
                    |_| "error".to_owned(),
                    |host| host.unix_socket_enable.to_string(),
                ),
            ]);
        }
        check_table(
            &rows,
            &[
                &["unix_socket_enable: true", "true", "true"],
                &["unix_socket_enable: True", "true", "true"],
                &["unix_socket_enable: yes", "false", "true"],
                &["unix_socket_enable: on", "false", "true"],
                &["unix_socket_enable: y", "false", "true"],
                &["unix_socket_enable: t", "true", "error"],
                &["unix_socket_enable: 1", "true", "true"],
                &["unix_socket_enable: tRuE", "false", "true"],
                &["unix_socket_enable: false", "false", "false"],
                &["unix_socket_enable: no", "false", "false"],
                &["unix_socket_enable: off", "false", "false"],
                &["unix_socket_enable: n", "false", "false"],
                &["unix_socket_enable: f", "false", "error"],
                &["unix_socket_enable: 0", "false", "false"],
                &["unix_socket_enable: maybe", "false", "error"],
            ],
            "pbs.yaml, Viper, config",
        )
    }
}

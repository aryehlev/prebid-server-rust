package main

import (
	"os"
	"path/filepath"
	"strconv"

	"github.com/spf13/viper"

	"github.com/prebid/prebid-server/v4/config"
)

// hostSettings has the two config.Configuration fields these goldens read, with their
// mapstructure tags (config/config.go:24-26).
type hostSettings struct {
	Port             int  `mapstructure:"port"`
	UnixSocketEnable bool `mapstructure:"unix_socket_enable"`
}

// loadHost sets Viper up as main.go's loadConfig does (config.SetupViper, which reads the
// environment and the file) and decodes the two fields with Viper's Unmarshal and the decode
// hook config.New uses (config/config.go:756-760).
func loadHost(filename string) (hostSettings, error) {
	v := viper.New()
	config.SetupViper(v, filename, nil)
	var h hostSettings
	err := v.Unmarshal(&h, viper.DecodeHook(config.AccountModulesHookFunc()))
	return h, err
}

// configOutcome is what the configuration held after loading: the field, or the error text.
type configOutcome struct {
	Input string `json:"input"`
	Value string `json:"value"`
	Err   string `json:"err"`
}

// envBoolSpellings are values of PBS_UNIX_SOCKET_ENABLE: strconv.ParseBool's spellings, the
// YAML 1.1 ones, mixed case and a word that is no boolean at all.
var envBoolSpellings = []string{
	"1", "t", "T", "true", "TRUE", "True", "tRuE", "yes", "YES", "on", "y",
	"0", "f", "F", "false", "FALSE", "False", "no", "off", "n", "maybe",
}

// fileBoolSpellings are values of unix_socket_enable in pbs.yaml: YAML's booleans, the YAML 1.1
// words, strconv.ParseBool's short forms, numbers, mixed case and a word that is no boolean.
var fileBoolSpellings = []string{
	"true", "True", "yes", "on", "y", "t", "1", "tRuE",
	"false", "no", "off", "n", "f", "0", "maybe",
}

type configDoc struct {
	Header        header          `json:"header"`
	EnvVar        string          `json:"env_var"`
	EnvBools      []configOutcome `json:"env_bools"`
	ConfigFile    string          `json:"config_file"`
	FilePorts     []configOutcome `json:"file_ports"`
	FileBools     []configOutcome `json:"file_bools"`
	DefaultPort   int             `json:"default_port"`
	DefaultSocket bool            `json:"default_unix_socket_enable"`
}

func configGolden() configDoc {
	doc := configDoc{
		Header: newHeader("github.com/spf13/viper", "github.com/spf13/cast",
			"github.com/mitchellh/mapstructure"),
		EnvVar:     "PBS_UNIX_SOCKET_ENABLE",
		ConfigFile: "pbs.yaml",
	}
	defaults, err := loadHost("")
	if err != nil {
		panic(err)
	}
	doc.DefaultPort = defaults.Port
	doc.DefaultSocket = defaults.UnixSocketEnable

	for _, s := range envBoolSpellings {
		if err := os.Setenv(doc.EnvVar, s); err != nil {
			panic(err)
		}
		out := configOutcome{Input: s}
		if h, err := loadHost(""); err != nil {
			out.Err = err.Error()
		} else {
			out.Value = strconv.FormatBool(h.UnixSocketEnable)
		}
		doc.EnvBools = append(doc.EnvBools, out)
	}
	if err := os.Unsetenv(doc.EnvVar); err != nil {
		panic(err)
	}

	for _, file := range []string{"Port: 9300\n", "port: 9300\n", "PORT: 9300\n"} {
		out := configOutcome{Input: file}
		if h, err := loadFromFile(doc.ConfigFile, file); err != nil {
			out.Err = err.Error()
		} else {
			out.Value = strconv.Itoa(h.Port)
		}
		doc.FilePorts = append(doc.FilePorts, out)
	}
	for _, s := range fileBoolSpellings {
		file := "unix_socket_enable: " + s + "\n"
		out := configOutcome{Input: file}
		if h, err := loadFromFile(doc.ConfigFile, file); err != nil {
			out.Err = err.Error()
		} else {
			out.Value = strconv.FormatBool(h.UnixSocketEnable)
		}
		doc.FileBools = append(doc.FileBools, out)
	}
	return doc
}

// loadFromFile writes contents as name into a fresh directory, loads the configuration with
// that directory as the working directory (SetupViper searches it, and /etc/config, for pbs.*)
// and removes the directory again.
func loadFromFile(name, contents string) (hostSettings, error) {
	wd, err := os.Getwd()
	if err != nil {
		panic(err)
	}
	dir, err := os.MkdirTemp("", "pbs-differences-")
	if err != nil {
		panic(err)
	}
	if err := os.WriteFile(filepath.Join(dir, name), []byte(contents), 0o600); err != nil {
		panic(err)
	}
	if err := os.Chdir(dir); err != nil {
		panic(err)
	}
	h, loadErr := loadHost("pbs")
	if err := os.Chdir(wd); err != nil {
		panic(err)
	}
	if err := os.RemoveAll(dir); err != nil {
		panic(err)
	}
	return h, loadErr
}

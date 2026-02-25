package config

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"gopkg.in/yaml.v3"
)

type GlobalFlags struct {
	ConfigPath     string
	JSON           bool
	Plain          bool
	Select         string
	ResultsOnly    bool
	EnableCommands string
	Strict         bool
	Timeout        string
	Retries        int
	MaxStale       string
	NoStale        bool
	NoCache        bool
}

type Settings struct {
	OutputMode      string
	SelectFields    []string
	ResultsOnly     bool
	EnableCommands  []string
	Strict          bool
	Timeout         time.Duration
	Retries         int
	MaxStale        time.Duration
	NoStale         bool
	CacheEnabled    bool
	CachePath       string
	CacheLockPath   string
	ActionStorePath string
	ActionLockPath  string
	DefiLlamaAPIKey string
	UniswapAPIKey   string
	OneInchAPIKey   string
	JupiterAPIKey   string
	BungeeAPIKey    string
	BungeeAffiliate string
}

type fileConfig struct {
	Output  string `yaml:"output"`
	Strict  *bool  `yaml:"strict"`
	Timeout string `yaml:"timeout"`
	Retries *int   `yaml:"retries"`
	Cache   struct {
		Enabled  *bool  `yaml:"enabled"`
		MaxStale string `yaml:"max_stale"`
		Path     string `yaml:"path"`
		LockPath string `yaml:"lock_path"`
	} `yaml:"cache"`
	Execution struct {
		ActionsPath     string `yaml:"actions_path"`
		ActionsLockPath string `yaml:"actions_lock_path"`
	} `yaml:"execution"`
	Providers struct {
		DefiLlama struct {
			APIKey    string `yaml:"api_key"`
			APIKeyEnv string `yaml:"api_key_env"`
		} `yaml:"defillama"`
		Uniswap struct {
			APIKey    string `yaml:"api_key"`
			APIKeyEnv string `yaml:"api_key_env"`
		} `yaml:"uniswap"`
		OneInch struct {
			APIKey    string `yaml:"api_key"`
			APIKeyEnv string `yaml:"api_key_env"`
		} `yaml:"oneinch"`
		Jupiter struct {
			APIKey    string `yaml:"api_key"`
			APIKeyEnv string `yaml:"api_key_env"`
		} `yaml:"jupiter"`
		Bungee struct {
			APIKey       string `yaml:"api_key"`
			APIKeyEnv    string `yaml:"api_key_env"`
			Affiliate    string `yaml:"affiliate"`
			AffiliateEnv string `yaml:"affiliate_env"`
		} `yaml:"bungee"`
	} `yaml:"providers"`
}

func Load(flags GlobalFlags) (Settings, error) {
	settings, err := defaultSettings()
	if err != nil {
		return Settings{}, err
	}

	cfgPath, err := resolveConfigPath(flags.ConfigPath)
	if err != nil {
		return Settings{}, err
	}

	if err := applyFileConfig(cfgPath, &settings); err != nil {
		return Settings{}, err
	}

	applyEnv(&settings)

	if err := applyFlags(flags, &settings); err != nil {
		return Settings{}, err
	}

	if settings.OutputMode == "" {
		settings.OutputMode = "json"
	}
	if settings.Timeout <= 0 {
		settings.Timeout = 10 * time.Second
	}
	if settings.Retries < 0 {
		settings.Retries = 0
	}
	if settings.MaxStale < 0 {
		settings.MaxStale = 5 * time.Minute
	}

	return settings, nil
}

func defaultSettings() (Settings, error) {
	cachePath, lockPath, err := defaultCachePaths()
	if err != nil {
		return Settings{}, err
	}
	cacheDir := filepath.Dir(cachePath)
	return Settings{
		OutputMode:      "json",
		Timeout:         10 * time.Second,
		Retries:         2,
		MaxStale:        5 * time.Minute,
		CacheEnabled:    true,
		CachePath:       cachePath,
		CacheLockPath:   lockPath,
		ActionStorePath: filepath.Join(cacheDir, "actions.db"),
		ActionLockPath:  filepath.Join(cacheDir, "actions.lock"),
	}, nil
}

func resolveConfigPath(input string) (string, error) {
	if strings.TrimSpace(input) != "" {
		return input, nil
	}
	base := os.Getenv("XDG_CONFIG_HOME")
	if base == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", err
		}
		base = filepath.Join(home, ".config")
	}
	return filepath.Join(base, "defi", "config.yaml"), nil
}

func defaultCachePaths() (string, string, error) {
	base := os.Getenv("XDG_CACHE_HOME")
	if base == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", "", err
		}
		base = filepath.Join(home, ".cache")
	}
	dir := filepath.Join(base, "defi")
	return filepath.Join(dir, "cache.db"), filepath.Join(dir, "cache.lock"), nil
}

func applyFileConfig(path string, settings *Settings) error {
	buf, err := os.ReadFile(path)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return nil
		}
		return fmt.Errorf("read config: %w", err)
	}

	var cfg fileConfig
	if err := yaml.Unmarshal(buf, &cfg); err != nil {
		return fmt.Errorf("parse config yaml: %w", err)
	}

	if cfg.Output != "" {
		settings.OutputMode = strings.ToLower(cfg.Output)
	}
	if cfg.Strict != nil {
		settings.Strict = *cfg.Strict
	}
	if cfg.Timeout != "" {
		d, err := time.ParseDuration(cfg.Timeout)
		if err != nil {
			return fmt.Errorf("config timeout: %w", err)
		}
		settings.Timeout = d
	}
	if cfg.Retries != nil {
		settings.Retries = *cfg.Retries
	}
	if cfg.Cache.Enabled != nil {
		settings.CacheEnabled = *cfg.Cache.Enabled
	}
	if cfg.Cache.MaxStale != "" {
		d, err := time.ParseDuration(cfg.Cache.MaxStale)
		if err != nil {
			return fmt.Errorf("config cache.max_stale: %w", err)
		}
		settings.MaxStale = d
	}
	if cfg.Cache.Path != "" {
		settings.CachePath = cfg.Cache.Path
	}
	if cfg.Cache.LockPath != "" {
		settings.CacheLockPath = cfg.Cache.LockPath
	}
	if cfg.Execution.ActionsPath != "" {
		settings.ActionStorePath = cfg.Execution.ActionsPath
	}
	if cfg.Execution.ActionsLockPath != "" {
		settings.ActionLockPath = cfg.Execution.ActionsLockPath
	}
	if cfg.Providers.Uniswap.APIKey != "" {
		settings.UniswapAPIKey = cfg.Providers.Uniswap.APIKey
	}
	if cfg.Providers.DefiLlama.APIKey != "" {
		settings.DefiLlamaAPIKey = cfg.Providers.DefiLlama.APIKey
	}
	if cfg.Providers.DefiLlama.APIKeyEnv != "" {
		settings.DefiLlamaAPIKey = os.Getenv(cfg.Providers.DefiLlama.APIKeyEnv)
	}
	if cfg.Providers.Uniswap.APIKeyEnv != "" {
		settings.UniswapAPIKey = os.Getenv(cfg.Providers.Uniswap.APIKeyEnv)
	}
	if cfg.Providers.OneInch.APIKey != "" {
		settings.OneInchAPIKey = cfg.Providers.OneInch.APIKey
	}
	if cfg.Providers.OneInch.APIKeyEnv != "" {
		settings.OneInchAPIKey = os.Getenv(cfg.Providers.OneInch.APIKeyEnv)
	}
	if cfg.Providers.Jupiter.APIKey != "" {
		settings.JupiterAPIKey = cfg.Providers.Jupiter.APIKey
	}
	if cfg.Providers.Jupiter.APIKeyEnv != "" {
		settings.JupiterAPIKey = os.Getenv(cfg.Providers.Jupiter.APIKeyEnv)
	}
	if cfg.Providers.Bungee.APIKey != "" {
		settings.BungeeAPIKey = cfg.Providers.Bungee.APIKey
	}
	if cfg.Providers.Bungee.APIKeyEnv != "" {
		settings.BungeeAPIKey = os.Getenv(cfg.Providers.Bungee.APIKeyEnv)
	}
	if cfg.Providers.Bungee.Affiliate != "" {
		settings.BungeeAffiliate = cfg.Providers.Bungee.Affiliate
	}
	if cfg.Providers.Bungee.AffiliateEnv != "" {
		settings.BungeeAffiliate = os.Getenv(cfg.Providers.Bungee.AffiliateEnv)
	}

	return nil
}

func applyEnv(settings *Settings) {
	if v := os.Getenv("DEFI_OUTPUT"); v != "" {
		settings.OutputMode = strings.ToLower(v)
	}
	if v := os.Getenv("DEFI_STRICT"); v != "" {
		if b, err := strconv.ParseBool(v); err == nil {
			settings.Strict = b
		}
	}
	if v := os.Getenv("DEFI_TIMEOUT"); v != "" {
		if d, err := time.ParseDuration(v); err == nil {
			settings.Timeout = d
		}
	}
	if v := os.Getenv("DEFI_RETRIES"); v != "" {
		if n, err := strconv.Atoi(v); err == nil {
			settings.Retries = n
		}
	}
	if v := os.Getenv("DEFI_MAX_STALE"); v != "" {
		if d, err := time.ParseDuration(v); err == nil {
			settings.MaxStale = d
		}
	}
	if v := os.Getenv("DEFI_NO_STALE"); v != "" {
		if b, err := strconv.ParseBool(v); err == nil {
			settings.NoStale = b
		}
	}
	if v := os.Getenv("DEFI_NO_CACHE"); v != "" {
		if b, err := strconv.ParseBool(v); err == nil {
			settings.CacheEnabled = !b
		}
	}
	if v := os.Getenv("DEFI_CACHE_PATH"); v != "" {
		settings.CachePath = v
	}
	if v := os.Getenv("DEFI_CACHE_LOCK_PATH"); v != "" {
		settings.CacheLockPath = v
	}
	if v := os.Getenv("DEFI_ACTIONS_PATH"); v != "" {
		settings.ActionStorePath = v
	}
	if v := os.Getenv("DEFI_ACTIONS_LOCK_PATH"); v != "" {
		settings.ActionLockPath = v
	}
	if v := os.Getenv("DEFI_UNISWAP_API_KEY"); v != "" {
		settings.UniswapAPIKey = v
	}
	if v := os.Getenv("DEFI_DEFILLAMA_API_KEY"); v != "" {
		settings.DefiLlamaAPIKey = v
	}
	if v := os.Getenv("DEFI_1INCH_API_KEY"); v != "" {
		settings.OneInchAPIKey = v
	}
	if v := os.Getenv("DEFI_JUPITER_API_KEY"); v != "" {
		settings.JupiterAPIKey = v
	}
	if v := os.Getenv("DEFI_BUNGEE_API_KEY"); v != "" {
		settings.BungeeAPIKey = v
	}
	if v := os.Getenv("DEFI_BUNGEE_AFFILIATE"); v != "" {
		settings.BungeeAffiliate = v
	}
}

func applyFlags(flags GlobalFlags, settings *Settings) error {
	if flags.JSON && flags.Plain {
		return fmt.Errorf("cannot use --json and --plain together")
	}
	if flags.JSON {
		settings.OutputMode = "json"
	}
	if flags.Plain {
		settings.OutputMode = "plain"
	}
	if strings.TrimSpace(flags.Select) != "" {
		parts := strings.Split(flags.Select, ",")
		fields := make([]string, 0, len(parts))
		for _, part := range parts {
			f := strings.TrimSpace(part)
			if f != "" {
				fields = append(fields, f)
			}
		}
		settings.SelectFields = fields
	}
	settings.ResultsOnly = flags.ResultsOnly

	if strings.TrimSpace(flags.EnableCommands) != "" {
		parts := strings.Split(flags.EnableCommands, ",")
		allowed := make([]string, 0, len(parts))
		for _, part := range parts {
			v := strings.TrimSpace(part)
			if v != "" {
				allowed = append(allowed, v)
			}
		}
		settings.EnableCommands = allowed
	}

	if flags.Strict {
		settings.Strict = true
	}
	if flags.Timeout != "" {
		d, err := time.ParseDuration(flags.Timeout)
		if err != nil {
			return fmt.Errorf("parse --timeout: %w", err)
		}
		settings.Timeout = d
	}
	if flags.Retries >= 0 {
		settings.Retries = flags.Retries
	}
	if flags.MaxStale != "" {
		d, err := time.ParseDuration(flags.MaxStale)
		if err != nil {
			return fmt.Errorf("parse --max-stale: %w", err)
		}
		settings.MaxStale = d
	}
	if flags.NoStale {
		settings.NoStale = true
	}
	if flags.NoCache {
		settings.CacheEnabled = false
	}

	if settings.OutputMode != "json" && settings.OutputMode != "plain" {
		return fmt.Errorf("output must be json or plain")
	}

	return nil
}

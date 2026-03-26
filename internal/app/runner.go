package app

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"math/big"
	"os"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/ethclient"
	"github.com/ggonzalez94/defi-cli/internal/cache"
	"github.com/ggonzalez94/defi-cli/internal/config"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/execution/actionbuilder"
	execsigner "github.com/ggonzalez94/defi-cli/internal/execution/signer"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/out"
	"github.com/ggonzalez94/defi-cli/internal/policy"
	"github.com/ggonzalez94/defi-cli/internal/providers"
	"github.com/ggonzalez94/defi-cli/internal/providers/aave"
	"github.com/ggonzalez94/defi-cli/internal/providers/across"
	"github.com/ggonzalez94/defi-cli/internal/providers/bungee"
	"github.com/ggonzalez94/defi-cli/internal/providers/defillama"
	"github.com/ggonzalez94/defi-cli/internal/providers/fibrous"
	"github.com/ggonzalez94/defi-cli/internal/providers/jupiter"
	"github.com/ggonzalez94/defi-cli/internal/providers/kamino"
	"github.com/ggonzalez94/defi-cli/internal/providers/lifi"
	"github.com/ggonzalez94/defi-cli/internal/providers/moonwell"
	"github.com/ggonzalez94/defi-cli/internal/providers/morpho"
	"github.com/ggonzalez94/defi-cli/internal/providers/oneinch"
	"github.com/ggonzalez94/defi-cli/internal/providers/taikoswap"
	"github.com/ggonzalez94/defi-cli/internal/providers/tempo"
	"github.com/ggonzalez94/defi-cli/internal/providers/uniswap"
	"github.com/ggonzalez94/defi-cli/internal/registry"
	"github.com/ggonzalez94/defi-cli/internal/schema"
	"github.com/ggonzalez94/defi-cli/internal/version"
	"github.com/spf13/cobra"
)

type Runner struct {
	stdout io.Writer
	stderr io.Writer
	now    func() time.Time
}

func NewRunner() *Runner {
	return NewRunnerWithWriters(os.Stdout, os.Stderr)
}

func NewRunnerWithWriters(stdout, stderr io.Writer) *Runner {
	return &Runner{
		stdout: stdout,
		stderr: stderr,
		now:    time.Now,
	}
}

type runtimeState struct {
	runner        *Runner
	flags         config.GlobalFlags
	settings      config.Settings
	cache         *cache.Store
	actionStore   *execution.Store
	actionBuilder *actionbuilder.Registry
	root          *cobra.Command
	lastCommand   string
	lastWarnings  []string
	lastProviders []model.ProviderStatus
	lastPartial   bool

	marketProvider      providers.MarketDataProvider
	lendingProviders    map[string]providers.LendingProvider
	yieldProviders      map[string]providers.YieldProvider
	bridgeProviders     map[string]providers.BridgeProvider
	bridgeDataProviders map[string]providers.BridgeDataProvider
	swapProviders       map[string]providers.SwapProvider
	providerInfos       []model.ProviderInfo
}

const cachePayloadSchemaVersion = "v2"

func (r *Runner) Run(args []string) int {
	state := &runtimeState{runner: r}
	root := state.newRootCommand()
	state.root = root
	state.resetCommandDiagnostics()
	root.SetArgs(args)
	root.SetOut(r.stdout)
	root.SetErr(r.stderr)
	root.SilenceUsage = true
	root.SilenceErrors = true

	err := root.Execute()
	err = normalizeRunError(err)
	if err == nil {
		if state.cache != nil {
			_ = state.cache.Close()
		}
		if state.actionStore != nil {
			_ = state.actionStore.Close()
		}
		return 0
	}

	state.renderError("", err, state.lastWarnings, state.lastProviders, state.lastPartial)
	if state.cache != nil {
		_ = state.cache.Close()
	}
	if state.actionStore != nil {
		_ = state.actionStore.Close()
	}
	return clierr.ExitCode(err)
}

func (s *runtimeState) newRootCommand() *cobra.Command {
	cmd := &cobra.Command{
		Use:   version.CLIName,
		Short: "Agent-first DeFi retrieval CLI",
		PersistentPreRunE: func(cmd *cobra.Command, args []string) error {
			if cmd.Name() == "help" {
				return nil
			}
			if !commandUsesStructuredInput(cmd) {
				if err := normalizeAndValidateCommandFlags(cmd); err != nil {
					return err
				}
			}
			settings, err := config.Load(s.flags)
			if err != nil {
				return clierr.Wrap(clierr.CodeUsage, "load configuration", err)
			}
			s.settings = settings

			path := trimRootPath(cmd.CommandPath())
			s.lastCommand = path
			if err := policy.CheckCommandAllowed(settings.EnableCommands, path); err != nil {
				return err
			}

			if s.marketProvider == nil {
				httpClient := httpx.New(settings.Timeout, settings.Retries)
				llama := defillama.New(httpClient, settings.DefiLlamaAPIKey)
				aaveProvider := aave.New(httpClient)
				morphoProvider := morpho.New(httpClient)
				kaminoProvider := kamino.New(httpClient)
				moonwellProvider := moonwell.New()
				jupiterProvider := jupiter.New(httpClient, settings.JupiterAPIKey)
				tempoProvider := tempo.New()
				taikoSwapProvider := taikoswap.New()
				s.marketProvider = llama
				s.lendingProviders = map[string]providers.LendingProvider{
					"aave":     aaveProvider,
					"morpho":   morphoProvider,
					"kamino":   kaminoProvider,
					"moonwell": moonwellProvider,
				}
				s.yieldProviders = map[string]providers.YieldProvider{
					"aave":     aaveProvider,
					"morpho":   morphoProvider,
					"kamino":   kaminoProvider,
					"moonwell": moonwellProvider,
				}

				s.bridgeProviders = map[string]providers.BridgeProvider{
					"across": across.New(httpClient),
					"lifi":   lifi.New(httpClient),
					"bungee": bungee.NewBridge(httpClient, settings.BungeeAPIKey, settings.BungeeAffiliate),
				}
				s.bridgeDataProviders = map[string]providers.BridgeDataProvider{
					"defillama": llama,
				}
				s.swapProviders = map[string]providers.SwapProvider{
					"1inch":     oneinch.New(httpClient, settings.OneInchAPIKey),
					"uniswap":   uniswap.New(httpClient, settings.UniswapAPIKey),
					"tempo":     tempoProvider,
					"taikoswap": taikoSwapProvider,
					"jupiter":   jupiterProvider,
					"bungee":    bungee.NewSwap(httpClient, settings.BungeeAPIKey, settings.BungeeAffiliate),
					"fibrous":   fibrous.New(httpClient),
				}
				s.providerInfos = []model.ProviderInfo{
					llama.Info(),
					aaveProvider.Info(),
					morphoProvider.Info(),
					kaminoProvider.Info(),
					moonwellProvider.Info(),
					s.bridgeProviders["across"].Info(),
					s.bridgeProviders["lifi"].Info(),
					s.bridgeProviders["bungee"].Info(),
					s.swapProviders["1inch"].Info(),
					s.swapProviders["uniswap"].Info(),
					s.swapProviders["tempo"].Info(),
					s.swapProviders["taikoswap"].Info(),
					s.swapProviders["jupiter"].Info(),
					s.swapProviders["bungee"].Info(),
					s.swapProviders["fibrous"].Info(),
				}
			}
			if s.actionBuilder == nil {
				s.actionBuilder = actionbuilder.New(s.swapProviders, s.bridgeProviders)
			} else {
				s.actionBuilder.Configure(s.swapProviders, s.bridgeProviders)
			}

			if settings.CacheEnabled && shouldOpenCache(path) && s.cache == nil {
				cacheStore, err := cache.Open(settings.CachePath, settings.CacheLockPath, settings.MaxStale)
				if err != nil {
					// Cache should be best-effort; continue without it if initialization fails.
					s.settings.CacheEnabled = false
				} else {
					s.cache = cacheStore
				}
			}
			if shouldOpenActionStore(path) && s.actionStore == nil {
				actionStore, err := execution.OpenStore(settings.ActionStorePath, settings.ActionLockPath)
				if err != nil {
					return clierr.Wrap(clierr.CodeInternal, "open action store", err)
				}
				s.actionStore = actionStore
			}
			return nil
		},
	}
	cmd.SetFlagErrorFunc(func(_ *cobra.Command, err error) error {
		return clierr.Wrap(clierr.CodeUsage, "parse flags", err)
	})

	cmd.PersistentFlags().BoolVar(&s.flags.JSON, "json", false, "Output JSON (default)")
	cmd.PersistentFlags().BoolVar(&s.flags.Plain, "plain", false, "Output plain text")
	cmd.PersistentFlags().StringVar(&s.flags.Select, "select", "", "Select fields from data (comma-separated)")
	cmd.PersistentFlags().BoolVar(&s.flags.ResultsOnly, "results-only", false, "Output only data payload")
	cmd.PersistentFlags().StringVar(&s.flags.EnableCommands, "enable-commands", "", "Allowlist command paths (comma-separated)")
	cmd.PersistentFlags().BoolVar(&s.flags.Strict, "strict", false, "Fail on partial results")
	cmd.PersistentFlags().StringVar(&s.flags.Timeout, "timeout", "", "Provider request timeout")
	cmd.PersistentFlags().IntVar(&s.flags.Retries, "retries", -1, "Retries per provider request")
	cmd.PersistentFlags().StringVar(&s.flags.MaxStale, "max-stale", "", "Maximum stale fallback window after TTL expiry")
	cmd.PersistentFlags().BoolVar(&s.flags.NoStale, "no-stale", false, "Reject stale cache entries")
	cmd.PersistentFlags().BoolVar(&s.flags.NoCache, "no-cache", false, "Disable cache reads and writes")
	cmd.PersistentFlags().StringVar(&s.flags.ConfigPath, "config", "", "Path to config file")
	_ = schema.SetFlagMetadata(cmd.PersistentFlags(), "config", schema.FlagMetadata{Format: "path"})

	cmd.AddCommand(s.newSchemaCommand())
	cmd.AddCommand(s.newProvidersCommand())
	cmd.AddCommand(s.newChainsCommand())
	cmd.AddCommand(s.newProtocolsCommand())
	cmd.AddCommand(s.newDexesCommand())
	cmd.AddCommand(s.newStablecoinsCommand())
	cmd.AddCommand(s.newAssetsCommand())
	cmd.AddCommand(s.newLendCommand())
	cmd.AddCommand(s.newRewardsCommand())
	cmd.AddCommand(s.newBridgeCommand())
	cmd.AddCommand(s.newSwapCommand())
	cmd.AddCommand(s.newApprovalsCommand())
	cmd.AddCommand(s.newTransferCommand())
	cmd.AddCommand(s.newActionsCommand())
	cmd.AddCommand(s.newYieldCommand())
	cmd.AddCommand(s.newWalletCommand())
	cmd.AddCommand(newVersionCommand())

	return cmd
}

func newVersionCommand() *cobra.Command {
	var long bool
	cmd := &cobra.Command{
		Use:   "version",
		Short: "Print CLI version",
		Run: func(cmd *cobra.Command, args []string) {
			if long {
				_, _ = fmt.Fprintln(cmd.OutOrStdout(), version.Long())
				return
			}
			_, _ = fmt.Fprintln(cmd.OutOrStdout(), version.CLIVersion)
		},
	}
	cmd.Flags().BoolVar(&long, "long", false, "Print extended build metadata")
	return cmd
}

func (s *runtimeState) newSchemaCommand() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "schema [command path]",
		Short: "Print machine-readable command schema",
		Args:  cobra.ArbitraryArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			path := ""
			if len(args) > 0 {
				path = strings.Join(args, " ")
			}
			data, err := schema.Build(s.root, path)
			if err != nil {
				return clierr.Wrap(clierr.CodeUsage, "build schema", err)
			}
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), data, nil, cacheMetaBypass(), nil, false)
		},
	}
	schemaResponse := schema.TypeSchema{Type: "object", Description: "Machine-readable command schema document"}
	_ = schema.SetCommandMetadata(cmd, schema.CommandMetadata{Response: &schemaResponse})
	return cmd
}

func (s *runtimeState) newProvidersCommand() *cobra.Command {
	root := &cobra.Command{Use: "providers", Short: "Provider commands"}
	list := &cobra.Command{
		Use:   "list",
		Short: "List supported providers and API key metadata (no keys required)",
		RunE: func(cmd *cobra.Command, args []string) error {
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), s.providerInfos, nil, cacheMetaBypass(), nil, false)
		},
	}
	providersResponse := schema.SchemaFromType([]model.ProviderInfo{})
	_ = schema.SetCommandMetadata(list, schema.CommandMetadata{Response: &providersResponse})
	root.AddCommand(list)
	return root
}

func (s *runtimeState) newChainsCommand() *cobra.Command {
	root := &cobra.Command{Use: "chains", Short: "Chain market data"}

	listCmd := &cobra.Command{
		Use:   "list",
		Short: "List all supported chains with aliases (no keys required)",
		RunE: func(cmd *cobra.Command, args []string) error {
			entries := id.ListChains()
			result := make([]model.SupportedChain, 0, len(entries))
			for _, e := range entries {
				result = append(result, model.SupportedChain{
					Name:       e.Chain.Name,
					Slug:       e.Chain.Slug,
					CAIP2:      e.Chain.CAIP2,
					Namespace:  e.Chain.Namespace(),
					EVMChainID: e.Chain.EVMChainID,
					Aliases:    e.Aliases,
				})
			}
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), result, nil, cacheMetaBypass(), nil, false)
		},
	}
	listResponse := schema.SchemaFromType([]model.SupportedChain{})
	_ = schema.SetCommandMetadata(listCmd, schema.CommandMetadata{Response: &listResponse})
	root.AddCommand(listCmd)

	var limit int
	topCmd := &cobra.Command{
		Use:   "top",
		Short: "Top chains by TVL",
		RunE: func(cmd *cobra.Command, args []string) error {
			req := map[string]any{"limit": limit}
			key := cacheKey(trimRootPath(cmd.CommandPath()), req)
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 5*time.Minute, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				start := time.Now()
				data, err := s.marketProvider.ChainsTop(ctx, limit)
				status := []model.ProviderStatus{{Name: s.marketProvider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, status, nil, false, err
			})
		},
	}
	topCmd.Flags().IntVar(&limit, "limit", 20, "Number of chains to return")
	root.AddCommand(topCmd)

	var assetsChainArg string
	var assetsArg string
	var assetsLimit int
	assetsCmd := &cobra.Command{
		Use:   "assets",
		Short: "TVL by asset for a chain (DefiLlama key required)",
		RunE: func(cmd *cobra.Command, args []string) error {
			chain, err := id.ParseChain(assetsChainArg)
			if err != nil {
				return err
			}

			asset, err := parseChainAssetFilter(chain, assetsArg)
			if err != nil {
				return err
			}

			req := map[string]any{
				"chain": chain.CAIP2,
				"asset": chainAssetFilterCacheValue(asset, assetsArg),
				"limit": assetsLimit,
			}
			key := cacheKey(trimRootPath(cmd.CommandPath()), req)
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 5*time.Minute, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				start := time.Now()
				data, err := s.marketProvider.ChainsAssets(ctx, chain, asset, assetsLimit)
				status := []model.ProviderStatus{{Name: s.marketProvider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, status, nil, false, err
			})
		},
	}
	assetsCmd.Flags().StringVar(&assetsChainArg, "chain", "", "Chain id/name/CAIP-2")
	assetsCmd.Flags().StringVar(&assetsArg, "asset", "", "Asset filter (symbol/address/CAIP-19)")
	assetsCmd.Flags().IntVar(&assetsLimit, "limit", 20, "Number of assets to return")
	_ = assetsCmd.MarkFlagRequired("chain")
	assetsResponse := schema.SchemaFromType([]model.ChainAssetTVL{})
	_ = schema.SetCommandMetadata(assetsCmd, schema.CommandMetadata{
		Auth: []schema.AuthRequirement{{
			Kind:        "api_key",
			EnvVars:     []string{"DEFI_DEFILLAMA_API_KEY"},
			Description: "DefiLlama chain asset TVL requires a DefiLlama API key.",
		}},
		Response: &assetsResponse,
	})
	root.AddCommand(assetsCmd)

	var gasChainArg string
	var gasRPCURL string
	gasCmd := &cobra.Command{
		Use:   "gas",
		Short: "Current gas prices for one or more EVM chains (no keys required)",
		RunE: func(cmd *cobra.Command, args []string) error {
			rawChains := strings.Split(gasChainArg, ",")
			var chainArgs []string
			for _, c := range rawChains {
				c = strings.TrimSpace(c)
				if c != "" {
					chainArgs = append(chainArgs, c)
				}
			}
			if len(chainArgs) == 0 {
				return clierr.New(clierr.CodeUsage, "at least one chain is required")
			}

			if len(chainArgs) > 1 && strings.TrimSpace(gasRPCURL) != "" {
				return clierr.New(clierr.CodeUsage, "--rpc-url cannot be used with multiple chains")
			}

			// Parse and validate all chains up front.
			type chainEntry struct {
				chain  id.Chain
				rpcURL string
			}
			entries := make([]chainEntry, 0, len(chainArgs))
			for _, raw := range chainArgs {
				chain, err := id.ParseChain(raw)
				if err != nil {
					return err
				}
				if chain.Namespace() != "eip155" {
					return clierr.New(clierr.CodeUnsupported, "chains gas is only supported for EVM chains: "+raw)
				}
				rpcURL, err := registry.ResolveRPCURL(gasRPCURL, chain.EVMChainID)
				if err != nil {
					return clierr.Wrap(clierr.CodeUnavailable, "resolve rpc for "+raw, err)
				}
				entries = append(entries, chainEntry{chain: chain, rpcURL: rpcURL})
			}

			ctx, cancel := context.WithTimeout(cmd.Context(), s.settings.Timeout)
			defer cancel()

			// Single chain: still returns a one-element array for consistent schema.
			if len(entries) == 1 {
				result, err := fetchGasPrice(ctx, entries[0].chain, entries[0].rpcURL, s.runner.now)
				if err != nil {
					return err
				}
				return s.emitSuccess(trimRootPath(cmd.CommandPath()), []model.GasPrice{result}, nil, cacheMetaBypass(), nil, false)
			}

			// Multiple chains: fetch in parallel, preserve input order.
			type gasResult struct {
				price model.GasPrice
				err   error
			}
			slots := make([]gasResult, len(entries))
			done := make(chan int, len(entries))
			for i, e := range entries {
				go func(idx int, entry chainEntry) {
					price, err := fetchGasPrice(ctx, entry.chain, entry.rpcURL, s.runner.now)
					slots[idx] = gasResult{price: price, err: err}
					done <- idx
				}(i, e)
			}
			for range entries {
				<-done
			}

			prices := make([]model.GasPrice, 0, len(entries))
			var warnings []string
			for i, r := range slots {
				if r.err != nil {
					warnings = append(warnings, fmt.Sprintf("chain %s: %s", entries[i].chain.CAIP2, r.err.Error()))
					continue
				}
				prices = append(prices, r.price)
			}

			if len(prices) == 0 {
				return clierr.New(clierr.CodeUnavailable, "all chains failed; "+strings.Join(warnings, "; "))
			}

			partial := len(warnings) > 0
			if partial && s.settings.Strict {
				return clierr.New(clierr.CodePartialStrict, "partial gas results in strict mode; failures: "+strings.Join(warnings, "; "))
			}
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), prices, warnings, cacheMetaBypass(), nil, partial)
		},
	}
	gasCmd.Flags().StringVar(&gasChainArg, "chain", "", "Chain id/name/CAIP-2 (comma-separated for multiple)")
	gasCmd.Flags().StringVar(&gasRPCURL, "rpc-url", "", "RPC URL override (single chain only)")
	_ = gasCmd.MarkFlagRequired("chain")
	gasResponse := schema.SchemaFromType([]model.GasPrice{})
	_ = schema.SetCommandMetadata(gasCmd, schema.CommandMetadata{Response: &gasResponse})
	root.AddCommand(gasCmd)

	return root
}

func (s *runtimeState) newProtocolsCommand() *cobra.Command {
	root := &cobra.Command{Use: "protocols", Short: "Protocol market data"}
	var limit int
	var category string
	var chain string
	cmd := &cobra.Command{
		Use:   "top",
		Short: "Top protocols by TVL",
		RunE: func(cmd *cobra.Command, args []string) error {
			req := map[string]any{"category": category, "chain": chain, "limit": limit}
			key := cacheKey(trimRootPath(cmd.CommandPath()), req)
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 5*time.Minute, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				start := time.Now()
				data, err := s.marketProvider.ProtocolsTop(ctx, category, chain, limit)
				status := []model.ProviderStatus{{Name: s.marketProvider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, status, nil, false, err
			})
		},
	}
	cmd.Flags().IntVar(&limit, "limit", 20, "Number of protocols to return")
	cmd.Flags().StringVar(&category, "category", "", "Filter by protocol category (e.g. lending)")
	cmd.Flags().StringVar(&chain, "chain", "", "Filter by DefiLlama chain name (e.g. Ethereum, Arbitrum, Polygon)")
	root.AddCommand(cmd)

	catCmd := &cobra.Command{
		Use:   "categories",
		Short: "List protocol categories with protocol counts and TVL",
		RunE: func(cmd *cobra.Command, args []string) error {
			key := cacheKey(trimRootPath(cmd.CommandPath()), map[string]any{})
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 5*time.Minute, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				start := time.Now()
				data, err := s.marketProvider.ProtocolsCategories(ctx)
				status := []model.ProviderStatus{{Name: s.marketProvider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, status, nil, false, err
			})
		},
	}
	root.AddCommand(catCmd)

	var feesLimit int
	var feesCategory string
	var feesChain string
	feesCmd := &cobra.Command{
		Use:   "fees",
		Short: "Top protocols by 24h fees",
		RunE: func(cmd *cobra.Command, args []string) error {
			req := map[string]any{"category": feesCategory, "chain": feesChain, "limit": feesLimit}
			key := cacheKey(trimRootPath(cmd.CommandPath()), req)
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 5*time.Minute, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				start := time.Now()
				data, err := s.marketProvider.ProtocolsFees(ctx, feesCategory, feesChain, feesLimit)
				status := []model.ProviderStatus{{Name: s.marketProvider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, status, nil, false, err
			})
		},
	}
	feesCmd.Flags().IntVar(&feesLimit, "limit", 20, "Number of protocols to return")
	feesCmd.Flags().StringVar(&feesCategory, "category", "", "Filter by protocol category (e.g. Dexs, Lending)")
	feesCmd.Flags().StringVar(&feesChain, "chain", "", "Filter by DefiLlama chain name (e.g. Ethereum, Arbitrum, Polygon)")
	root.AddCommand(feesCmd)

	var revLimit int
	var revCategory string
	var revChain string
	revCmd := &cobra.Command{
		Use:   "revenue",
		Short: "Top protocols by 24h revenue",
		RunE: func(cmd *cobra.Command, args []string) error {
			req := map[string]any{"category": revCategory, "chain": revChain, "limit": revLimit}
			key := cacheKey(trimRootPath(cmd.CommandPath()), req)
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 5*time.Minute, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				start := time.Now()
				data, err := s.marketProvider.ProtocolsRevenue(ctx, revCategory, revChain, revLimit)
				status := []model.ProviderStatus{{Name: s.marketProvider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, status, nil, false, err
			})
		},
	}
	revCmd.Flags().IntVar(&revLimit, "limit", 20, "Number of protocols to return")
	revCmd.Flags().StringVar(&revCategory, "category", "", "Filter by protocol category (e.g. Dexs, Lending)")
	revCmd.Flags().StringVar(&revChain, "chain", "", "Filter by DefiLlama chain name (e.g. Ethereum, Arbitrum, Polygon)")
	root.AddCommand(revCmd)

	return root
}

func (s *runtimeState) newDexesCommand() *cobra.Command {
	root := &cobra.Command{Use: "dexes", Short: "DEX market data"}
	var limit int
	var chain string
	volCmd := &cobra.Command{
		Use:   "volume",
		Short: "Top DEXes by 24h trading volume",
		RunE: func(cmd *cobra.Command, args []string) error {
			req := map[string]any{"chain": chain, "limit": limit}
			key := cacheKey(trimRootPath(cmd.CommandPath()), req)
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 5*time.Minute, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				start := time.Now()
				data, err := s.marketProvider.DexesVolume(ctx, chain, limit)
				status := []model.ProviderStatus{{Name: s.marketProvider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, status, nil, false, err
			})
		},
	}
	volCmd.Flags().IntVar(&limit, "limit", 20, "Number of DEXes to return")
	volCmd.Flags().StringVar(&chain, "chain", "", "Filter by DefiLlama chain name (e.g. Ethereum, Arbitrum, Polygon)")
	root.AddCommand(volCmd)

	return root
}

func (s *runtimeState) newStablecoinsCommand() *cobra.Command {
	root := &cobra.Command{Use: "stablecoins", Short: "Stablecoin market data"}
	var limit int
	var pegType string
	cmd := &cobra.Command{
		Use:   "top",
		Short: "Top stablecoins by circulating market cap",
		RunE: func(cmd *cobra.Command, args []string) error {
			req := map[string]any{"peg_type": pegType, "limit": limit}
			key := cacheKey(trimRootPath(cmd.CommandPath()), req)
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 5*time.Minute, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				start := time.Now()
				data, err := s.marketProvider.StablecoinsTop(ctx, pegType, limit)
				status := []model.ProviderStatus{{Name: s.marketProvider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, status, nil, false, err
			})
		},
	}
	cmd.Flags().IntVar(&limit, "limit", 20, "Number of stablecoins to return")
	cmd.Flags().StringVar(&pegType, "peg-type", "", "Filter by peg type (e.g. peggedUSD, peggedEUR)")
	root.AddCommand(cmd)

	var chainsLimit int
	chainsCmd := &cobra.Command{
		Use:   "chains",
		Short: "Chains ranked by total stablecoin market cap",
		RunE: func(cmd *cobra.Command, args []string) error {
			req := map[string]any{"limit": chainsLimit}
			key := cacheKey(trimRootPath(cmd.CommandPath()), req)
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 5*time.Minute, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				start := time.Now()
				data, err := s.marketProvider.StablecoinChains(ctx, chainsLimit)
				status := []model.ProviderStatus{{Name: s.marketProvider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, status, nil, false, err
			})
		},
	}
	chainsCmd.Flags().IntVar(&chainsLimit, "limit", 20, "Number of chains to return")
	root.AddCommand(chainsCmd)

	return root
}

func (s *runtimeState) newAssetsCommand() *cobra.Command {
	root := &cobra.Command{Use: "assets", Short: "Asset helpers"}
	var chainArg string
	var symbol string
	var input string
	cmd := &cobra.Command{
		Use:   "resolve",
		Short: "Resolve an asset symbol/address/CAIP-19 to canonical asset ID",
		RunE: func(cmd *cobra.Command, args []string) error {
			if chainArg == "" {
				return clierr.New(clierr.CodeUsage, "--chain is required")
			}
			value := input
			if value == "" {
				value = symbol
			}
			if value == "" {
				return clierr.New(clierr.CodeUsage, "--asset or --symbol is required")
			}
			chain, err := id.ParseChain(chainArg)
			if err != nil {
				return err
			}
			asset, err := id.ParseAsset(value, chain)
			if err != nil {
				return err
			}
			result := model.AssetResolution{
				Input:       value,
				ChainID:     chain.CAIP2,
				Symbol:      asset.Symbol,
				AssetID:     asset.AssetID,
				Address:     asset.Address,
				Decimals:    asset.Decimals,
				ResolvedBy:  "registry",
				Unambiguous: true,
			}
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), result, nil, cacheMetaBypass(), nil, false)
		},
	}
	cmd.Flags().StringVar(&chainArg, "chain", "", "Chain identifier (CAIP-2, chain ID, or slug)")
	cmd.Flags().StringVar(&symbol, "symbol", "", "Asset symbol (e.g., USDC)")
	cmd.Flags().StringVar(&input, "asset", "", "Asset as CAIP-19 or token address")
	root.AddCommand(cmd)
	return root
}

func (s *runtimeState) newLendCommand() *cobra.Command {
	root := &cobra.Command{Use: "lend", Short: "Lending data"}
	var providerArg string
	var chainArg string
	var assetArg string
	var marketsLimit int
	var marketsRPCURL string

	marketsCmd := &cobra.Command{
		Use:   "markets",
		Short: "List lending markets",
		RunE: func(cmd *cobra.Command, args []string) error {
			providerName := normalizeLendingProvider(providerArg)
			if providerName == "" {
				return clierr.New(clierr.CodeUsage, "--provider is required")
			}
			chain, asset, err := parseChainAsset(chainArg, assetArg)
			if err != nil {
				return err
			}
			req := map[string]any{"provider": providerName, "chain": chain.CAIP2, "asset": asset.AssetID, "limit": marketsLimit, "rpc_url": strings.TrimSpace(marketsRPCURL)}
			key := cacheKey(trimRootPath(cmd.CommandPath()), req)
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 60*time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				provider, err := s.selectLendingProvider(providerName)
				if err != nil {
					return nil, nil, nil, false, err
				}
				applyRPCOverride(provider, marketsRPCURL)

				start := time.Now()
				data, err := provider.LendMarkets(ctx, providerName, chain, asset)
				statuses := []model.ProviderStatus{{Name: provider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				if err != nil {
					return nil, statuses, nil, false, err
				}
				data = applyLendMarketLimit(data, marketsLimit)
				return data, statuses, nil, false, nil
			})
		},
	}
	marketsCmd.Flags().StringVar(&providerArg, "provider", "", "Lending provider (aave, morpho, kamino, moonwell)")
	marketsCmd.Flags().StringVar(&chainArg, "chain", "", "Chain identifier")
	marketsCmd.Flags().StringVar(&assetArg, "asset", "", "Asset (symbol/address/CAIP-19)")
	marketsCmd.Flags().IntVar(&marketsLimit, "limit", 20, "Maximum lending markets to return")
	marketsCmd.Flags().StringVar(&marketsRPCURL, "rpc-url", "", "Optional RPC URL override for on-chain providers")
	_ = marketsCmd.MarkFlagRequired("provider")
	_ = marketsCmd.MarkFlagRequired("chain")
	_ = marketsCmd.MarkFlagRequired("asset")

	var ratesProvider, ratesChain, ratesAsset string
	var ratesLimit int
	var ratesRPCURL string
	ratesCmd := &cobra.Command{
		Use:   "rates",
		Short: "List lending rates",
		RunE: func(cmd *cobra.Command, args []string) error {
			providerName := normalizeLendingProvider(ratesProvider)
			if providerName == "" {
				return clierr.New(clierr.CodeUsage, "--provider is required")
			}
			chain, asset, err := parseChainAsset(ratesChain, ratesAsset)
			if err != nil {
				return err
			}
			req := map[string]any{"provider": providerName, "chain": chain.CAIP2, "asset": asset.AssetID, "limit": ratesLimit, "rpc_url": strings.TrimSpace(ratesRPCURL)}
			key := cacheKey(trimRootPath(cmd.CommandPath()), req)
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 30*time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				provider, err := s.selectLendingProvider(providerName)
				if err != nil {
					return nil, nil, nil, false, err
				}
				applyRPCOverride(provider, ratesRPCURL)

				start := time.Now()
				data, err := provider.LendRates(ctx, providerName, chain, asset)
				statuses := []model.ProviderStatus{{Name: provider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				if err != nil {
					return nil, statuses, nil, false, err
				}
				data = applyLendRateLimit(data, ratesLimit)
				return data, statuses, nil, false, nil
			})
		},
	}
	ratesCmd.Flags().StringVar(&ratesProvider, "provider", "", "Lending provider (aave, morpho, kamino, moonwell)")
	ratesCmd.Flags().StringVar(&ratesChain, "chain", "", "Chain identifier")
	ratesCmd.Flags().StringVar(&ratesAsset, "asset", "", "Asset (symbol/address/CAIP-19)")
	ratesCmd.Flags().IntVar(&ratesLimit, "limit", 20, "Maximum lending rates to return")
	ratesCmd.Flags().StringVar(&ratesRPCURL, "rpc-url", "", "Optional RPC URL override for on-chain providers")
	_ = ratesCmd.MarkFlagRequired("provider")
	_ = ratesCmd.MarkFlagRequired("chain")
	_ = ratesCmd.MarkFlagRequired("asset")

	var positionsProvider, positionsChain, positionsAddress, positionsAsset, positionsType, positionsRPCURL string
	var positionsLimit int
	positionsCmd := &cobra.Command{
		Use:   "positions",
		Short: "List lending positions for an account address",
		RunE: func(cmd *cobra.Command, args []string) error {
			providerName := normalizeLendingProvider(positionsProvider)
			if providerName == "" {
				return clierr.New(clierr.CodeUsage, "--provider is required")
			}
			chain, err := id.ParseChain(positionsChain)
			if err != nil {
				return err
			}
			account := strings.TrimSpace(positionsAddress)
			if account == "" {
				return clierr.New(clierr.CodeUsage, "--address is required")
			}
			if chain.IsEVM() && !common.IsHexAddress(account) {
				return clierr.New(clierr.CodeUsage, "--address must be a valid EVM hex address")
			}

			asset, err := parseOptionalChainAsset(chain, positionsAsset)
			if err != nil {
				return err
			}
			positionType, err := parseLendPositionType(positionsType)
			if err != nil {
				return err
			}

			cacheAccount := account
			if chain.IsEVM() {
				cacheAccount = strings.ToLower(account)
			}
			req := map[string]any{
				"provider": providerName,
				"chain":    chain.CAIP2,
				"address":  cacheAccount,
				"asset":    chainAssetFilterCacheValue(asset, positionsAsset),
				"type":     string(positionType),
				"limit":    positionsLimit,
				"rpc_url":  strings.TrimSpace(positionsRPCURL),
			}
			key := cacheKey(trimRootPath(cmd.CommandPath()), req)
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 30*time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				provider, err := s.selectLendingProvider(providerName)
				if err != nil {
					return nil, nil, nil, false, err
				}
				positionProvider, ok := provider.(providers.LendingPositionsProvider)
				if !ok {
					return nil, nil, nil, false, clierr.New(clierr.CodeUnsupported, fmt.Sprintf("lending provider %s does not support positions", providerName))
				}

				start := time.Now()
				data, err := positionProvider.LendPositions(ctx, providers.LendPositionsRequest{
					Chain:        chain,
					Account:      account,
					Asset:        asset,
					PositionType: positionType,
					Limit:        positionsLimit,
					RPCURL:       strings.TrimSpace(positionsRPCURL),
				})
				statuses := []model.ProviderStatus{{Name: provider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, statuses, nil, false, err
			})
		},
	}
	positionsCmd.Flags().StringVar(&positionsProvider, "provider", "", "Lending provider (aave, morpho, moonwell)")
	positionsCmd.Flags().StringVar(&positionsChain, "chain", "", "Chain identifier")
	positionsCmd.Flags().StringVar(&positionsAddress, "address", "", "Position owner address")
	positionsCmd.Flags().StringVar(&positionsAsset, "asset", "", "Optional asset filter (symbol/address/CAIP-19)")
	positionsCmd.Flags().StringVar(&positionsType, "type", string(providers.LendPositionTypeAll), "Position type filter (all|supply|borrow|collateral)")
	positionsCmd.Flags().IntVar(&positionsLimit, "limit", 20, "Maximum positions to return")
	positionsCmd.Flags().StringVar(&positionsRPCURL, "rpc-url", "", "Optional RPC URL override used by providers that need on-chain reads")
	_ = positionsCmd.MarkFlagRequired("provider")
	_ = positionsCmd.MarkFlagRequired("chain")
	_ = positionsCmd.MarkFlagRequired("address")

	root.AddCommand(marketsCmd)
	root.AddCommand(ratesCmd)
	root.AddCommand(positionsCmd)
	s.addLendExecutionSubcommands(root)
	return root
}

func (s *runtimeState) newBridgeCommand() *cobra.Command {
	root := &cobra.Command{Use: "bridge", Short: "Bridge quote and analytics commands"}

	var quoteProviderArg, fromArg, toArg, assetArg, toAssetArg, fromAmountForGas string
	var amountBase, amountDecimal string
	quoteCmd := &cobra.Command{
		Use:   "quote",
		Short: "Get bridge quote",
		RunE: func(cmd *cobra.Command, args []string) error {
			providerName := strings.ToLower(strings.TrimSpace(quoteProviderArg))
			if providerName == "" {
				return clierr.New(clierr.CodeUsage, "--provider is required (across|lifi)")
			}
			provider, ok := s.bridgeProviders[providerName]
			if !ok {
				return clierr.New(clierr.CodeUnsupported, "unsupported bridge provider")
			}
			fromChain, err := id.ParseChain(fromArg)
			if err != nil {
				return err
			}
			toChain, err := id.ParseChain(toArg)
			if err != nil {
				return err
			}
			fromAsset, err := id.ParseAsset(assetArg, fromChain)
			if err != nil {
				return err
			}
			toAssetInput := strings.TrimSpace(toAssetArg)
			if toAssetInput == "" {
				if fromAsset.Symbol != "" {
					toAssetInput = fromAsset.Symbol
				} else {
					return clierr.New(clierr.CodeUsage, "destination asset cannot be inferred, provide --to-asset")
				}
			}
			toAsset, err := id.ParseAsset(toAssetInput, toChain)
			if err != nil {
				return clierr.Wrap(clierr.CodeUsage, "resolve destination asset", err)
			}

			decimals := fromAsset.Decimals
			if decimals <= 0 {
				decimals = 18
			}
			base, decimal, err := id.NormalizeAmount(amountBase, amountDecimal, decimals)
			if err != nil {
				return err
			}

			reqStruct := providers.BridgeQuoteRequest{
				FromChain:        fromChain,
				ToChain:          toChain,
				FromAsset:        fromAsset,
				ToAsset:          toAsset,
				AmountBaseUnits:  base,
				AmountDecimal:    decimal,
				FromAmountForGas: strings.TrimSpace(fromAmountForGas),
			}
			key := cacheKey(trimRootPath(cmd.CommandPath()), map[string]any{
				"provider":            providerName,
				"from":                fromChain.CAIP2,
				"to":                  toChain.CAIP2,
				"from_asset":          fromAsset.AssetID,
				"to_asset":            toAsset.AssetID,
				"amount":              base,
				"from_amount_for_gas": reqStruct.FromAmountForGas,
			})
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 15*time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				start := time.Now()
				data, err := provider.QuoteBridge(ctx, reqStruct)
				status := []model.ProviderStatus{{Name: provider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, status, nil, false, err
			})
		},
	}
	quoteCmd.Flags().StringVar(&quoteProviderArg, "provider", "", "Bridge provider (across|lifi|bungee; no API key required)")
	quoteCmd.Flags().StringVar(&fromArg, "from", "", "Source chain")
	quoteCmd.Flags().StringVar(&toArg, "to", "", "Destination chain")
	quoteCmd.Flags().StringVar(&assetArg, "asset", "", "Asset (symbol/address/CAIP-19) on source chain")
	quoteCmd.Flags().StringVar(&toAssetArg, "to-asset", "", "Destination asset override (symbol/address/CAIP-19)")
	quoteCmd.Flags().StringVar(&amountBase, "amount", "", "Amount in base units")
	quoteCmd.Flags().StringVar(&amountDecimal, "amount-decimal", "", "Amount in decimal units")
	quoteCmd.Flags().StringVar(&fromAmountForGas, "from-amount-for-gas", "", "Optional amount in source token base units to reserve for destination native gas (LiFi)")
	_ = quoteCmd.MarkFlagRequired("from")
	_ = quoteCmd.MarkFlagRequired("to")
	_ = quoteCmd.MarkFlagRequired("asset")
	_ = quoteCmd.MarkFlagRequired("provider")
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "from", schema.FlagMetadata{Required: true, Format: "chain"})
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "to", schema.FlagMetadata{Required: true, Format: "chain"})
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "asset", schema.FlagMetadata{Required: true, Format: "asset"})
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "to-asset", schema.FlagMetadata{Format: "asset"})
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "amount", schema.FlagMetadata{Format: "base-units"})
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "amount-decimal", schema.FlagMetadata{Format: "decimal-amount"})
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "from-amount-for-gas", schema.FlagMetadata{Format: "base-units"})
	bridgeQuoteResponse := schema.SchemaFromType(model.BridgeQuote{})
	annotateStructuredFlagCommand(quoteCmd, structuredInputOptions{Response: &bridgeQuoteResponse})

	var listLimit int
	var includeChains bool
	listCmd := &cobra.Command{
		Use:   "list",
		Short: "List bridge volumes and coverage (DefiLlama key required)",
		RunE: func(cmd *cobra.Command, args []string) error {
			const providerName = "defillama"
			provider, ok := s.bridgeDataProviders[providerName]
			if !ok {
				return clierr.New(clierr.CodeUnsupported, "bridge data provider is not configured")
			}
			req := providers.BridgeListRequest{
				Limit:         listLimit,
				IncludeChains: includeChains,
			}
			key := cacheKey(trimRootPath(cmd.CommandPath()), map[string]any{
				"provider":       providerName,
				"limit":          req.Limit,
				"include_chains": req.IncludeChains,
			})
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 60*time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				start := time.Now()
				data, err := provider.ListBridges(ctx, req)
				status := []model.ProviderStatus{{Name: provider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, status, nil, false, err
			})
		},
	}
	listCmd.Flags().IntVar(&listLimit, "limit", 20, "Maximum bridges to return")
	listCmd.Flags().BoolVar(&includeChains, "include-chains", true, "Include chain coverage for each bridge")
	bridgeListResponse := schema.SchemaFromType([]model.BridgeSummary{})
	_ = schema.SetCommandMetadata(listCmd, schema.CommandMetadata{
		Auth: []schema.AuthRequirement{{
			Kind:        "api_key",
			EnvVars:     []string{"DEFI_DEFILLAMA_API_KEY"},
			Description: "Bridge list uses DefiLlama bridge data and requires a DefiLlama API key.",
		}},
		Response: &bridgeListResponse,
	})

	var bridgeArg string
	var includeChainBreakdown bool
	detailsCmd := &cobra.Command{
		Use:   "details",
		Short: "Get bridge volume details and chain breakdown (DefiLlama key required)",
		RunE: func(cmd *cobra.Command, args []string) error {
			const providerName = "defillama"
			provider, ok := s.bridgeDataProviders[providerName]
			if !ok {
				return clierr.New(clierr.CodeUnsupported, "bridge data provider is not configured")
			}
			req := providers.BridgeDetailsRequest{
				Bridge:                bridgeArg,
				IncludeChainBreakdown: includeChainBreakdown,
			}
			key := cacheKey(trimRootPath(cmd.CommandPath()), map[string]any{
				"provider":                providerName,
				"bridge":                  strings.ToLower(strings.TrimSpace(req.Bridge)),
				"include_chain_breakdown": req.IncludeChainBreakdown,
			})
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 60*time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				start := time.Now()
				data, err := provider.BridgeDetails(ctx, req)
				status := []model.ProviderStatus{{Name: provider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, status, nil, false, err
			})
		},
	}
	detailsCmd.Flags().StringVar(&bridgeArg, "bridge", "", "Bridge identifier (id, slug, or name)")
	detailsCmd.Flags().BoolVar(&includeChainBreakdown, "include-chain-breakdown", true, "Include per-chain bridge stats")
	_ = detailsCmd.MarkFlagRequired("bridge")
	bridgeDetailsResponse := schema.SchemaFromType(model.BridgeDetails{})
	_ = schema.SetCommandMetadata(detailsCmd, schema.CommandMetadata{
		Auth: []schema.AuthRequirement{{
			Kind:        "api_key",
			EnvVars:     []string{"DEFI_DEFILLAMA_API_KEY"},
			Description: "Bridge details uses DefiLlama bridge data and requires a DefiLlama API key.",
		}},
		Response: &bridgeDetailsResponse,
	})

	root.AddCommand(quoteCmd)
	root.AddCommand(listCmd)
	root.AddCommand(detailsCmd)
	s.addBridgeExecutionSubcommands(root)
	return root
}

func (s *runtimeState) newSwapCommand() *cobra.Command {
	root := &cobra.Command{Use: "swap", Short: "Swap quote and execution commands"}

	normalizeTradeType := func(raw string) (providers.SwapTradeType, error) {
		tradeType := providers.SwapTradeType(strings.ToLower(strings.TrimSpace(raw)))
		switch tradeType {
		case "", providers.SwapTradeTypeExactInput:
			return providers.SwapTradeTypeExactInput, nil
		case providers.SwapTradeTypeExactOutput:
			return providers.SwapTradeTypeExactOutput, nil
		default:
			return "", clierr.New(clierr.CodeUsage, "--type must be exact-input or exact-output")
		}
	}

	swapProviderSupportsExactOutput := func(providerName string) bool {
		switch providers.NormalizeSwapProvider(providerName) {
		case "uniswap", "tempo":
			return true
		default:
			return false
		}
	}

	parseSwapRequest := func(
		chainArg, fromAssetArg, toAssetArg string,
		tradeType providers.SwapTradeType,
		amountBase, amountDecimal, amountOutBase, amountOutDecimal, rpcURL string,
	) (providers.SwapQuoteRequest, error) {
		chain, err := id.ParseChain(chainArg)
		if err != nil {
			return providers.SwapQuoteRequest{}, err
		}
		fromAsset, err := id.ParseAsset(fromAssetArg, chain)
		if err != nil {
			return providers.SwapQuoteRequest{}, err
		}
		toAsset, err := id.ParseAsset(toAssetArg, chain)
		if err != nil {
			return providers.SwapQuoteRequest{}, err
		}

		var base, decimal string
		switch tradeType {
		case providers.SwapTradeTypeExactInput:
			if amountOutBase != "" || amountOutDecimal != "" {
				return providers.SwapQuoteRequest{}, clierr.New(clierr.CodeUsage, "--amount-out/--amount-out-decimal are only valid with --type exact-output")
			}
			decimals := fromAsset.Decimals
			if decimals <= 0 {
				decimals = 18
			}
			base, decimal, err = id.NormalizeAmount(amountBase, amountDecimal, decimals)
			if err != nil {
				return providers.SwapQuoteRequest{}, err
			}
		case providers.SwapTradeTypeExactOutput:
			if amountBase != "" || amountDecimal != "" {
				return providers.SwapQuoteRequest{}, clierr.New(clierr.CodeUsage, "--amount/--amount-decimal are only valid with --type exact-input")
			}
			if amountOutBase == "" && amountOutDecimal == "" {
				return providers.SwapQuoteRequest{}, clierr.New(clierr.CodeUsage, "exact-output requires --amount-out or --amount-out-decimal")
			}
			decimals := toAsset.Decimals
			if decimals <= 0 {
				decimals = 18
			}
			base, decimal, err = id.NormalizeAmount(amountOutBase, amountOutDecimal, decimals)
			if err != nil {
				return providers.SwapQuoteRequest{}, err
			}
		default:
			return providers.SwapQuoteRequest{}, clierr.New(clierr.CodeUsage, "--type must be exact-input or exact-output")
		}

		return providers.SwapQuoteRequest{
			Chain:           chain,
			FromAsset:       fromAsset,
			ToAsset:         toAsset,
			AmountBaseUnits: base,
			AmountDecimal:   decimal,
			RPCURL:          strings.TrimSpace(rpcURL),
			TradeType:       tradeType,
		}, nil
	}

	var quoteProviderArg, quoteChainArg, quoteFromAssetArg, quoteToAssetArg, quoteTradeTypeArg string
	var quoteAmountBase, quoteAmountDecimal, quoteAmountOutBase, quoteAmountOutDecimal, quoteRPCURL string
	var quoteFromAddress string
	var quoteSlippagePct float64
	quoteCmd := &cobra.Command{
		Use:   "quote",
		Short: "Get swap quote",
		RunE: func(cmd *cobra.Command, args []string) error {
			providerName := providers.NormalizeSwapProvider(quoteProviderArg)
			if providerName == "" {
				return clierr.New(clierr.CodeUsage, "--provider is required (1inch|uniswap|tempo|taikoswap|jupiter|fibrous|bungee)")
			}
			provider, ok := s.swapProviders[providerName]
			if !ok {
				return clierr.New(clierr.CodeUnsupported, "unsupported swap provider")
			}
			tradeType, err := normalizeTradeType(quoteTradeTypeArg)
			if err != nil {
				return err
			}
			if tradeType == providers.SwapTradeTypeExactOutput && !swapProviderSupportsExactOutput(providerName) {
				return clierr.New(clierr.CodeUnsupported, "exact-output swap quotes currently support only --provider uniswap or --provider tempo")
			}

			var slippagePtr *float64
			slippageMode := "auto"
			if cmd.Flags().Changed("slippage-pct") {
				if providerName != "uniswap" {
					return clierr.New(clierr.CodeUsage, "--slippage-pct is supported only with --provider uniswap")
				}
				if quoteSlippagePct <= 0 || quoteSlippagePct > 100 {
					return clierr.New(clierr.CodeUsage, "--slippage-pct must be > 0 and <= 100")
				}
				slippageMode = "manual"
				slippagePtr = &quoteSlippagePct
			}

			swapper := strings.TrimSpace(quoteFromAddress)
			if swapper != "" && !common.IsHexAddress(swapper) {
				return clierr.New(clierr.CodeUsage, "--from-address must be a valid EVM hex address")
			}
			if providerName == "uniswap" && swapper == "" {
				return clierr.New(clierr.CodeUsage, "--from-address is required for --provider uniswap")
			}

			reqStruct, err := parseSwapRequest(
				quoteChainArg,
				quoteFromAssetArg,
				quoteToAssetArg,
				tradeType,
				quoteAmountBase,
				quoteAmountDecimal,
				quoteAmountOutBase,
				quoteAmountOutDecimal,
				quoteRPCURL,
			)
			if err != nil {
				return err
			}
			reqStruct.SlippagePct = slippagePtr
			reqStruct.Swapper = swapper
			key := cacheKey(trimRootPath(cmd.CommandPath()), map[string]any{
				"provider":      providerName,
				"chain":         reqStruct.Chain.CAIP2,
				"from":          reqStruct.FromAsset.AssetID,
				"to":            reqStruct.ToAsset.AssetID,
				"trade_type":    reqStruct.TradeType,
				"amount":        reqStruct.AmountBaseUnits,
				"slippage_mode": slippageMode,
				"slippage_pct":  reqStruct.SlippagePct,
				"swapper":       strings.ToLower(reqStruct.Swapper),
				"rpc_url":       reqStruct.RPCURL,
			})
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 15*time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				start := time.Now()
				data, err := provider.QuoteSwap(ctx, reqStruct)
				status := []model.ProviderStatus{{Name: provider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, status, nil, false, err
			})
		},
	}
	quoteCmd.Flags().StringVar(&quoteProviderArg, "provider", "", "Swap provider (1inch|uniswap|tempo|taikoswap|jupiter|fibrous|bungee)")
	quoteCmd.Flags().StringVar(&quoteChainArg, "chain", "", "Chain identifier")
	quoteCmd.Flags().StringVar(&quoteFromAssetArg, "from-asset", "", "Input asset")
	quoteCmd.Flags().StringVar(&quoteToAssetArg, "to-asset", "", "Output asset")
	quoteCmd.Flags().StringVar(&quoteTradeTypeArg, "type", string(providers.SwapTradeTypeExactInput), "Swap type (exact-input|exact-output)")
	quoteCmd.Flags().StringVar(&quoteAmountBase, "amount", "", "Exact-input amount in base units")
	quoteCmd.Flags().StringVar(&quoteAmountDecimal, "amount-decimal", "", "Exact-input amount in decimal units")
	quoteCmd.Flags().StringVar(&quoteAmountOutBase, "amount-out", "", "Exact-output amount in base units")
	quoteCmd.Flags().StringVar(&quoteAmountOutDecimal, "amount-out-decimal", "", "Exact-output amount in decimal units")
	quoteCmd.Flags().Float64Var(&quoteSlippagePct, "slippage-pct", 0, "Manual max slippage percent override (Uniswap only; default uses provider auto slippage)")
	quoteCmd.Flags().StringVar(&quoteFromAddress, "from-address", "", "Swapper/sender EOA address (required for --provider uniswap)")
	quoteCmd.Flags().StringVar(&quoteRPCURL, "rpc-url", "", "RPC URL override for on-chain quote providers")
	_ = quoteCmd.MarkFlagRequired("chain")
	_ = quoteCmd.MarkFlagRequired("from-asset")
	_ = quoteCmd.MarkFlagRequired("to-asset")
	_ = quoteCmd.MarkFlagRequired("provider")
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "chain", schema.FlagMetadata{Required: true, Format: "chain"})
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "from-asset", schema.FlagMetadata{Required: true, Format: "asset"})
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "to-asset", schema.FlagMetadata{Required: true, Format: "asset"})
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "type", schema.FlagMetadata{Enum: []string{string(providers.SwapTradeTypeExactInput), string(providers.SwapTradeTypeExactOutput)}})
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "amount", schema.FlagMetadata{Format: "base-units"})
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "amount-decimal", schema.FlagMetadata{Format: "decimal-amount"})
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "amount-out", schema.FlagMetadata{Format: "base-units"})
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "amount-out-decimal", schema.FlagMetadata{Format: "decimal-amount"})
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "from-address", schema.FlagMetadata{Format: "evm-address"})
	_ = schema.SetFlagMetadata(quoteCmd.Flags(), "rpc-url", schema.FlagMetadata{Format: "url"})
	swapQuoteResponse := schema.SchemaFromType(model.SwapQuote{})
	annotateStructuredFlagCommand(quoteCmd, structuredInputOptions{
		Auth: []schema.AuthRequirement{
			{
				Kind:        "api_key",
				EnvVars:     []string{"DEFI_1INCH_API_KEY"},
				When:        map[string][]string{"provider": []string{"1inch"}},
				Description: "1inch quote requests require a 1inch API key.",
			},
			{
				Kind:        "api_key",
				EnvVars:     []string{"DEFI_UNISWAP_API_KEY"},
				When:        map[string][]string{"provider": []string{"uniswap"}},
				Description: "Uniswap quote requests require a Uniswap API key.",
			},
			{
				Kind:        "api_key",
				EnvVars:     []string{"DEFI_JUPITER_API_KEY"},
				Optional:    true,
				When:        map[string][]string{"provider": []string{"jupiter"}},
				Description: "Jupiter API keys are optional and mainly increase rate limits.",
			},
		},
		Response: &swapQuoteResponse,
	})

	type swapPlanArgs struct {
		Provider         string `json:"provider" flag:"provider" required:"true" enum:"taikoswap,tempo"`
		ChainArg         string `json:"chain" flag:"chain" required:"true" format:"chain"`
		FromAssetArg     string `json:"from_asset" flag:"from-asset" required:"true" format:"asset"`
		ToAssetArg       string `json:"to_asset" flag:"to-asset" required:"true" format:"asset"`
		TradeType        string `json:"type" flag:"type" enum:"exact-input,exact-output"`
		AmountBase       string `json:"amount" flag:"amount" format:"base-units"`
		AmountDecimal    string `json:"amount_decimal" flag:"amount-decimal" format:"decimal-amount"`
		AmountOutBase    string `json:"amount_out" flag:"amount-out" format:"base-units"`
		AmountOutDecimal string `json:"amount_out_decimal" flag:"amount-out-decimal" format:"decimal-amount"`
		WalletRef        string `json:"wallet" flag:"wallet" format:"identifier"`
		FromAddress      string `json:"from_address" flag:"from-address" format:"evm-address"`
		Recipient        string `json:"recipient" flag:"recipient" format:"evm-address"`
		SlippageBps      int64  `json:"slippage_bps" flag:"slippage-bps"`
		Simulate         bool   `json:"simulate" flag:"simulate"`
		RPCURL           string `json:"rpc_url" flag:"rpc-url" format:"url"`
	}
	type swapSubmitArgs struct {
		ActionID           string  `json:"action_id" flag:"action-id" required:"true" format:"action-id"`
		Simulate           bool    `json:"simulate" flag:"simulate"`
		Signer             string  `json:"signer" flag:"signer" enum:"local,tempo"`
		KeySource          string  `json:"key_source" flag:"key-source" enum:"auto,env,file,keystore"`
		PrivateKey         string  `json:"private_key" flag:"private-key" format:"hex"`
		FromAddress        string  `json:"from_address" flag:"from-address" format:"evm-address"`
		PollInterval       string  `json:"poll_interval" flag:"poll-interval" format:"duration"`
		StepTimeout        string  `json:"step_timeout" flag:"step-timeout" format:"duration"`
		GasMultiplier      float64 `json:"gas_multiplier" flag:"gas-multiplier"`
		MaxFeeGwei         string  `json:"max_fee_gwei" flag:"max-fee-gwei"`
		MaxPriorityFeeGwei string  `json:"max_priority_fee_gwei" flag:"max-priority-fee-gwei"`
		AllowMaxApproval   bool    `json:"allow_max_approval" flag:"allow-max-approval"`
		UnsafeProviderTx   bool    `json:"unsafe_provider_tx" flag:"unsafe-provider-tx"`
		FeeToken           string  `json:"fee_token" flag:"fee-token" format:"evm-address"`
	}
	var plan swapPlanArgs
	planCmd := &cobra.Command{
		Use:   "plan",
		Short: "Create and persist a swap action plan",
		RunE: func(cmd *cobra.Command, args []string) error {
			providerName := providers.NormalizeSwapProvider(plan.Provider)
			if providerName == "" {
				return clierr.New(clierr.CodeUsage, "--provider is required")
			}
			tradeType, err := normalizeTradeType(plan.TradeType)
			if err != nil {
				return err
			}
			if tradeType == providers.SwapTradeTypeExactOutput && !swapProviderSupportsExactOutput(providerName) {
				return clierr.New(clierr.CodeUnsupported, "exact-output swap planning currently supports only --provider tempo")
			}
			reqStruct, err := parseSwapRequest(
				plan.ChainArg,
				plan.FromAssetArg,
				plan.ToAssetArg,
				tradeType,
				plan.AmountBase,
				plan.AmountDecimal,
				plan.AmountOutBase,
				plan.AmountOutDecimal,
				plan.RPCURL,
			)
			if err != nil {
				return err
			}
			var identity executionIdentity
			warnings := []string(nil)
			sender := ""
			if providerName == "tempo" {
				if strings.TrimSpace(plan.WalletRef) != "" && strings.TrimSpace(plan.FromAddress) != "" {
					return clierr.New(clierr.CodeUsage, "use only one identity input: --wallet or --from-address")
				}
				if strings.TrimSpace(plan.WalletRef) != "" {
					return clierr.New(clierr.CodeUnsupported, "--wallet planning is not supported on Tempo chains yet; use --from-address")
				}
				if strings.TrimSpace(plan.FromAddress) == "" {
					return clierr.New(clierr.CodeUsage, "--from-address is required for --provider tempo")
				}
				if !common.IsHexAddress(plan.FromAddress) {
					return clierr.New(clierr.CodeUsage, "--from-address must be a valid EVM hex address")
				}
				sender = common.HexToAddress(plan.FromAddress).Hex()
			} else {
				identity, err = resolveExecutionIdentity(plan.WalletRef, plan.FromAddress, plan.ChainArg)
				if err != nil {
					return err
				}
				sender = identity.FromAddress
				warnings = identity.Warnings
			}

			ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
			defer cancel()
			start := time.Now()
			action, providerInfoName, err := s.actionBuilderRegistry().BuildSwapAction(ctx, providerName, "plan", reqStruct, providers.SwapExecutionOptions{
				Sender:      sender,
				Recipient:   plan.Recipient,
				SlippageBps: plan.SlippageBps,
				Simulate:    plan.Simulate,
				RPCURL:      plan.RPCURL,
			})
			if strings.TrimSpace(providerInfoName) == "" {
				providerInfoName = providerName
			}
			statuses := []model.ProviderStatus{{Name: providerInfoName, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
			if err != nil {
				s.captureCommandDiagnostics(nil, statuses, false)
				return err
			}
			if providerName == "tempo" {
				action.FromAddress = sender
				action.ExecutionBackend = execution.ExecutionBackendTempo
			} else {
				applyExecutionIdentityToAction(&action, identity)
			}
			if err := s.ensureActionStore(); err != nil {
				return err
			}
			if err := s.actionStore.Save(action); err != nil {
				return clierr.Wrap(clierr.CodeInternal, "persist planned action", err)
			}
			s.captureCommandDiagnostics(nil, statuses, false)
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, warnings, cacheMetaBypass(), statuses, false)
		},
	}
	planCmd.Flags().StringVar(&plan.Provider, "provider", "", "Swap execution provider (taikoswap|tempo)")
	planCmd.Flags().StringVar(&plan.ChainArg, "chain", "", "Chain identifier")
	planCmd.Flags().StringVar(&plan.FromAssetArg, "from-asset", "", "Input asset")
	planCmd.Flags().StringVar(&plan.ToAssetArg, "to-asset", "", "Output asset")
	planCmd.Flags().StringVar(&plan.TradeType, "type", string(providers.SwapTradeTypeExactInput), "Swap type (exact-input|exact-output)")
	planCmd.Flags().StringVar(&plan.AmountBase, "amount", "", "Exact-input amount in base units")
	planCmd.Flags().StringVar(&plan.AmountDecimal, "amount-decimal", "", "Exact-input amount in decimal units")
	planCmd.Flags().StringVar(&plan.AmountOutBase, "amount-out", "", "Exact-output amount in base units")
	planCmd.Flags().StringVar(&plan.AmountOutDecimal, "amount-out-decimal", "", "Exact-output amount in decimal units")
	planCmd.Flags().StringVar(&plan.WalletRef, "wallet", "", "Wallet identifier or name")
	planCmd.Flags().StringVar(&plan.FromAddress, "from-address", "", "Sender EOA address")
	planCmd.Flags().StringVar(&plan.Recipient, "recipient", "", "Recipient address (defaults to the resolved sender address)")
	planCmd.Flags().Int64Var(&plan.SlippageBps, "slippage-bps", 50, "Max slippage in basis points")
	planCmd.Flags().BoolVar(&plan.Simulate, "simulate", true, "Include simulation checks during execution")
	planCmd.Flags().StringVar(&plan.RPCURL, "rpc-url", "", "RPC URL override for the selected chain")
	_ = planCmd.MarkFlagRequired("chain")
	_ = planCmd.MarkFlagRequired("from-asset")
	_ = planCmd.MarkFlagRequired("to-asset")
	_ = planCmd.MarkFlagRequired("provider")
	configureStructuredInput[swapPlanArgs](planCmd, structuredInputOptions{
		Mutation:         true,
		InputConstraints: swapPlanIdentityInputConstraints(),
	})

	var submit swapSubmitArgs
	submitCmd := &cobra.Command{
		Use:   "submit",
		Short: "Execute a previously planned swap action",
		RunE: func(cmd *cobra.Command, args []string) error {
			actionID, err := resolveActionID(submit.ActionID)
			if err != nil {
				return err
			}
			if err := s.ensureActionStore(); err != nil {
				return err
			}
			action, err := s.actionStore.Get(actionID)
			if err != nil {
				return clierr.Wrap(clierr.CodeUsage, "load action", err)
			}
			if action.IntentType != "swap" {
				return clierr.New(clierr.CodeUsage, "action is not a swap intent")
			}
			if action.Status == execution.ActionStatusCompleted {
				return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, []string{"action already completed"}, cacheMetaBypass(), nil, false)
			}

			resolvedExec, err := resolveActionExecutionBackend(cmd, action, submitExecutionInputs{
				Signer:      submit.Signer,
				KeySource:   submit.KeySource,
				PrivateKey:  submit.PrivateKey,
				FromAddress: submit.FromAddress,
			})
			if err != nil {
				return err
			}
			if err := validateExecutionSender(action, submit.FromAddress, resolvedExec.sender); err != nil {
				return err
			}
			execOpts, err := parseExecuteOptions(
				submit.Simulate,
				submit.PollInterval,
				submit.StepTimeout,
				submit.GasMultiplier,
				submit.MaxFeeGwei,
				submit.MaxPriorityFeeGwei,
				submit.AllowMaxApproval,
				submit.UnsafeProviderTx,
				submit.FeeToken,
			)
			if err != nil {
				return err
			}
			if err := s.executeActionWithTimeout(&action, resolvedExec.txSigner, resolvedExec.evmBackend, execOpts); err != nil {
				return err
			}
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, nil, cacheMetaBypass(), nil, false)
		},
	}
	submitCmd.Flags().StringVar(&submit.ActionID, "action-id", "", "Action identifier returned by swap plan")
	submitCmd.Flags().BoolVar(&submit.Simulate, "simulate", true, "Run preflight simulation before submission")
	submitCmd.Flags().StringVar(&submit.Signer, "signer", "local", "Signer backend (local|tempo)")
	submitCmd.Flags().StringVar(&submit.KeySource, "key-source", execsigner.KeySourceAuto, "Key source (auto|env|file|keystore)")
	submitCmd.Flags().StringVar(&submit.PrivateKey, "private-key", "", "Private key hex override for local signer (less safe)")
	submitCmd.Flags().StringVar(&submit.FromAddress, "from-address", "", "Expected sender EOA address")
	submitCmd.Flags().StringVar(&submit.PollInterval, "poll-interval", "2s", "Receipt polling interval")
	submitCmd.Flags().StringVar(&submit.StepTimeout, "step-timeout", "2m", "Per-step receipt timeout")
	submitCmd.Flags().Float64Var(&submit.GasMultiplier, "gas-multiplier", 1.2, "Gas estimate safety multiplier")
	submitCmd.Flags().StringVar(&submit.MaxFeeGwei, "max-fee-gwei", "", "Optional EIP-1559 max fee (gwei)")
	submitCmd.Flags().StringVar(&submit.MaxPriorityFeeGwei, "max-priority-fee-gwei", "", "Optional EIP-1559 max priority fee (gwei)")
	submitCmd.Flags().BoolVar(&submit.AllowMaxApproval, "allow-max-approval", false, "Allow approval amounts greater than planned input amount")
	submitCmd.Flags().BoolVar(&submit.UnsafeProviderTx, "unsafe-provider-tx", false, "Bypass provider transaction guardrails for bridge/aggregator payloads")
	submitCmd.Flags().StringVar(&submit.FeeToken, "fee-token", "", "Fee token address for Tempo chains (defaults to chain USDC.e)")
	annotateStructuredSubmitCommand(submitCmd, swapSubmitArgs{})

	var statusActionID string
	statusCmd := &cobra.Command{
		Use:   "status",
		Short: "Get swap action status",
		RunE: func(cmd *cobra.Command, args []string) error {
			actionID, err := resolveActionID(statusActionID)
			if err != nil {
				return err
			}
			if err := s.ensureActionStore(); err != nil {
				return err
			}
			action, err := s.actionStore.Get(actionID)
			if err != nil {
				return clierr.Wrap(clierr.CodeUsage, "load action", err)
			}
			if action.IntentType != "swap" {
				return clierr.New(clierr.CodeUsage, "action is not a swap intent")
			}
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, nil, cacheMetaBypass(), nil, false)
		},
	}
	statusCmd.Flags().StringVar(&statusActionID, "action-id", "", "Action identifier returned by swap plan")
	annotateExecutionStatusCommand(statusCmd)

	root.AddCommand(quoteCmd)
	root.AddCommand(planCmd)
	root.AddCommand(submitCmd)
	root.AddCommand(statusCmd)
	return root
}

func (s *runtimeState) newActionsCommand() *cobra.Command {
	root := &cobra.Command{
		Use:   "actions",
		Short: "Execution action inspection commands",
		RunE: func(cmd *cobra.Command, args []string) error {
			if len(args) == 0 {
				return cmd.Help()
			}
			return clierr.New(clierr.CodeUsage, fmt.Sprintf("unknown actions subcommand %q", args[0]))
		},
	}

	var listStatus string
	var listLimit int
	listCmd := &cobra.Command{
		Use:   "list",
		Short: "List persisted actions",
		RunE: func(cmd *cobra.Command, args []string) error {
			if err := s.ensureActionStore(); err != nil {
				return err
			}
			items, err := s.actionStore.List(strings.TrimSpace(listStatus), listLimit)
			if err != nil {
				return clierr.Wrap(clierr.CodeInternal, "list actions", err)
			}
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), items, nil, cacheMetaBypass(), nil, false)
		},
	}
	listCmd.Flags().StringVar(&listStatus, "status", "", "Optional action status filter")
	listCmd.Flags().IntVar(&listLimit, "limit", 20, "Maximum actions to return")

	lookupAction := func(cmd *cobra.Command, actionIDArg string) error {
		actionID, err := resolveActionID(actionIDArg)
		if err != nil {
			return err
		}
		if err := s.ensureActionStore(); err != nil {
			return err
		}
		item, err := s.actionStore.Get(actionID)
		if err != nil {
			return clierr.Wrap(clierr.CodeUsage, "load action", err)
		}
		return s.emitSuccess(trimRootPath(cmd.CommandPath()), item, nil, cacheMetaBypass(), nil, false)
	}

	var showActionID string
	showCmd := &cobra.Command{
		Use:   "show",
		Short: "Show action details by action id",
		RunE: func(cmd *cobra.Command, _ []string) error {
			return lookupAction(cmd, showActionID)
		},
	}
	showCmd.Flags().StringVar(&showActionID, "action-id", "", "Action identifier")

	var estimateActionID, estimateStepIDs, estimateMaxFeeGwei, estimateMaxPriorityFeeGwei, estimateBlockTag string
	var estimateGasMultiplier float64
	estimateCmd := &cobra.Command{
		Use:   "estimate",
		Short: "Estimate gas and EIP-1559 fees for a planned action",
		RunE: func(cmd *cobra.Command, _ []string) error {
			actionID, err := resolveActionID(estimateActionID)
			if err != nil {
				return err
			}
			if err := s.ensureActionStore(); err != nil {
				return err
			}
			action, err := s.actionStore.Get(actionID)
			if err != nil {
				return clierr.Wrap(clierr.CodeUsage, "load action", err)
			}
			opts, err := parseActionEstimateOptions(
				estimateStepIDs,
				estimateGasMultiplier,
				estimateMaxFeeGwei,
				estimateMaxPriorityFeeGwei,
				estimateBlockTag,
			)
			if err != nil {
				return err
			}
			ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
			defer cancel()
			estimate, err := execution.EstimateActionGas(ctx, action, opts)
			if err != nil {
				return err
			}
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), estimate, nil, cacheMetaBypass(), nil, false)
		},
	}
	estimateCmd.Flags().StringVar(&estimateActionID, "action-id", "", "Action identifier")
	estimateCmd.Flags().StringVar(&estimateStepIDs, "step-ids", "", "Optional comma-separated step_id filter")
	estimateCmd.Flags().Float64Var(&estimateGasMultiplier, "gas-multiplier", 1.2, "Gas estimate safety multiplier")
	estimateCmd.Flags().StringVar(&estimateMaxFeeGwei, "max-fee-gwei", "", "Optional EIP-1559 max fee (gwei)")
	estimateCmd.Flags().StringVar(&estimateMaxPriorityFeeGwei, "max-priority-fee-gwei", "", "Optional EIP-1559 max priority fee (gwei)")
	estimateCmd.Flags().StringVar(&estimateBlockTag, "block-tag", "pending", "Block tag used for estimation (pending|latest)")

	root.AddCommand(listCmd)
	root.AddCommand(showCmd)
	root.AddCommand(estimateCmd)
	return root
}

func (s *runtimeState) newYieldCommand() *cobra.Command {
	root := &cobra.Command{Use: "yield", Short: "Yield opportunities, positions, history, and execution"}

	var opportunitiesChainArg, opportunitiesAssetArg, opportunitiesProvidersArg, opportunitiesSortArg string
	var opportunitiesLimit int
	var opportunitiesMinTVL, opportunitiesMinAPY float64
	var opportunitiesIncludeIncomplete bool
	var opportunitiesRPCURL string
	opportunitiesCmd := &cobra.Command{
		Use:   "opportunities",
		Short: "Rank yield opportunities",
		RunE: func(cmd *cobra.Command, args []string) error {
			chain, asset, err := parseChainAsset(opportunitiesChainArg, opportunitiesAssetArg)
			if err != nil {
				return err
			}
			req := providers.YieldRequest{
				Chain:             chain,
				Asset:             asset,
				Limit:             opportunitiesLimit,
				MinTVLUSD:         opportunitiesMinTVL,
				MinAPY:            opportunitiesMinAPY,
				Providers:         splitCSV(opportunitiesProvidersArg),
				SortBy:            opportunitiesSortArg,
				IncludeIncomplete: opportunitiesIncludeIncomplete,
			}
			key := cacheKey(trimRootPath(cmd.CommandPath()), map[string]any{
				"chain":              req.Chain.CAIP2,
				"asset":              req.Asset.AssetID,
				"limit":              req.Limit,
				"min_tvl_usd":        req.MinTVLUSD,
				"min_apy":            req.MinAPY,
				"providers":          req.Providers,
				"sort":               req.SortBy,
				"include_incomplete": req.IncludeIncomplete,
				"rpc_url":            strings.TrimSpace(opportunitiesRPCURL),
			})
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 60*time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				selectedProviders, err := s.selectYieldProviders(req.Providers, req.Chain)
				if err != nil {
					return nil, nil, nil, false, err
				}
				warnings := []string{}
				statuses := make([]model.ProviderStatus, 0, len(selectedProviders))
				combined := make([]model.YieldOpportunity, 0)
				partial := false
				var firstErr error

				for _, providerName := range selectedProviders {
					provider := s.yieldProviders[providerName]
					applyRPCOverride(provider, opportunitiesRPCURL)
					reqCopy := req
					reqCopy.Providers = nil
					start := time.Now()
					items, providerErr := provider.YieldOpportunities(ctx, reqCopy)
					statuses = append(statuses, model.ProviderStatus{Name: provider.Info().Name, Status: statusFromErr(providerErr), LatencyMS: time.Since(start).Milliseconds()})
					if providerErr != nil {
						partial = true
						warnings = append(warnings, fmt.Sprintf("provider %s failed: %v", provider.Info().Name, providerErr))
						if firstErr == nil {
							firstErr = providerErr
						}
						continue
					}
					combined = append(combined, items...)
				}

				if opportunitiesIncludeIncomplete {
					warnings = append(warnings, "include_incomplete enabled: opportunities with missing APY/TVL may be present")
				}

				if len(combined) == 0 {
					if firstErr != nil {
						return nil, statuses, warnings, partial, firstErr
					}
					return nil, statuses, warnings, partial, clierr.New(clierr.CodeUnavailable, "no yield opportunities returned by selected providers")
				}

				combined = dedupeYieldByOpportunityID(combined)
				sortYieldOpportunities(combined, req.SortBy)
				if req.Limit > 0 && len(combined) > req.Limit {
					combined = combined[:req.Limit]
				}
				if opportunitiesIncludeIncomplete {
					warnings = append(warnings, fmt.Sprintf("returned %d combined opportunities across %d provider(s)", len(combined), len(selectedProviders)))
				}
				return combined, statuses, warnings, partial, nil
			})
		},
	}
	opportunitiesCmd.Flags().StringVar(&opportunitiesChainArg, "chain", "", "Chain identifier")
	opportunitiesCmd.Flags().StringVar(&opportunitiesAssetArg, "asset", "", "Asset symbol/address/CAIP-19")
	opportunitiesCmd.Flags().IntVar(&opportunitiesLimit, "limit", 20, "Maximum opportunities to return")
	opportunitiesCmd.Flags().Float64Var(&opportunitiesMinTVL, "min-tvl-usd", 0, "Minimum TVL in USD")
	opportunitiesCmd.Flags().Float64Var(&opportunitiesMinAPY, "min-apy", 0, "Minimum total APY percent")
	opportunitiesCmd.Flags().StringVar(&opportunitiesProvidersArg, "providers", "", "Filter by provider names (aave,morpho,kamino,moonwell)")
	opportunitiesCmd.Flags().StringVar(&opportunitiesSortArg, "sort", "apy_total", "Sort key (apy_total|tvl_usd|liquidity_usd)")
	opportunitiesCmd.Flags().BoolVar(&opportunitiesIncludeIncomplete, "include-incomplete", false, "Include opportunities missing APY/TVL")
	opportunitiesCmd.Flags().StringVar(&opportunitiesRPCURL, "rpc-url", "", "Optional RPC URL override for on-chain providers")
	_ = opportunitiesCmd.MarkFlagRequired("chain")
	_ = opportunitiesCmd.MarkFlagRequired("asset")
	root.AddCommand(opportunitiesCmd)

	var positionsChainArg, positionsAddressArg, positionsAssetArg, positionsProvidersArg string
	var positionsLimit int
	var positionsRPCURL string
	positionsCmd := &cobra.Command{
		Use:   "positions",
		Short: "List yield positions for an account address",
		RunE: func(cmd *cobra.Command, args []string) error {
			chain, err := id.ParseChain(positionsChainArg)
			if err != nil {
				return err
			}
			account := strings.TrimSpace(positionsAddressArg)
			if account == "" {
				return clierr.New(clierr.CodeUsage, "--address is required")
			}
			if chain.IsEVM() && !common.IsHexAddress(account) {
				return clierr.New(clierr.CodeUsage, "--address must be a valid EVM hex address")
			}

			asset, err := parseOptionalChainAsset(chain, positionsAssetArg)
			if err != nil {
				return err
			}
			providerFilter := splitCSV(positionsProvidersArg)

			cacheAccount := account
			if chain.IsEVM() {
				cacheAccount = strings.ToLower(account)
			}
			req := map[string]any{
				"chain":     chain.CAIP2,
				"address":   cacheAccount,
				"asset":     chainAssetFilterCacheValue(asset, positionsAssetArg),
				"providers": providerFilter,
				"limit":     positionsLimit,
				"rpc_url":   strings.TrimSpace(positionsRPCURL),
			}
			key := cacheKey(trimRootPath(cmd.CommandPath()), req)
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 30*time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				selectedProviders, err := s.selectYieldProviders(providerFilter, chain)
				if err != nil {
					return nil, nil, nil, false, err
				}

				statuses := make([]model.ProviderStatus, 0, len(selectedProviders))
				warnings := []string{}
				combined := make([]model.YieldPosition, 0)
				partial := false
				var firstErr error

				for _, providerName := range selectedProviders {
					provider := s.yieldProviders[providerName]
					positionProvider, ok := provider.(providers.YieldPositionsProvider)
					providerStart := time.Now()
					if !ok {
						providerErr := clierr.New(clierr.CodeUnsupported, fmt.Sprintf("yield provider %s does not support positions", providerName))
						statuses = append(statuses, model.ProviderStatus{Name: provider.Info().Name, Status: statusFromErr(providerErr), LatencyMS: time.Since(providerStart).Milliseconds()})
						warnings = append(warnings, fmt.Sprintf("provider %s does not support yield positions", provider.Info().Name))
						partial = true
						if firstErr == nil {
							firstErr = providerErr
						}
						continue
					}

					items, providerErr := positionProvider.YieldPositions(ctx, providers.YieldPositionsRequest{
						Chain:   chain,
						Account: account,
						Asset:   asset,
						Limit:   positionsLimit,
						RPCURL:  strings.TrimSpace(positionsRPCURL),
					})
					statuses = append(statuses, model.ProviderStatus{Name: provider.Info().Name, Status: statusFromErr(providerErr), LatencyMS: time.Since(providerStart).Milliseconds()})
					if providerErr != nil {
						warnings = append(warnings, fmt.Sprintf("provider %s failed: %v", provider.Info().Name, providerErr))
						partial = true
						if firstErr == nil {
							firstErr = providerErr
						}
						continue
					}
					combined = append(combined, items...)
				}

				if len(combined) == 0 {
					if firstErr != nil {
						return nil, statuses, warnings, partial, firstErr
					}
					return nil, statuses, warnings, partial, clierr.New(clierr.CodeUnavailable, "no yield positions returned by selected providers")
				}

				sortYieldPositions(combined)
				if positionsLimit > 0 && len(combined) > positionsLimit {
					combined = combined[:positionsLimit]
				}
				return combined, statuses, warnings, partial, nil
			})
		},
	}
	positionsCmd.Flags().StringVar(&positionsChainArg, "chain", "", "Chain identifier")
	positionsCmd.Flags().StringVar(&positionsAddressArg, "address", "", "Position owner address")
	positionsCmd.Flags().StringVar(&positionsAssetArg, "asset", "", "Optional asset filter (symbol/address/CAIP-19)")
	positionsCmd.Flags().StringVar(&positionsProvidersArg, "providers", "", "Filter by provider names (aave,morpho,kamino,moonwell)")
	positionsCmd.Flags().IntVar(&positionsLimit, "limit", 20, "Maximum positions to return")
	positionsCmd.Flags().StringVar(&positionsRPCURL, "rpc-url", "", "Optional RPC URL override used by providers that need on-chain valuation")
	_ = positionsCmd.MarkFlagRequired("chain")
	_ = positionsCmd.MarkFlagRequired("address")
	root.AddCommand(positionsCmd)

	var historyChainArg, historyAssetArg, historyProvidersArg, historyMetricsArg string
	var historyIntervalArg, historyWindowArg, historyFromArg, historyToArg, historyOpportunityIDsArg string
	var historyLimit int
	historyCmd := &cobra.Command{
		Use:   "history",
		Short: "Get yield history for provider opportunities",
		RunE: func(cmd *cobra.Command, args []string) error {
			chain, asset, err := parseChainAsset(historyChainArg, historyAssetArg)
			if err != nil {
				return err
			}
			metrics, err := parseYieldHistoryMetrics(historyMetricsArg)
			if err != nil {
				return err
			}
			interval, err := parseYieldHistoryInterval(historyIntervalArg)
			if err != nil {
				return err
			}
			startTime, endTime, err := resolveYieldHistoryRange(historyFromArg, historyToArg, historyWindowArg, s.runner.now().UTC())
			if err != nil {
				return err
			}
			opportunityIDs := splitCSV(historyOpportunityIDsArg)
			opportunityIDSet := make(map[string]struct{}, len(opportunityIDs))
			for _, item := range opportunityIDs {
				opportunityIDSet[item] = struct{}{}
			}
			providerFilter := splitCSV(historyProvidersArg)

			key := cacheKey(trimRootPath(cmd.CommandPath()), map[string]any{
				"chain":             chain.CAIP2,
				"asset":             asset.AssetID,
				"providers":         providerFilter,
				"metrics":           metrics,
				"interval":          interval,
				"start_time":        startTime.UTC().Format(time.RFC3339),
				"end_time":          endTime.UTC().Format(time.RFC3339),
				"opportunity_ids":   opportunityIDs,
				"opportunity_limit": historyLimit,
			})
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 5*time.Minute, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				selectedProviders, err := s.selectYieldProviders(providerFilter, chain)
				if err != nil {
					return nil, nil, nil, false, err
				}

				statuses := make([]model.ProviderStatus, 0, len(selectedProviders))
				warnings := []string{}
				combined := make([]model.YieldHistorySeries, 0)
				partial := false
				var firstErr error

				for _, providerName := range selectedProviders {
					provider := s.yieldProviders[providerName]
					historyProvider, ok := provider.(providers.YieldHistoryProvider)
					providerStart := time.Now()
					if !ok {
						providerErr := clierr.New(clierr.CodeUnsupported, fmt.Sprintf("yield provider %s does not support history", providerName))
						statuses = append(statuses, model.ProviderStatus{Name: provider.Info().Name, Status: statusFromErr(providerErr), LatencyMS: time.Since(providerStart).Milliseconds()})
						warnings = append(warnings, fmt.Sprintf("provider %s does not support yield history", provider.Info().Name))
						partial = true
						if firstErr == nil {
							firstErr = providerErr
						}
						continue
					}

					discoveryReq := providers.YieldRequest{
						Chain:             chain,
						Asset:             asset,
						Limit:             historyLimit,
						MinTVLUSD:         0,
						MinAPY:            0,
						SortBy:            "apy_total",
						IncludeIncomplete: true,
					}
					if len(opportunityIDSet) > 0 {
						discoveryReq.Limit = 0
					}
					opportunities, providerErr := provider.YieldOpportunities(ctx, discoveryReq)
					if providerErr != nil {
						statuses = append(statuses, model.ProviderStatus{Name: provider.Info().Name, Status: statusFromErr(providerErr), LatencyMS: time.Since(providerStart).Milliseconds()})
						warnings = append(warnings, fmt.Sprintf("provider %s failed during opportunity lookup: %v", provider.Info().Name, providerErr))
						partial = true
						if firstErr == nil {
							firstErr = providerErr
						}
						continue
					}
					if len(opportunityIDSet) > 0 {
						opportunities = filterYieldOpportunitiesByID(opportunities, opportunityIDSet)
					}
					if historyLimit > 0 && len(opportunities) > historyLimit {
						opportunities = opportunities[:historyLimit]
					}
					if len(opportunities) == 0 {
						providerErr = clierr.New(clierr.CodeUnavailable, fmt.Sprintf("provider %s returned no matching opportunities", providerName))
						statuses = append(statuses, model.ProviderStatus{Name: provider.Info().Name, Status: statusFromErr(providerErr), LatencyMS: time.Since(providerStart).Milliseconds()})
						warnings = append(warnings, fmt.Sprintf("provider %s returned no matching opportunities", provider.Info().Name))
						partial = true
						if firstErr == nil {
							firstErr = providerErr
						}
						continue
					}

					providerSeries := make([]model.YieldHistorySeries, 0, len(opportunities)*len(metrics))
					var providerHistoryErr error
					for _, opportunity := range opportunities {
						series, err := historyProvider.YieldHistory(ctx, providers.YieldHistoryRequest{
							Opportunity: opportunity,
							StartTime:   startTime,
							EndTime:     endTime,
							Interval:    interval,
							Metrics:     metrics,
						})
						if err != nil {
							partial = true
							warnings = append(warnings, fmt.Sprintf("provider %s failed history for opportunity %s: %v", provider.Info().Name, opportunity.OpportunityID, err))
							if providerHistoryErr == nil {
								providerHistoryErr = err
							}
							continue
						}
						providerSeries = append(providerSeries, series...)
					}

					statusErr := providerHistoryErr
					if len(providerSeries) == 0 && statusErr == nil {
						statusErr = clierr.New(clierr.CodeUnavailable, fmt.Sprintf("provider %s returned no historical points", providerName))
					}
					statuses = append(statuses, model.ProviderStatus{Name: provider.Info().Name, Status: statusFromErr(statusErr), LatencyMS: time.Since(providerStart).Milliseconds()})
					if statusErr != nil && firstErr == nil {
						firstErr = statusErr
					}
					combined = append(combined, providerSeries...)
				}

				if len(combined) == 0 {
					if firstErr != nil {
						return nil, statuses, warnings, partial, firstErr
					}
					return nil, statuses, warnings, partial, clierr.New(clierr.CodeUnavailable, "no yield history returned by selected providers")
				}

				sortYieldHistorySeries(combined)
				return combined, statuses, warnings, partial, nil
			})
		},
	}
	historyCmd.Flags().StringVar(&historyChainArg, "chain", "", "Chain identifier")
	historyCmd.Flags().StringVar(&historyAssetArg, "asset", "", "Asset symbol/address/CAIP-19")
	historyCmd.Flags().StringVar(&historyProvidersArg, "providers", "", "Filter by provider names (aave,morpho,kamino)")
	historyCmd.Flags().StringVar(&historyMetricsArg, "metrics", "apy_total", "History metrics (apy_total,tvl_usd)")
	historyCmd.Flags().StringVar(&historyIntervalArg, "interval", "day", "Point interval (hour|day)")
	historyCmd.Flags().StringVar(&historyWindowArg, "window", "7d", "Lookback window (for example 24h,7d,30d)")
	historyCmd.Flags().StringVar(&historyFromArg, "from", "", "Start time (RFC3339). Overrides --window when set")
	historyCmd.Flags().StringVar(&historyToArg, "to", "", "End time (RFC3339). Defaults to now")
	historyCmd.Flags().StringVar(&historyOpportunityIDsArg, "opportunity-ids", "", "Optional comma-separated opportunity IDs from yield opportunities")
	historyCmd.Flags().IntVar(&historyLimit, "limit", 20, "Maximum opportunities per provider to fetch history for")
	_ = historyCmd.MarkFlagRequired("chain")
	_ = historyCmd.MarkFlagRequired("asset")
	root.AddCommand(historyCmd)

	s.addYieldExecutionSubcommands(root)
	return root
}

type fetchFn func(ctx context.Context) (data any, providerStatus []model.ProviderStatus, warnings []string, partial bool, err error)

func (s *runtimeState) runCachedCommand(commandPath, key string, ttl time.Duration, fetch fetchFn) error {
	s.resetCommandDiagnostics()
	cacheStatus := cacheMetaMiss()
	warnings := []string{}
	var staleData any
	staleAvailable := false
	staleObservedAge := time.Duration(0)
	staleObservedAt := time.Time{}
	staleCacheStatus := cacheMetaMiss()

	if s.settings.CacheEnabled && s.cache != nil {
		cached, err := s.cache.Get(key, s.settings.MaxStale)
		if err == nil && cached.Hit {
			entryStatus := model.CacheStatus{Status: "hit", AgeMS: cached.Age.Milliseconds(), Stale: cached.Stale}
			if !cached.Stale {
				var data any
				if err := json.Unmarshal(cached.Value, &data); err == nil {
					s.captureCommandDiagnostics(warnings, nil, false)
					return s.emitSuccess(commandPath, data, warnings, entryStatus, nil, false)
				}
			} else {
				var data any
				if err := json.Unmarshal(cached.Value, &data); err == nil {
					staleData = data
					staleAvailable = true
					staleObservedAge = cached.Age
					staleObservedAt = time.Now()
					staleCacheStatus = entryStatus
				}
			}
		}
	}

	ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
	defer cancel()
	data, providerStatus, providerWarnings, partial, err := fetch(ctx)
	warnings = append(warnings, providerWarnings...)
	s.captureCommandDiagnostics(warnings, providerStatus, partial)
	if err != nil {
		if staleAvailable {
			if !staleFallbackAllowed(err) {
				return err
			}
			currentStaleAge := staleObservedAge
			if !staleObservedAt.IsZero() {
				currentStaleAge += time.Since(staleObservedAt)
			}
			staleCacheStatus.AgeMS = currentStaleAge.Milliseconds()
			if s.settings.NoStale {
				return clierr.Wrap(clierr.CodeStale, "fresh provider fetch failed and stale fallback is disabled (--no-stale)", err)
			}
			if staleExceedsBudget(currentStaleAge, ttl, s.settings.MaxStale) {
				return clierr.Wrap(clierr.CodeStale, "fresh provider fetch failed and cached data exceeded stale budget", err)
			}
			warnings = append(warnings, "provider fetch failed; serving stale data within max-stale budget")
			s.captureCommandDiagnostics(warnings, providerStatus, false)
			return s.emitSuccess(commandPath, staleData, warnings, staleCacheStatus, providerStatus, false)
		}
		return err
	}

	if partial && s.settings.Strict {
		s.captureCommandDiagnostics(warnings, providerStatus, true)
		return clierr.New(clierr.CodePartialStrict, "partial results returned in strict mode")
	}

	if s.settings.CacheEnabled && s.cache != nil {
		if payload, err := json.Marshal(data); err == nil {
			_ = s.cache.Set(key, payload, ttl)
			cacheStatus = model.CacheStatus{Status: "write", AgeMS: 0, Stale: false}
		}
	}

	s.captureCommandDiagnostics(warnings, providerStatus, partial)
	return s.emitSuccess(commandPath, data, warnings, cacheStatus, providerStatus, partial)
}

func (s *runtimeState) emitSuccess(commandPath string, data any, warnings []string, cacheStatus model.CacheStatus, providers []model.ProviderStatus, partial bool) error {
	env := model.Envelope{
		Version:  model.EnvelopeVersion,
		Success:  true,
		Data:     data,
		Error:    nil,
		Warnings: warnings,
		Meta: model.EnvelopeMeta{
			RequestID: newRequestID(),
			Timestamp: s.runner.now().UTC(),
			Command:   commandPath,
			Providers: providers,
			Cache:     cacheStatus,
			Partial:   partial,
		},
	}
	return out.Render(s.runner.stdout, env, s.settings)
}

func (s *runtimeState) renderError(commandPath string, err error, warnings []string, providers []model.ProviderStatus, partial bool) {
	if strings.TrimSpace(commandPath) == "" {
		commandPath = s.lastCommand
		if commandPath == "" {
			commandPath = version.CLIName
		}
	}
	code := clierr.ExitCode(err)
	typ := "internal_error"
	message := err.Error()
	if cErr, ok := clierr.As(err); ok {
		message = cErr.Message
		if cErr.Cause != nil {
			message = fmt.Sprintf("%s: %v", cErr.Message, cErr.Cause)
		}
		switch cErr.Code {
		case clierr.CodeUsage:
			typ = "usage_error"
		case clierr.CodeAuth:
			typ = "auth_error"
		case clierr.CodeRateLimited:
			typ = "rate_limited"
		case clierr.CodeUnavailable:
			typ = "provider_unavailable"
		case clierr.CodeUnsupported:
			typ = "unsupported"
		case clierr.CodeStale:
			typ = "stale_data"
		case clierr.CodePartialStrict:
			typ = "partial_results"
		case clierr.CodeBlocked:
			typ = "command_blocked"
		case clierr.CodeActionPlan:
			typ = "action_plan_error"
		case clierr.CodeActionSim:
			typ = "action_simulation_error"
		case clierr.CodeActionPolicy:
			typ = "action_policy_error"
		case clierr.CodeActionTimeout:
			typ = "action_timeout"
		case clierr.CodeSigner:
			typ = "signer_error"
		}
	}

	settings := s.settings
	if settings.OutputMode == "" {
		settings.OutputMode = "json"
	}
	settings.ResultsOnly = false
	settings.SelectFields = nil
	env := model.Envelope{
		Version: model.EnvelopeVersion,
		Success: false,
		Data:    []any{},
		Error: &model.ErrorBody{
			Code:    code,
			Type:    typ,
			Message: message,
		},
		Warnings: warnings,
		Meta: model.EnvelopeMeta{
			RequestID: newRequestID(),
			Timestamp: s.runner.now().UTC(),
			Command:   commandPath,
			Providers: providers,
			Cache:     cacheMetaBypass(),
			Partial:   partial,
		},
	}
	_ = out.Render(s.runner.stderr, env, settings)
}

func normalizeLendingProvider(input string) string {
	return providers.NormalizeLendingProvider(input)
}

func parseLendPositionType(input string) (providers.LendPositionType, error) {
	switch strings.ToLower(strings.TrimSpace(input)) {
	case "", string(providers.LendPositionTypeAll):
		return providers.LendPositionTypeAll, nil
	case string(providers.LendPositionTypeSupply):
		return providers.LendPositionTypeSupply, nil
	case string(providers.LendPositionTypeBorrow):
		return providers.LendPositionTypeBorrow, nil
	case string(providers.LendPositionTypeCollateral):
		return providers.LendPositionTypeCollateral, nil
	default:
		return "", clierr.New(clierr.CodeUsage, "--type must be one of: all,supply,borrow,collateral")
	}
}

func (s *runtimeState) selectLendingProvider(providerName string) (providers.LendingProvider, error) {
	primary, ok := s.lendingProviders[providerName]
	if !ok {
		return nil, clierr.New(clierr.CodeUnsupported, fmt.Sprintf("unsupported lending provider: %s", providerName))
	}
	return primary, nil
}

func (s *runtimeState) selectYieldProviders(filter []string, chain id.Chain) ([]string, error) {
	if len(filter) == 0 {
		keys := make([]string, 0, len(s.yieldProviders))
		for name := range s.yieldProviders {
			if !yieldProviderSupportsChain(name, chain) {
				continue
			}
			keys = append(keys, name)
		}
		sort.Strings(keys)
		return keys, nil
	}

	selected := make([]string, 0, len(filter))
	seen := map[string]struct{}{}
	for _, item := range filter {
		name := strings.ToLower(strings.TrimSpace(item))
		if _, ok := s.yieldProviders[name]; !ok {
			return nil, clierr.New(clierr.CodeUsage, fmt.Sprintf("unsupported yield provider: %s", item))
		}
		if _, exists := seen[name]; exists {
			continue
		}
		seen[name] = struct{}{}
		selected = append(selected, name)
	}
	sort.Strings(selected)
	return selected, nil
}

func yieldProviderSupportsChain(name string, chain id.Chain) bool {
	switch name {
	case "kamino":
		return chain.IsSolana()
	case "aave", "morpho":
		return chain.IsEVM()
	case "moonwell":
		return chain.IsEVM() && (chain.EVMChainID == 8453 || chain.EVMChainID == 10)
	default:
		return true
	}
}

func dedupeYieldByOpportunityID(items []model.YieldOpportunity) []model.YieldOpportunity {
	if len(items) <= 1 {
		return items
	}
	byID := make(map[string]model.YieldOpportunity, len(items))
	for _, item := range items {
		existing, ok := byID[item.OpportunityID]
		if !ok || compareYieldOpportunities(item, existing, "apy_total") {
			byID[item.OpportunityID] = item
		}
	}
	out := make([]model.YieldOpportunity, 0, len(byID))
	for _, item := range byID {
		out = append(out, item)
	}
	return out
}

func sortYieldOpportunities(items []model.YieldOpportunity, sortBy string) {
	sortBy = strings.ToLower(strings.TrimSpace(sortBy))
	if sortBy == "" {
		sortBy = "apy_total"
	}
	sort.Slice(items, func(i, j int) bool {
		return compareYieldOpportunities(items[i], items[j], sortBy)
	})
}

func compareYieldOpportunities(a, b model.YieldOpportunity, sortBy string) bool {
	switch sortBy {
	case "tvl_usd":
		if a.TVLUSD != b.TVLUSD {
			return a.TVLUSD > b.TVLUSD
		}
	case "liquidity_usd":
		if a.LiquidityUSD != b.LiquidityUSD {
			return a.LiquidityUSD > b.LiquidityUSD
		}
	default:
		if a.APYTotal != b.APYTotal {
			return a.APYTotal > b.APYTotal
		}
	}
	if a.APYTotal != b.APYTotal {
		return a.APYTotal > b.APYTotal
	}
	if a.TVLUSD != b.TVLUSD {
		return a.TVLUSD > b.TVLUSD
	}
	if a.LiquidityUSD != b.LiquidityUSD {
		return a.LiquidityUSD > b.LiquidityUSD
	}
	return strings.Compare(a.OpportunityID, b.OpportunityID) < 0
}

func filterYieldOpportunitiesByID(items []model.YieldOpportunity, ids map[string]struct{}) []model.YieldOpportunity {
	if len(ids) == 0 {
		return items
	}
	out := make([]model.YieldOpportunity, 0, len(items))
	for _, item := range items {
		if _, ok := ids[strings.ToLower(strings.TrimSpace(item.OpportunityID))]; ok {
			out = append(out, item)
		}
	}
	return out
}

func sortYieldHistorySeries(items []model.YieldHistorySeries) {
	for i := range items {
		sort.Slice(items[i].Points, func(a, b int) bool {
			return strings.Compare(items[i].Points[a].Timestamp, items[i].Points[b].Timestamp) < 0
		})
	}
	sort.Slice(items, func(i, j int) bool {
		a, b := items[i], items[j]
		if a.Provider != b.Provider {
			return a.Provider < b.Provider
		}
		if a.OpportunityID != b.OpportunityID {
			return a.OpportunityID < b.OpportunityID
		}
		if a.Metric != b.Metric {
			return a.Metric < b.Metric
		}
		if a.Interval != b.Interval {
			return a.Interval < b.Interval
		}
		return strings.Compare(a.StartTime, b.StartTime) < 0
	})
}

func sortYieldPositions(items []model.YieldPosition) {
	sort.Slice(items, func(i, j int) bool {
		if items[i].AmountUSD != items[j].AmountUSD {
			return items[i].AmountUSD > items[j].AmountUSD
		}
		if items[i].APYTotal != items[j].APYTotal {
			return items[i].APYTotal > items[j].APYTotal
		}
		if items[i].Provider != items[j].Provider {
			return items[i].Provider < items[j].Provider
		}
		if items[i].AssetID != items[j].AssetID {
			return items[i].AssetID < items[j].AssetID
		}
		return items[i].ProviderNativeID < items[j].ProviderNativeID
	})
}

func parseYieldHistoryMetrics(input string) ([]providers.YieldHistoryMetric, error) {
	parts := splitCSV(input)
	if len(parts) == 0 {
		parts = []string{string(providers.YieldHistoryMetricAPYTotal)}
	}
	out := make([]providers.YieldHistoryMetric, 0, len(parts))
	seen := map[providers.YieldHistoryMetric]struct{}{}
	for _, part := range parts {
		var metric providers.YieldHistoryMetric
		switch strings.ToLower(strings.TrimSpace(part)) {
		case string(providers.YieldHistoryMetricAPYTotal):
			metric = providers.YieldHistoryMetricAPYTotal
		case string(providers.YieldHistoryMetricTVLUSD):
			metric = providers.YieldHistoryMetricTVLUSD
		default:
			return nil, clierr.New(clierr.CodeUsage, "--metrics must be one or more of: apy_total,tvl_usd")
		}
		if _, ok := seen[metric]; ok {
			continue
		}
		seen[metric] = struct{}{}
		out = append(out, metric)
	}
	return out, nil
}

func parseYieldHistoryInterval(input string) (providers.YieldHistoryInterval, error) {
	switch strings.ToLower(strings.TrimSpace(input)) {
	case "", "day", "daily", "1d":
		return providers.YieldHistoryIntervalDay, nil
	case "hour", "hourly", "1h":
		return providers.YieldHistoryIntervalHour, nil
	default:
		return "", clierr.New(clierr.CodeUsage, "--interval must be one of: hour,day")
	}
}

func resolveYieldHistoryRange(fromArg, toArg, windowArg string, now time.Time) (time.Time, time.Time, error) {
	endTime := now.UTC()
	if strings.TrimSpace(toArg) != "" {
		parsed, err := parseRFC3339(toArg)
		if err != nil {
			return time.Time{}, time.Time{}, clierr.Wrap(clierr.CodeUsage, "parse --to", err)
		}
		endTime = parsed.UTC()
	}
	if endTime.After(now.Add(5 * time.Minute)) {
		return time.Time{}, time.Time{}, clierr.New(clierr.CodeUsage, "--to cannot be in the future")
	}

	var startTime time.Time
	if strings.TrimSpace(fromArg) != "" {
		parsed, err := parseRFC3339(fromArg)
		if err != nil {
			return time.Time{}, time.Time{}, clierr.Wrap(clierr.CodeUsage, "parse --from", err)
		}
		startTime = parsed.UTC()
	} else {
		window, err := parseLookbackWindow(windowArg)
		if err != nil {
			return time.Time{}, time.Time{}, clierr.Wrap(clierr.CodeUsage, "parse --window", err)
		}
		startTime = endTime.Add(-window)
	}

	if !startTime.Before(endTime) {
		return time.Time{}, time.Time{}, clierr.New(clierr.CodeUsage, "history range must have --from before --to")
	}
	if endTime.Sub(startTime) > 366*24*time.Hour {
		return time.Time{}, time.Time{}, clierr.New(clierr.CodeUsage, "history range cannot exceed 366d")
	}
	return startTime, endTime, nil
}

func parseRFC3339(raw string) (time.Time, error) {
	value := strings.TrimSpace(raw)
	if value == "" {
		return time.Time{}, fmt.Errorf("empty timestamp")
	}
	ts, err := time.Parse(time.RFC3339, value)
	if err == nil {
		return ts, nil
	}
	ts, err = time.Parse(time.RFC3339Nano, value)
	if err == nil {
		return ts, nil
	}
	return time.Time{}, fmt.Errorf("expected RFC3339 timestamp")
}

func parseLookbackWindow(raw string) (time.Duration, error) {
	value := strings.ToLower(strings.TrimSpace(raw))
	if value == "" {
		value = "7d"
	}
	switch {
	case strings.HasSuffix(value, "d"):
		n, err := strconv.Atoi(strings.TrimSuffix(value, "d"))
		if err != nil || n <= 0 {
			return 0, fmt.Errorf("invalid day window")
		}
		return time.Duration(n) * 24 * time.Hour, nil
	case strings.HasSuffix(value, "w"):
		n, err := strconv.Atoi(strings.TrimSuffix(value, "w"))
		if err != nil || n <= 0 {
			return 0, fmt.Errorf("invalid week window")
		}
		return time.Duration(n) * 7 * 24 * time.Hour, nil
	default:
		d, err := time.ParseDuration(value)
		if err != nil || d <= 0 {
			return 0, fmt.Errorf("invalid duration window")
		}
		return d, nil
	}
}

// rpcConfigurable is implemented by providers that support per-request RPC
// URL overrides (e.g. Moonwell, which reads data via on-chain multicalls).
type rpcConfigurable interface {
	SetRPCOverride(url string)
}

// applyRPCOverride sets the RPC URL on providers that support it. The
// provider is tested via interface assertion so non-RPC providers are
// silently ignored.
func applyRPCOverride(provider any, rpcURL string) {
	if url := strings.TrimSpace(rpcURL); url != "" {
		if p, ok := provider.(rpcConfigurable); ok {
			p.SetRPCOverride(url)
		}
	}
}

func applyLendMarketLimit(items []model.LendMarket, limit int) []model.LendMarket {
	if limit <= 0 || len(items) <= limit {
		return items
	}
	return items[:limit]
}

func applyLendRateLimit(items []model.LendRate, limit int) []model.LendRate {
	if limit <= 0 || len(items) <= limit {
		return items
	}
	return items[:limit]
}

func parseChainAsset(chainArg, assetArg string) (id.Chain, id.Asset, error) {
	if strings.TrimSpace(chainArg) == "" {
		return id.Chain{}, id.Asset{}, clierr.New(clierr.CodeUsage, "--chain is required")
	}
	if strings.TrimSpace(assetArg) == "" {
		return id.Chain{}, id.Asset{}, clierr.New(clierr.CodeUsage, "--asset is required")
	}
	chain, err := id.ParseChain(chainArg)
	if err != nil {
		return id.Chain{}, id.Asset{}, err
	}
	asset, err := id.ParseAsset(assetArg, chain)
	if err != nil {
		return id.Chain{}, id.Asset{}, err
	}
	return chain, asset, nil
}

func parseOptionalChainAsset(chain id.Chain, assetArg string) (id.Asset, error) {
	assetArg = strings.TrimSpace(assetArg)
	if assetArg == "" {
		return id.Asset{}, nil
	}

	asset, err := id.ParseAsset(assetArg, chain)
	if err == nil {
		return asset, nil
	}

	if looksLikeAddressOrCAIP(assetArg) || !looksLikeSymbolFilter(assetArg) {
		return id.Asset{}, err
	}

	return id.Asset{
		ChainID: chain.CAIP2,
		Symbol:  strings.ToUpper(assetArg),
	}, nil
}

func parseChainAssetFilter(chain id.Chain, assetArg string) (id.Asset, error) {
	assetArg = strings.TrimSpace(assetArg)
	if assetArg == "" {
		return id.Asset{}, nil
	}

	asset, err := id.ParseAsset(assetArg, chain)
	if err == nil {
		if strings.TrimSpace(asset.Symbol) == "" {
			return id.Asset{}, clierr.New(clierr.CodeUsage, "asset filter by address/CAIP requires a known token symbol on the selected chain")
		}
		return asset, nil
	}

	if looksLikeAddressOrCAIP(assetArg) || !looksLikeSymbolFilter(assetArg) {
		return id.Asset{}, err
	}

	return id.Asset{
		ChainID: chain.CAIP2,
		Symbol:  strings.ToUpper(assetArg),
	}, nil
}

func looksLikeAddressOrCAIP(input string) bool {
	norm := strings.ToLower(strings.TrimSpace(input))
	return strings.HasPrefix(norm, "eip155:") || (strings.HasPrefix(norm, "0x") && len(norm) == 42)
}

func looksLikeSymbolFilter(input string) bool {
	norm := strings.TrimSpace(input)
	if norm == "" || len(norm) > 64 {
		return false
	}
	if strings.ContainsAny(norm, " \t\r\n:/") {
		return false
	}
	return true
}

func chainAssetFilterCacheValue(asset id.Asset, rawInput string) string {
	if strings.TrimSpace(rawInput) == "" {
		return ""
	}
	if strings.TrimSpace(asset.AssetID) != "" {
		return asset.AssetID
	}
	if strings.TrimSpace(asset.Symbol) != "" {
		return "symbol:" + strings.ToUpper(strings.TrimSpace(asset.Symbol))
	}
	return "raw:" + strings.ToUpper(strings.TrimSpace(rawInput))
}

func fetchGasPrice(ctx context.Context, chain id.Chain, rpcURL string, now func() time.Time) (model.GasPrice, error) {
	client, err := ethclient.DialContext(ctx, rpcURL)
	if err != nil {
		return model.GasPrice{}, clierr.Wrap(clierr.CodeUnavailable, "connect rpc", err)
	}
	defer client.Close()

	header, err := client.HeaderByNumber(ctx, nil)
	if err != nil {
		return model.GasPrice{}, clierr.Wrap(clierr.CodeUnavailable, "fetch block header", err)
	}

	gasPrice, err := client.SuggestGasPrice(ctx)
	if err != nil {
		return model.GasPrice{}, clierr.Wrap(clierr.CodeUnavailable, "fetch gas price", err)
	}

	eip1559 := header.BaseFee != nil
	var baseFee, priorityFee *big.Int
	var warnings []string
	if eip1559 {
		baseFee = header.BaseFee
		priorityFee, err = client.SuggestGasTipCap(ctx)
		if err != nil {
			priorityFee = new(big.Int)
			warnings = append(warnings, fmt.Sprintf("priority fee unavailable: %v", err))
		}
	}

	result := model.GasPrice{
		ChainID:      chain.CAIP2,
		ChainName:    chain.Name,
		BlockNumber:  header.Number.Int64(),
		EIP1559:      eip1559,
		GasPriceGwei: weiToGwei(gasPrice),
		Warnings:     warnings,
		FetchedAt:    now().UTC().Format(time.RFC3339),
	}
	if eip1559 {
		result.BaseFeeGwei = weiToGwei(baseFee)
		result.PriorityFeeGwei = weiToGwei(priorityFee)
	}
	return result, nil
}

func weiToGwei(wei *big.Int) string {
	if wei == nil {
		return "0"
	}
	gwei := new(big.Float).SetInt(wei)
	gwei.Quo(gwei, big.NewFloat(1e9))
	return gwei.Text('f', 6)
}

func cacheKey(commandPath string, req any) string {
	buf, _ := json.Marshal(req)
	prefix := []byte(commandPath + "|" + cachePayloadSchemaVersion + "|")
	sum := sha256.Sum256(append(prefix, buf...))
	return hex.EncodeToString(sum[:])
}

func newRequestID() string {
	buf := make([]byte, 16)
	_, _ = rand.Read(buf)
	return hex.EncodeToString(buf)
}

func splitCSV(v string) []string {
	if strings.TrimSpace(v) == "" {
		return nil
	}
	parts := strings.Split(v, ",")
	out := make([]string, 0, len(parts))
	for _, part := range parts {
		norm := strings.ToLower(strings.TrimSpace(part))
		if norm != "" {
			out = append(out, norm)
		}
	}
	return out
}

func trimRootPath(path string) string {
	parts := strings.Fields(path)
	if len(parts) <= 1 {
		return path
	}
	return strings.Join(parts[1:], " ")
}

func statusFromErr(err error) string {
	if err == nil {
		return "ok"
	}
	if cErr, ok := clierr.As(err); ok {
		switch cErr.Code {
		case clierr.CodeAuth:
			return "auth_error"
		case clierr.CodeRateLimited:
			return "rate_limited"
		case clierr.CodeUnavailable:
			return "unavailable"
		default:
			return "error"
		}
	}
	return "error"
}

func cacheMetaBypass() model.CacheStatus {
	return model.CacheStatus{Status: "bypass", AgeMS: 0, Stale: false}
}

func cacheMetaMiss() model.CacheStatus {
	return model.CacheStatus{Status: "miss", AgeMS: 0, Stale: false}
}

func normalizeRunError(err error) error {
	if err == nil {
		return nil
	}
	if _, ok := clierr.As(err); ok {
		return err
	}
	if isLikelyUsageError(err) {
		return clierr.Wrap(clierr.CodeUsage, "invalid command input", err)
	}
	return clierr.Wrap(clierr.CodeInternal, "execute command", err)
}

func isLikelyUsageError(err error) bool {
	if err == nil {
		return false
	}
	msg := strings.ToLower(strings.TrimSpace(err.Error()))
	patterns := []string{
		"unknown command",
		"unknown flag",
		"required flag(s)",
		"flag needs an argument",
		"requires at least",
		"requires exactly",
		"accepts ",
		"invalid argument",
		"invalid args",
	}
	for _, p := range patterns {
		if strings.Contains(msg, p) {
			return true
		}
	}
	return false
}

func staleExceedsBudget(age, ttl, maxStale time.Duration) bool {
	if age <= ttl {
		return false
	}
	if maxStale < 0 {
		return false
	}
	return age > ttl+maxStale
}

func staleFallbackAllowed(err error) bool {
	cErr, ok := clierr.As(err)
	if !ok {
		return false
	}
	return cErr.Code == clierr.CodeUnavailable || cErr.Code == clierr.CodeRateLimited
}

func shouldOpenCache(commandPath string) bool {
	path := normalizeCommandPath(commandPath)
	switch path {
	case "", "version", "schema", "providers", "providers list", "chains list", "chains gas":
		return false
	}
	if isExecutionCommandPath(path) {
		return false
	}
	return true
}

func shouldOpenActionStore(commandPath string) bool {
	return isExecutionCommandPath(normalizeCommandPath(commandPath))
}

func normalizeCommandPath(commandPath string) string {
	return strings.Join(strings.Fields(strings.ToLower(strings.TrimSpace(commandPath))), " ")
}

func isExecutionCommandPath(path string) bool {
	switch path {
	case "actions", "actions list", "actions show", "actions estimate":
		return true
	}
	parts := strings.Fields(path)
	if len(parts) < 2 {
		return false
	}
	switch parts[0] {
	case "swap", "bridge", "approvals", "transfer", "lend", "rewards", "yield":
		last := parts[len(parts)-1]
		return last == "plan" || last == "submit" || last == "status"
	default:
		return false
	}
}

func assetHasResolvedSymbol(asset id.Asset) bool {
	return strings.TrimSpace(asset.Symbol) != ""
}

func (s *runtimeState) ensureActionStore() error {
	if s.actionStore != nil {
		return nil
	}
	path := strings.TrimSpace(s.settings.ActionStorePath)
	lockPath := strings.TrimSpace(s.settings.ActionLockPath)
	if path == "" || lockPath == "" {
		defaults, err := config.Load(config.GlobalFlags{})
		if err != nil {
			return clierr.Wrap(clierr.CodeInternal, "resolve default action store settings", err)
		}
		if path == "" {
			path = defaults.ActionStorePath
		}
		if lockPath == "" {
			lockPath = defaults.ActionLockPath
		}
	}
	store, err := execution.OpenStore(path, lockPath)
	if err != nil {
		return clierr.Wrap(clierr.CodeInternal, "open action store", err)
	}
	s.actionStore = store
	return nil
}

func (s *runtimeState) actionBuilderRegistry() *actionbuilder.Registry {
	if s.actionBuilder == nil {
		s.actionBuilder = actionbuilder.New(s.swapProviders, s.bridgeProviders)
	} else {
		s.actionBuilder.Configure(s.swapProviders, s.bridgeProviders)
	}
	return s.actionBuilder
}

func resolveActionID(actionID string) (string, error) {
	actionID = strings.TrimSpace(actionID)
	if actionID == "" {
		return "", clierr.New(clierr.CodeUsage, "action id is required (--action-id)")
	}
	if !actionIDPattern.MatchString(actionID) {
		return "", clierr.New(clierr.CodeUsage, "action id must match act_<32 hex chars>")
	}
	return actionID, nil
}

func applyExecutionIdentityToAction(action *execution.Action, identity executionIdentity) {
	if action == nil {
		return
	}
	action.WalletID = identity.WalletID
	action.WalletName = identity.WalletName
	action.FromAddress = identity.FromAddress
	action.ExecutionBackend = identity.ExecutionBackend
}

func newExecutionSigner(signerBackend, keySource, privateKey string) (execsigner.Signer, error) {
	signerBackend = strings.ToLower(strings.TrimSpace(signerBackend))
	if signerBackend == "" {
		signerBackend = "local"
	}
	switch signerBackend {
	case "local":
		localSigner, err := execsigner.NewLocalSignerFromInputs(keySource, privateKey)
		if err != nil {
			return nil, clierr.Wrap(clierr.CodeSigner, "initialize local signer", err)
		}
		return localSigner, nil
	case "tempo":
		if privateKey != "" {
			return nil, clierr.New(clierr.CodeUsage, "--signer tempo cannot be combined with --private-key; tempo wallet manages keys automatically")
		}
		tempoSigner, warnings, err := execsigner.NewTempoSignerFromCLI()
		if err != nil {
			return nil, clierr.Wrap(clierr.CodeSigner, "tempo wallet", err)
		}
		for _, w := range warnings {
			fmt.Fprintf(os.Stderr, "warning: %s\n", w)
		}
		return tempoSigner, nil
	default:
		return nil, clierr.New(clierr.CodeUnsupported, fmt.Sprintf("unsupported signer backend %q (expected local|tempo)", signerBackend))
	}
}

// effectiveSenderAddress returns the on-chain sender address for the given signer.
// For Tempo smart-wallet signers, this is the wallet address rather than the
// signing key address.
func effectiveSenderAddress(txSigner execsigner.Signer) string {
	if ts, ok := txSigner.(execsigner.TempoSigner); ok {
		return ts.WalletAddress().Hex()
	}
	return txSigner.Address().Hex()
}

func parseExecuteOptions(
	simulate bool,
	pollInterval, stepTimeout string,
	gasMultiplier float64,
	maxFeeGwei, maxPriorityFeeGwei string,
	allowMaxApproval bool,
	unsafeProviderTx bool,
	feeToken string,
) (execution.ExecuteOptions, error) {
	opts := execution.DefaultExecuteOptions()
	opts.Simulate = simulate
	if strings.TrimSpace(pollInterval) != "" {
		d, err := time.ParseDuration(pollInterval)
		if err != nil {
			return execution.ExecuteOptions{}, clierr.Wrap(clierr.CodeUsage, "parse --poll-interval", err)
		}
		if d <= 0 {
			return execution.ExecuteOptions{}, clierr.New(clierr.CodeUsage, "--poll-interval must be > 0")
		}
		opts.PollInterval = d
	}
	if strings.TrimSpace(stepTimeout) != "" {
		d, err := time.ParseDuration(stepTimeout)
		if err != nil {
			return execution.ExecuteOptions{}, clierr.Wrap(clierr.CodeUsage, "parse --step-timeout", err)
		}
		if d <= 0 {
			return execution.ExecuteOptions{}, clierr.New(clierr.CodeUsage, "--step-timeout must be > 0")
		}
		opts.StepTimeout = d
	}
	if gasMultiplier <= 1 {
		return execution.ExecuteOptions{}, clierr.New(clierr.CodeUsage, "--gas-multiplier must be > 1")
	}
	opts.GasMultiplier = gasMultiplier
	opts.MaxFeeGwei = strings.TrimSpace(maxFeeGwei)
	opts.MaxPriorityFeeGwei = strings.TrimSpace(maxPriorityFeeGwei)
	opts.AllowMaxApproval = allowMaxApproval
	opts.UnsafeProviderTx = unsafeProviderTx
	opts.FeeToken = strings.TrimSpace(feeToken)
	return opts, nil
}

func parseActionEstimateOptions(
	stepIDsCSV string,
	gasMultiplier float64,
	maxFeeGwei, maxPriorityFeeGwei, blockTag string,
) (execution.EstimateOptions, error) {
	opts := execution.DefaultEstimateOptions()
	opts.StepIDs = splitCSV(stepIDsCSV)
	if gasMultiplier <= 1 {
		return execution.EstimateOptions{}, clierr.New(clierr.CodeUsage, "--gas-multiplier must be > 1")
	}
	opts.GasMultiplier = gasMultiplier
	opts.MaxFeeGwei = strings.TrimSpace(maxFeeGwei)
	opts.MaxPriorityFeeGwei = strings.TrimSpace(maxPriorityFeeGwei)
	switch strings.ToLower(strings.TrimSpace(blockTag)) {
	case "", string(execution.EstimateBlockTagPending):
		opts.BlockTag = execution.EstimateBlockTagPending
	case string(execution.EstimateBlockTagLatest):
		opts.BlockTag = execution.EstimateBlockTagLatest
	default:
		return execution.EstimateOptions{}, clierr.New(clierr.CodeUsage, "--block-tag must be one of: pending,latest")
	}
	return opts, nil
}

func (s *runtimeState) resetCommandDiagnostics() {
	s.lastWarnings = nil
	s.lastProviders = nil
	s.lastPartial = false
}

func (s *runtimeState) captureCommandDiagnostics(warnings []string, providers []model.ProviderStatus, partial bool) {
	if len(warnings) == 0 {
		s.lastWarnings = nil
	} else {
		s.lastWarnings = append([]string(nil), warnings...)
	}
	if len(providers) == 0 {
		s.lastProviders = nil
	} else {
		s.lastProviders = append([]model.ProviderStatus(nil), providers...)
	}
	s.lastPartial = partial
}

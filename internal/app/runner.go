package app

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"sort"
	"strings"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/cache"
	"github.com/ggonzalez94/defi-cli/internal/config"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/out"
	"github.com/ggonzalez94/defi-cli/internal/policy"
	"github.com/ggonzalez94/defi-cli/internal/providers"
	"github.com/ggonzalez94/defi-cli/internal/providers/aave"
	"github.com/ggonzalez94/defi-cli/internal/providers/across"
	"github.com/ggonzalez94/defi-cli/internal/providers/defillama"
	"github.com/ggonzalez94/defi-cli/internal/providers/jupiter"
	"github.com/ggonzalez94/defi-cli/internal/providers/kamino"
	"github.com/ggonzalez94/defi-cli/internal/providers/lifi"
	"github.com/ggonzalez94/defi-cli/internal/providers/morpho"
	"github.com/ggonzalez94/defi-cli/internal/providers/oneinch"
	"github.com/ggonzalez94/defi-cli/internal/providers/uniswap"
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
	root          *cobra.Command
	lastCommand   string
	lastWarnings  []string
	lastProviders []model.ProviderStatus
	lastPartial   bool

	marketProvider         providers.MarketDataProvider
	defaultLendingProvider providers.LendingProvider
	lendingProviders       map[string]providers.LendingProvider
	yieldProviders         map[string]providers.YieldProvider
	bridgeProviders        map[string]providers.BridgeProvider
	bridgeDataProviders    map[string]providers.BridgeDataProvider
	swapProviders          map[string]providers.SwapProvider
	providerInfos          []model.ProviderInfo
}

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
		return 0
	}

	state.renderError("", err, state.lastWarnings, state.lastProviders, state.lastPartial)
	if state.cache != nil {
		_ = state.cache.Close()
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
				jupiterProvider := jupiter.New(httpClient, settings.JupiterAPIKey)
				s.marketProvider = llama
				s.defaultLendingProvider = llama
				s.lendingProviders = map[string]providers.LendingProvider{
					"aave":   aaveProvider,
					"morpho": morphoProvider,
					"kamino": kaminoProvider,
					"spark":  llama,
				}
				s.yieldProviders = map[string]providers.YieldProvider{
					"defillama": llama,
					"aave":      aaveProvider,
					"morpho":    morphoProvider,
					"kamino":    kaminoProvider,
				}

				s.bridgeProviders = map[string]providers.BridgeProvider{
					"across": across.New(httpClient),
					"lifi":   lifi.New(httpClient),
				}
				s.bridgeDataProviders = map[string]providers.BridgeDataProvider{
					"defillama": llama,
				}
				s.swapProviders = map[string]providers.SwapProvider{
					"1inch":   oneinch.New(httpClient, settings.OneInchAPIKey),
					"uniswap": uniswap.New(httpClient, settings.UniswapAPIKey),
					"jupiter": jupiterProvider,
				}
				s.providerInfos = []model.ProviderInfo{
					llama.Info(),
					aaveProvider.Info(),
					morphoProvider.Info(),
					kaminoProvider.Info(),
					s.bridgeProviders["across"].Info(),
					s.bridgeProviders["lifi"].Info(),
					s.swapProviders["1inch"].Info(),
					s.swapProviders["uniswap"].Info(),
					s.swapProviders["jupiter"].Info(),
				}
			}

			if settings.CacheEnabled && shouldOpenCache(path) && s.cache == nil {
				cacheStore, err := cache.Open(settings.CachePath, settings.CacheLockPath)
				if err != nil {
					return clierr.Wrap(clierr.CodeInternal, "open cache", err)
				}
				s.cache = cacheStore
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

	cmd.AddCommand(s.newSchemaCommand())
	cmd.AddCommand(s.newProvidersCommand())
	cmd.AddCommand(s.newChainsCommand())
	cmd.AddCommand(s.newProtocolsCommand())
	cmd.AddCommand(s.newAssetsCommand())
	cmd.AddCommand(s.newLendCommand())
	cmd.AddCommand(s.newBridgeCommand())
	cmd.AddCommand(s.newSwapCommand())
	cmd.AddCommand(s.newYieldCommand())
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
	root.AddCommand(list)
	return root
}

func (s *runtimeState) newChainsCommand() *cobra.Command {
	root := &cobra.Command{Use: "chains", Short: "Chain market data"}
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
	root.AddCommand(assetsCmd)

	return root
}

func (s *runtimeState) newProtocolsCommand() *cobra.Command {
	root := &cobra.Command{Use: "protocols", Short: "Protocol market data"}
	var limit int
	var category string
	cmd := &cobra.Command{
		Use:   "top",
		Short: "Top protocols by TVL",
		RunE: func(cmd *cobra.Command, args []string) error {
			req := map[string]any{"category": category, "limit": limit}
			key := cacheKey(trimRootPath(cmd.CommandPath()), req)
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 5*time.Minute, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				start := time.Now()
				data, err := s.marketProvider.ProtocolsTop(ctx, category, limit)
				status := []model.ProviderStatus{{Name: s.marketProvider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, status, nil, false, err
			})
		},
	}
	cmd.Flags().IntVar(&limit, "limit", 20, "Number of protocols to return")
	cmd.Flags().StringVar(&category, "category", "", "Filter by protocol category (e.g. lending)")
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
	var protocolArg string
	var chainArg string
	var assetArg string
	var marketsLimit int

	marketsCmd := &cobra.Command{
		Use:   "markets",
		Short: "List lending markets",
		RunE: func(cmd *cobra.Command, args []string) error {
			protocol := normalizeLendingProtocol(protocolArg)
			if protocol == "" {
				return clierr.New(clierr.CodeUsage, "--protocol is required")
			}
			chain, asset, err := parseChainAsset(chainArg, assetArg)
			if err != nil {
				return err
			}
			req := map[string]any{"protocol": protocol, "chain": chain.CAIP2, "asset": asset.AssetID, "limit": marketsLimit}
			key := cacheKey(trimRootPath(cmd.CommandPath()), req)
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 60*time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				primary, fallback, err := s.selectLendingProviders(protocol)
				if err != nil {
					return nil, nil, nil, false, err
				}
				warnings := []string{}
				statuses := []model.ProviderStatus{}

				start := time.Now()
				data, err := primary.LendMarkets(ctx, protocol, chain, asset)
				statuses = append(statuses, model.ProviderStatus{Name: primary.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()})
				if err == nil || fallback == nil {
					data = applyLendMarketLimit(data, marketsLimit)
					return data, statuses, warnings, false, err
				}
				if !shouldFallback(err) {
					return nil, statuses, warnings, false, err
				}
				if !assetHasResolvedSymbol(asset) {
					warnings = append(warnings, "fallback skipped: asset symbol could not be resolved from input")
					return nil, statuses, warnings, false, err
				}

				start = time.Now()
				fallbackData, fallbackErr := fallback.LendMarkets(ctx, protocol, chain, asset)
				statuses = append(statuses, model.ProviderStatus{Name: fallback.Info().Name, Status: statusFromErr(fallbackErr), LatencyMS: time.Since(start).Milliseconds()})
				if fallbackErr == nil {
					warnings = append(warnings, fmt.Sprintf("primary provider %s failed; using fallback %s", primary.Info().Name, fallback.Info().Name))
					fallbackData = applyLendMarketLimit(fallbackData, marketsLimit)
					return fallbackData, statuses, warnings, false, nil
				}
				return nil, statuses, warnings, false, err
			})
		},
	}
	marketsCmd.Flags().StringVar(&protocolArg, "protocol", "", "Lending protocol (aave, morpho, kamino, spark)")
	marketsCmd.Flags().StringVar(&chainArg, "chain", "", "Chain identifier")
	marketsCmd.Flags().StringVar(&assetArg, "asset", "", "Asset (symbol/address/CAIP-19)")
	marketsCmd.Flags().IntVar(&marketsLimit, "limit", 20, "Maximum lending markets to return")

	var ratesProtocol, ratesChain, ratesAsset string
	var ratesLimit int
	ratesCmd := &cobra.Command{
		Use:   "rates",
		Short: "List lending rates",
		RunE: func(cmd *cobra.Command, args []string) error {
			protocol := normalizeLendingProtocol(ratesProtocol)
			if protocol == "" {
				return clierr.New(clierr.CodeUsage, "--protocol is required")
			}
			chain, asset, err := parseChainAsset(ratesChain, ratesAsset)
			if err != nil {
				return err
			}
			req := map[string]any{"protocol": protocol, "chain": chain.CAIP2, "asset": asset.AssetID, "limit": ratesLimit}
			key := cacheKey(trimRootPath(cmd.CommandPath()), req)
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 30*time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				primary, fallback, err := s.selectLendingProviders(protocol)
				if err != nil {
					return nil, nil, nil, false, err
				}
				warnings := []string{}
				statuses := []model.ProviderStatus{}

				start := time.Now()
				data, err := primary.LendRates(ctx, protocol, chain, asset)
				statuses = append(statuses, model.ProviderStatus{Name: primary.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()})
				if err == nil || fallback == nil {
					data = applyLendRateLimit(data, ratesLimit)
					return data, statuses, warnings, false, err
				}
				if !shouldFallback(err) {
					return nil, statuses, warnings, false, err
				}
				if !assetHasResolvedSymbol(asset) {
					warnings = append(warnings, "fallback skipped: asset symbol could not be resolved from input")
					return nil, statuses, warnings, false, err
				}

				start = time.Now()
				fallbackData, fallbackErr := fallback.LendRates(ctx, protocol, chain, asset)
				statuses = append(statuses, model.ProviderStatus{Name: fallback.Info().Name, Status: statusFromErr(fallbackErr), LatencyMS: time.Since(start).Milliseconds()})
				if fallbackErr == nil {
					warnings = append(warnings, fmt.Sprintf("primary provider %s failed; using fallback %s", primary.Info().Name, fallback.Info().Name))
					fallbackData = applyLendRateLimit(fallbackData, ratesLimit)
					return fallbackData, statuses, warnings, false, nil
				}
				return nil, statuses, warnings, false, err
			})
		},
	}
	ratesCmd.Flags().StringVar(&ratesProtocol, "protocol", "", "Lending protocol (aave, morpho, kamino, spark)")
	ratesCmd.Flags().StringVar(&ratesChain, "chain", "", "Chain identifier")
	ratesCmd.Flags().StringVar(&ratesAsset, "asset", "", "Asset (symbol/address/CAIP-19)")
	ratesCmd.Flags().IntVar(&ratesLimit, "limit", 20, "Maximum lending rates to return")

	root.AddCommand(marketsCmd)
	root.AddCommand(ratesCmd)
	return root
}

func (s *runtimeState) newBridgeCommand() *cobra.Command {
	root := &cobra.Command{Use: "bridge", Short: "Bridge quote and analytics commands"}

	var quoteProviderArg, fromArg, toArg, assetArg, toAssetArg string
	var amountBase, amountDecimal string
	quoteCmd := &cobra.Command{
		Use:   "quote",
		Short: "Get bridge quote",
		RunE: func(cmd *cobra.Command, args []string) error {
			providerName := strings.ToLower(strings.TrimSpace(quoteProviderArg))
			if providerName == "" {
				providerName = "across"
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
				FromChain:       fromChain,
				ToChain:         toChain,
				FromAsset:       fromAsset,
				ToAsset:         toAsset,
				AmountBaseUnits: base,
				AmountDecimal:   decimal,
			}
			key := cacheKey(trimRootPath(cmd.CommandPath()), map[string]any{
				"provider":   providerName,
				"from":       fromChain.CAIP2,
				"to":         toChain.CAIP2,
				"from_asset": fromAsset.AssetID,
				"to_asset":   toAsset.AssetID,
				"amount":     base,
			})
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 15*time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				start := time.Now()
				data, err := provider.QuoteBridge(ctx, reqStruct)
				status := []model.ProviderStatus{{Name: provider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, status, nil, false, err
			})
		},
	}
	quoteCmd.Flags().StringVar(&quoteProviderArg, "provider", "across", "Bridge provider (across|lifi; no API key required)")
	quoteCmd.Flags().StringVar(&fromArg, "from", "", "Source chain")
	quoteCmd.Flags().StringVar(&toArg, "to", "", "Destination chain")
	quoteCmd.Flags().StringVar(&assetArg, "asset", "", "Asset (symbol/address/CAIP-19) on source chain")
	quoteCmd.Flags().StringVar(&toAssetArg, "to-asset", "", "Destination asset override (symbol/address/CAIP-19)")
	quoteCmd.Flags().StringVar(&amountBase, "amount", "", "Amount in base units")
	quoteCmd.Flags().StringVar(&amountDecimal, "amount-decimal", "", "Amount in decimal units")
	_ = quoteCmd.MarkFlagRequired("from")
	_ = quoteCmd.MarkFlagRequired("to")
	_ = quoteCmd.MarkFlagRequired("asset")

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

	root.AddCommand(quoteCmd)
	root.AddCommand(listCmd)
	root.AddCommand(detailsCmd)
	return root
}

func (s *runtimeState) newSwapCommand() *cobra.Command {
	root := &cobra.Command{Use: "swap", Short: "Swap quote commands"}
	var providerArg, chainArg, fromAssetArg, toAssetArg string
	var amountBase, amountDecimal string
	cmd := &cobra.Command{
		Use:   "quote",
		Short: "Get swap quote",
		RunE: func(cmd *cobra.Command, args []string) error {
			chain, err := id.ParseChain(chainArg)
			if err != nil {
				return err
			}
			providerName := strings.ToLower(strings.TrimSpace(providerArg))
			if providerName == "" {
				if chain.IsSolana() {
					providerName = "jupiter"
				} else {
					providerName = "1inch"
				}
			}
			provider, ok := s.swapProviders[providerName]
			if !ok {
				return clierr.New(clierr.CodeUnsupported, "unsupported swap provider")
			}
			fromAsset, err := id.ParseAsset(fromAssetArg, chain)
			if err != nil {
				return err
			}
			toAsset, err := id.ParseAsset(toAssetArg, chain)
			if err != nil {
				return err
			}
			decimals := fromAsset.Decimals
			if decimals <= 0 {
				decimals = 18
			}
			base, decimal, err := id.NormalizeAmount(amountBase, amountDecimal, decimals)
			if err != nil {
				return err
			}
			reqStruct := providers.SwapQuoteRequest{Chain: chain, FromAsset: fromAsset, ToAsset: toAsset, AmountBaseUnits: base, AmountDecimal: decimal}
			key := cacheKey(trimRootPath(cmd.CommandPath()), map[string]any{
				"provider": providerName,
				"chain":    chain.CAIP2,
				"from":     fromAsset.AssetID,
				"to":       toAsset.AssetID,
				"amount":   base,
			})
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 15*time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				start := time.Now()
				data, err := provider.QuoteSwap(ctx, reqStruct)
				status := []model.ProviderStatus{{Name: provider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
				return data, status, nil, false, err
			})
		},
	}
	cmd.Flags().StringVar(&providerArg, "provider", "", "Swap provider (defaults: 1inch for EVM, jupiter for Solana; options: 1inch|uniswap|jupiter)")
	cmd.Flags().StringVar(&chainArg, "chain", "", "Chain identifier")
	cmd.Flags().StringVar(&fromAssetArg, "from-asset", "", "Input asset")
	cmd.Flags().StringVar(&toAssetArg, "to-asset", "", "Output asset")
	cmd.Flags().StringVar(&amountBase, "amount", "", "Amount in base units")
	cmd.Flags().StringVar(&amountDecimal, "amount-decimal", "", "Amount in decimal units")
	_ = cmd.MarkFlagRequired("chain")
	_ = cmd.MarkFlagRequired("from-asset")
	_ = cmd.MarkFlagRequired("to-asset")
	root.AddCommand(cmd)
	return root
}

func (s *runtimeState) newYieldCommand() *cobra.Command {
	root := &cobra.Command{Use: "yield", Short: "Yield opportunity commands"}
	var chainArg, assetArg, providersArg, sortArg, maxRisk string
	var limit int
	var minTVL, minAPY float64
	var includeIncomplete bool
	cmd := &cobra.Command{
		Use:   "opportunities",
		Short: "Rank yield opportunities",
		RunE: func(cmd *cobra.Command, args []string) error {
			chain, asset, err := parseChainAsset(chainArg, assetArg)
			if err != nil {
				return err
			}
			req := providers.YieldRequest{
				Chain:             chain,
				Asset:             asset,
				Limit:             limit,
				MinTVLUSD:         minTVL,
				MinAPY:            minAPY,
				MaxRisk:           maxRisk,
				Providers:         splitCSV(providersArg),
				SortBy:            sortArg,
				IncludeIncomplete: includeIncomplete,
			}
			key := cacheKey(trimRootPath(cmd.CommandPath()), map[string]any{
				"chain":              req.Chain.CAIP2,
				"asset":              req.Asset.AssetID,
				"limit":              req.Limit,
				"min_tvl_usd":        req.MinTVLUSD,
				"min_apy":            req.MinAPY,
				"max_risk":           req.MaxRisk,
				"providers":          req.Providers,
				"sort":               req.SortBy,
				"include_incomplete": req.IncludeIncomplete,
			})
			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 60*time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				selectedProviders, err := s.selectYieldProviders(req.Providers)
				if err != nil {
					return nil, nil, nil, false, err
				}
				warnings := []string{}
				if !assetHasResolvedSymbol(req.Asset) {
					filteredProviders := make([]string, 0, len(selectedProviders))
					skippedDefiLlama := false
					for _, providerName := range selectedProviders {
						if providerName == "defillama" {
							skippedDefiLlama = true
							continue
						}
						filteredProviders = append(filteredProviders, providerName)
					}
					if skippedDefiLlama {
						warnings = append(warnings, "provider defillama skipped: unresolved asset symbol cannot be matched safely")
					}
					selectedProviders = filteredProviders
					if len(selectedProviders) == 0 {
						return nil, nil, warnings, false, clierr.New(clierr.CodeUsage, "selected providers require a resolvable asset symbol")
					}
				}
				statuses := make([]model.ProviderStatus, 0, len(selectedProviders))
				combined := make([]model.YieldOpportunity, 0)
				partial := false
				var firstErr error

				for _, providerName := range selectedProviders {
					provider := s.yieldProviders[providerName]
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

				if includeIncomplete {
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
				if includeIncomplete {
					warnings = append(warnings, fmt.Sprintf("returned %d combined opportunities across %d provider(s)", len(combined), len(selectedProviders)))
				}
				return combined, statuses, warnings, partial, nil
			})
		},
	}
	cmd.Flags().StringVar(&chainArg, "chain", "", "Chain identifier")
	cmd.Flags().StringVar(&assetArg, "asset", "", "Asset symbol/address/CAIP-19")
	cmd.Flags().IntVar(&limit, "limit", 20, "Maximum opportunities to return")
	cmd.Flags().Float64Var(&minTVL, "min-tvl-usd", 0, "Minimum TVL in USD")
	cmd.Flags().StringVar(&maxRisk, "max-risk", "high", "Maximum risk level (low|medium|high|unknown)")
	cmd.Flags().Float64Var(&minAPY, "min-apy", 0, "Minimum total APY percent")
	cmd.Flags().StringVar(&providersArg, "providers", "", "Filter by provider names (defillama,aave,morpho,kamino)")
	cmd.Flags().StringVar(&sortArg, "sort", "score", "Sort key (score|apy_total|tvl_usd|liquidity_usd)")
	cmd.Flags().BoolVar(&includeIncomplete, "include-incomplete", false, "Include opportunities missing APY/TVL")
	_ = cmd.MarkFlagRequired("chain")
	_ = cmd.MarkFlagRequired("asset")
	root.AddCommand(cmd)
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

func normalizeLendingProtocol(input string) string {
	switch strings.ToLower(strings.TrimSpace(input)) {
	case "aave", "aave-v2", "aave-v3":
		return "aave"
	case "morpho", "morpho-blue":
		return "morpho"
	case "kamino", "kamino-lend", "kamino-finance":
		return "kamino"
	case "spark":
		return "spark"
	default:
		return strings.ToLower(strings.TrimSpace(input))
	}
}

func (s *runtimeState) selectLendingProviders(protocol string) (providers.LendingProvider, providers.LendingProvider, error) {
	primary, ok := s.lendingProviders[protocol]
	if !ok {
		return nil, nil, clierr.New(clierr.CodeUnsupported, fmt.Sprintf("unsupported lending protocol: %s", protocol))
	}
	if s.defaultLendingProvider == nil || primary.Info().Name == s.defaultLendingProvider.Info().Name {
		return primary, nil, nil
	}
	return primary, s.defaultLendingProvider, nil
}

func shouldFallback(err error) bool {
	cErr, ok := clierr.As(err)
	if !ok {
		return false
	}
	return cErr.Code == clierr.CodeUnavailable || cErr.Code == clierr.CodeUnsupported || cErr.Code == clierr.CodeRateLimited
}

func (s *runtimeState) selectYieldProviders(filter []string) ([]string, error) {
	if len(filter) == 0 {
		keys := make([]string, 0, len(s.yieldProviders))
		for name := range s.yieldProviders {
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

func dedupeYieldByOpportunityID(items []model.YieldOpportunity) []model.YieldOpportunity {
	if len(items) <= 1 {
		return items
	}
	byID := make(map[string]model.YieldOpportunity, len(items))
	for _, item := range items {
		existing, ok := byID[item.OpportunityID]
		if !ok || item.Score > existing.Score {
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
		sortBy = "score"
	}
	sort.Slice(items, func(i, j int) bool {
		a, b := items[i], items[j]
		switch sortBy {
		case "apy_total":
			if a.APYTotal != b.APYTotal {
				return a.APYTotal > b.APYTotal
			}
		case "tvl_usd":
			if a.TVLUSD != b.TVLUSD {
				return a.TVLUSD > b.TVLUSD
			}
		case "liquidity_usd":
			if a.LiquidityUSD != b.LiquidityUSD {
				return a.LiquidityUSD > b.LiquidityUSD
			}
		default:
			if a.Score != b.Score {
				return a.Score > b.Score
			}
		}
		if a.APYTotal != b.APYTotal {
			return a.APYTotal > b.APYTotal
		}
		if a.TVLUSD != b.TVLUSD {
			return a.TVLUSD > b.TVLUSD
		}
		return strings.Compare(a.OpportunityID, b.OpportunityID) < 0
	})
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

func cacheKey(commandPath string, req any) string {
	buf, _ := json.Marshal(req)
	sum := sha256.Sum256(append([]byte(commandPath+"|"), buf...))
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
	switch normalizeCommandPath(commandPath) {
	case "", "version", "schema", "providers", "providers list":
		return false
	default:
		return true
	}
}

func normalizeCommandPath(commandPath string) string {
	return strings.Join(strings.Fields(strings.ToLower(strings.TrimSpace(commandPath))), " ")
}

func assetHasResolvedSymbol(asset id.Asset) bool {
	return strings.TrimSpace(asset.Symbol) != ""
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

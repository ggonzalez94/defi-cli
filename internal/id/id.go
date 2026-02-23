package id

import (
	"fmt"
	"regexp"
	"sort"
	"strconv"
	"strings"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
)

var (
	chainPattern   = regexp.MustCompile(`^eip155:[0-9]+$`)
	evmAddrPattern = regexp.MustCompile(`^0x[0-9a-fA-F]{40}$`)
)

type Chain struct {
	Name       string
	Slug       string
	CAIP2      string
	EVMChainID int64
}

type Asset struct {
	ChainID  string
	AssetID  string
	Address  string
	Symbol   string
	Decimals int
}

type Token struct {
	Symbol   string
	Address  string
	Decimals int
}

var chainBySlug = map[string]Chain{
	"ethereum":      {Name: "Ethereum", Slug: "ethereum", CAIP2: "eip155:1", EVMChainID: 1},
	"mainnet":       {Name: "Ethereum", Slug: "ethereum", CAIP2: "eip155:1", EVMChainID: 1},
	"optimism":      {Name: "Optimism", Slug: "optimism", CAIP2: "eip155:10", EVMChainID: 10},
	"op mainnet":    {Name: "Optimism", Slug: "optimism", CAIP2: "eip155:10", EVMChainID: 10},
	"op-mainnet":    {Name: "Optimism", Slug: "optimism", CAIP2: "eip155:10", EVMChainID: 10},
	"bsc":           {Name: "BSC", Slug: "bsc", CAIP2: "eip155:56", EVMChainID: 56},
	"gnosis":        {Name: "Gnosis", Slug: "gnosis", CAIP2: "eip155:100", EVMChainID: 100},
	"xdai":          {Name: "Gnosis", Slug: "gnosis", CAIP2: "eip155:100", EVMChainID: 100},
	"polygon":       {Name: "Polygon", Slug: "polygon", CAIP2: "eip155:137", EVMChainID: 137},
	"monad":         {Name: "Monad", Slug: "monad", CAIP2: "eip155:143", EVMChainID: 143},
	"sonic":         {Name: "Sonic", Slug: "sonic", CAIP2: "eip155:146", EVMChainID: 146},
	"fraxtal":       {Name: "Fraxtal", Slug: "fraxtal", CAIP2: "eip155:252", EVMChainID: 252},
	"zksync":        {Name: "zkSync Era", Slug: "zksync", CAIP2: "eip155:324", EVMChainID: 324},
	"zksync era":    {Name: "zkSync Era", Slug: "zksync", CAIP2: "eip155:324", EVMChainID: 324},
	"zksync-era":    {Name: "zkSync Era", Slug: "zksync", CAIP2: "eip155:324", EVMChainID: 324},
	"worldchain":    {Name: "World Chain", Slug: "world-chain", CAIP2: "eip155:480", EVMChainID: 480},
	"world chain":   {Name: "World Chain", Slug: "world-chain", CAIP2: "eip155:480", EVMChainID: 480},
	"world-chain":   {Name: "World Chain", Slug: "world-chain", CAIP2: "eip155:480", EVMChainID: 480},
	"hyperevm":      {Name: "HyperEVM", Slug: "hyperevm", CAIP2: "eip155:999", EVMChainID: 999},
	"hyper evm":     {Name: "HyperEVM", Slug: "hyperevm", CAIP2: "eip155:999", EVMChainID: 999},
	"hyper-evm":     {Name: "HyperEVM", Slug: "hyperevm", CAIP2: "eip155:999", EVMChainID: 999},
	"citrea":        {Name: "Citrea", Slug: "citrea", CAIP2: "eip155:4114", EVMChainID: 4114},
	"mantle":        {Name: "Mantle", Slug: "mantle", CAIP2: "eip155:5000", EVMChainID: 5000},
	"megaeth":       {Name: "MegaETH", Slug: "megaeth", CAIP2: "eip155:4326", EVMChainID: 4326},
	"mega eth":      {Name: "MegaETH", Slug: "megaeth", CAIP2: "eip155:4326", EVMChainID: 4326},
	"mega-eth":      {Name: "MegaETH", Slug: "megaeth", CAIP2: "eip155:4326", EVMChainID: 4326},
	"base":          {Name: "Base", Slug: "base", CAIP2: "eip155:8453", EVMChainID: 8453},
	"blast":         {Name: "Blast", Slug: "blast", CAIP2: "eip155:81457", EVMChainID: 81457},
	"berachain":     {Name: "Berachain", Slug: "berachain", CAIP2: "eip155:80094", EVMChainID: 80094},
	"arbitrum":      {Name: "Arbitrum", Slug: "arbitrum", CAIP2: "eip155:42161", EVMChainID: 42161},
	"avalanche":     {Name: "Avalanche", Slug: "avalanche", CAIP2: "eip155:43114", EVMChainID: 43114},
	"linea":         {Name: "Linea", Slug: "linea", CAIP2: "eip155:59144", EVMChainID: 59144},
	"ink":           {Name: "Ink", Slug: "ink", CAIP2: "eip155:57073", EVMChainID: 57073},
	"scroll":        {Name: "Scroll", Slug: "scroll", CAIP2: "eip155:534352", EVMChainID: 534352},
	"celo":          {Name: "Celo", Slug: "celo", CAIP2: "eip155:42220", EVMChainID: 42220},
	"taiko":         {Name: "Taiko", Slug: "taiko", CAIP2: "eip155:167000", EVMChainID: 167000},
	"taiko alethia": {Name: "Taiko", Slug: "taiko", CAIP2: "eip155:167000", EVMChainID: 167000},
	"taiko-alethia": {Name: "Taiko", Slug: "taiko", CAIP2: "eip155:167000", EVMChainID: 167000},
}

var chainByID = map[int64]Chain{
	1:      chainBySlug["ethereum"],
	10:     chainBySlug["optimism"],
	56:     chainBySlug["bsc"],
	100:    chainBySlug["gnosis"],
	137:    chainBySlug["polygon"],
	143:    chainBySlug["monad"],
	999:    chainBySlug["hyperevm"],
	4114:   chainBySlug["citrea"],
	146:    chainBySlug["sonic"],
	252:    chainBySlug["fraxtal"],
	324:    chainBySlug["zksync"],
	480:    chainBySlug["world-chain"],
	5000:   chainBySlug["mantle"],
	4326:   chainBySlug["megaeth"],
	8453:   chainBySlug["base"],
	42220:  chainBySlug["celo"],
	42161:  chainBySlug["arbitrum"],
	43114:  chainBySlug["avalanche"],
	57073:  chainBySlug["ink"],
	59144:  chainBySlug["linea"],
	80094:  chainBySlug["berachain"],
	81457:  chainBySlug["blast"],
	167000: chainBySlug["taiko"],
	534352: chainBySlug["scroll"],
}

var chainByCAIP2 = buildChainByCAIP2()

// Small bootstrap registry for deterministic asset parsing on Tier-1 chains.
var tokenRegistry = map[string][]Token{
	"eip155:1": {
		{Symbol: "AAVE", Address: "0x7fc66500c84a76ad7e9c93437bfc5ac33e2ddae9", Decimals: 18},
		{Symbol: "BNB", Address: "0xb8c77482e45f1f44de1745f52c74426c631bdd52", Decimals: 18},
		{Symbol: "CAKE", Address: "0x152649ea73beab28c5b49b26eb48f7ead6d4c898", Decimals: 18},
		{Symbol: "CBBTC", Address: "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf", Decimals: 8},
		{Symbol: "CRV", Address: "0xd533a949740bb3306d119cc777fa900ba034cd52", Decimals: 18},
		{Symbol: "CRVUSD", Address: "0xf939e0a03fb07f59a73314e73794be0e57ac1b4e", Decimals: 18},
		{Symbol: "DAI", Address: "0x6b175474e89094c44da98b954eedeac495271d0f", Decimals: 18},
		{Symbol: "ENA", Address: "0x57e114b691db790c35207b2e685d4a43181e6061", Decimals: 18},
		{Symbol: "ETHFI", Address: "0xfe0c30065b384f05761f15d0cc899d4f9f9cc0eb", Decimals: 18},
		{Symbol: "EURC", Address: "0x1abaea1f7c830bd89acc67ec4af516284b1bc33c", Decimals: 6},
		{Symbol: "FRAX", Address: "0x853d955acef822db058eb8505911ed77f175b99e", Decimals: 18},
		{Symbol: "GHO", Address: "0x40d16fc0246ad3160ccc09b8d0d3a2cd28ae6c2f", Decimals: 18},
		{Symbol: "LDO", Address: "0x5a98fcbea516cf06857215779fd812ca3bef1b32", Decimals: 18},
		{Symbol: "LINK", Address: "0x514910771af9ca656af840dff83e8264ecf986ca", Decimals: 18},
		{Symbol: "MORPHO", Address: "0x58d97b57bb95320f9a05dc918aef65434969c2b2", Decimals: 18},
		{Symbol: "PAXG", Address: "0x45804880de22913dafe09f4980848ece6ecbaf78", Decimals: 18},
		{Symbol: "PENDLE", Address: "0x808507121b80c02388fad14726482e061b8da827", Decimals: 18},
		{Symbol: "PEPE", Address: "0x6982508145454ce325ddbe47a25d4ec3d2311933", Decimals: 18},
		{Symbol: "SHIB", Address: "0x95ad61b0a150d79219dcf64e1e6cc01f0b64c4ce", Decimals: 18},
		{Symbol: "TAIKO", Address: "0x10dea67478c5f8c5e2d90e5e9b26dbe60c54d800", Decimals: 18},
		{Symbol: "TUSD", Address: "0x0000000000085d4780b73119b644ae5ecd22b376", Decimals: 18},
		{Symbol: "UNI", Address: "0x1f9840a85d5af5bf1d1762f925bdaddc4201f984", Decimals: 18},
		{Symbol: "USDC", Address: "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", Decimals: 6},
		{Symbol: "USDE", Address: "0x4c9edd5852cd905f086c759e8383e09bff1e68b3", Decimals: 18},
		{Symbol: "USDS", Address: "0xdc035d45d973e3ec169d2276ddab16f1e407384f", Decimals: 18},
		{Symbol: "USDT", Address: "0xdac17f958d2ee523a2206206994597c13d831ec7", Decimals: 6},
		{Symbol: "USD1", Address: "0x8d0d000ee44948fc98c9b98a4fa4921476f08b0d", Decimals: 18},
		{Symbol: "WBTC", Address: "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599", Decimals: 8},
		{Symbol: "WETH", Address: "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2", Decimals: 18},
		{Symbol: "WLFI", Address: "0xda5e1988097297dcdc1f90d4dfe7909e847cbef6", Decimals: 18},
		{Symbol: "XAUT", Address: "0x68749665ff8d2d112fa859aa293f07a622782f38", Decimals: 6},
		{Symbol: "ZRO", Address: "0x6985884c4392d348587b19cb9eaaf157f13271cd", Decimals: 18},
	},
	"eip155:10": {
		{Symbol: "AAVE", Address: "0x76fb31fb4af56892a25e32cfc43de717950c9278", Decimals: 18},
		{Symbol: "CRV", Address: "0x0994206dfe8de6ec6920ff4d779b0d950605fb53", Decimals: 18},
		{Symbol: "CRVUSD", Address: "0xc52d7f23a2e460248db6ee192cb23dd12bddcbf6", Decimals: 18},
		{Symbol: "DAI", Address: "0xda10009cbd5d07dd0cecc66161fc93d7c9000da1", Decimals: 18},
		{Symbol: "ENA", Address: "0x58538e6a46e07434d7e7375bc268d3cb839c0133", Decimals: 18},
		{Symbol: "FRAX", Address: "0x2e3d870790dc77a83dd1d18184acc7439a53f475", Decimals: 18},
		{Symbol: "LDO", Address: "0xfdb794692724153d1488ccdbe0c56c252596735f", Decimals: 18},
		{Symbol: "LINK", Address: "0x350a791bfc2c21f9ed5d10980dad2e2638ffa7f6", Decimals: 18},
		{Symbol: "OP", Address: "0x4200000000000000000000000000000000000042", Decimals: 18},
		{Symbol: "PENDLE", Address: "0xbc7b1ff1c6989f006a1185318ed4e7b5796e66e1", Decimals: 18},
		{Symbol: "TUSD", Address: "0xcb59a0a753fdb7491d5f3d794316f1ade197b21e", Decimals: 18},
		{Symbol: "UNI", Address: "0x6fd9d7ad17242c41f7131d257212c54a0e816691", Decimals: 18},
		{Symbol: "USDC", Address: "0x7f5c764cbc14f9669b88837ca1490cca17c31607", Decimals: 6},
		{Symbol: "USDE", Address: "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", Decimals: 18},
		{Symbol: "USDT", Address: "0x94b008aa00579c1307b0ef2c499ad98a8ce58e58", Decimals: 6},
		{Symbol: "USDT0", Address: "0x01bff41798a0bcf287b996046ca68b395dbc1071", Decimals: 6},
		{Symbol: "WBTC", Address: "0x68f180fcce6836688e9084f035309e29bf0a2095", Decimals: 8},
		{Symbol: "WETH", Address: "0x4200000000000000000000000000000000000006", Decimals: 18},
		{Symbol: "ZRO", Address: "0x6985884c4392d348587b19cb9eaaf157f13271cd", Decimals: 18},
	},
	"eip155:56": {
		{Symbol: "AAVE", Address: "0xfb6115445bff7b52feb98650c87f44907e58f802", Decimals: 18},
		{Symbol: "BTCB", Address: "0x7130d2a12b9bcbfae4f2634d864a1ee1ce3ead9c", Decimals: 18},
		{Symbol: "CAKE", Address: "0x0e09fabb73bd3ade0a17ecc321fd13a19e81ce82", Decimals: 18},
		{Symbol: "CRVUSD", Address: "0xe2fb3f127f5450dee44afe054385d74c392bdef4", Decimals: 18},
		{Symbol: "DAI", Address: "0x1af3f329e8be154074d8769d1ffa4ee058b1dbc3", Decimals: 18},
		{Symbol: "ENA", Address: "0x58538e6a46e07434d7e7375bc268d3cb839c0133", Decimals: 18},
		{Symbol: "FRAX", Address: "0x90c97f71e18723b0cf0dfa30ee176ab653e89f40", Decimals: 18},
		{Symbol: "LINK", Address: "0xf8a0bf9cf54bb92f17374d9e9a321e6a111a51bd", Decimals: 18},
		{Symbol: "PENDLE", Address: "0xb3ed0a426155b79b898849803e3b36552f7ed507", Decimals: 18},
		{Symbol: "PEPE", Address: "0x25d887ce7a35172c62febfd67a1856f20faebb00", Decimals: 18},
		{Symbol: "TUSD", Address: "0x40af3827f39d0eacbf4a168f8d4ee67c121d11c9", Decimals: 18},
		{Symbol: "UNI", Address: "0xbf5140a22578168fd562dccf235e5d43a02ce9b1", Decimals: 18},
		{Symbol: "USDC", Address: "0x8ac76a51cc950d9822d68b83fe1ad97b32cd580d", Decimals: 18},
		{Symbol: "USDE", Address: "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", Decimals: 18},
		{Symbol: "USDT", Address: "0x55d398326f99059ff775485246999027b3197955", Decimals: 18},
		{Symbol: "USD1", Address: "0x8d0d000ee44948fc98c9b98a4fa4921476f08b0d", Decimals: 18},
		{Symbol: "WBNB", Address: "0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c", Decimals: 18},
		{Symbol: "WBTC", Address: "0x0555e30da8f98308edb960aa94c0db47230d2b9c", Decimals: 8},
		{Symbol: "WETH", Address: "0x2170ed0880ac9a755fd29b2688956bd959f933f8", Decimals: 18},
		{Symbol: "ZRO", Address: "0x6985884c4392d348587b19cb9eaaf157f13271cd", Decimals: 18},
	},
	"eip155:100": {
		{Symbol: "AAVE", Address: "0xdf613af6b44a31299e48131e9347f034347e2f00", Decimals: 18},
		{Symbol: "CRV", Address: "0x712b3d230f3c1c19db860d80619288b1f0bdd0bd", Decimals: 18},
		{Symbol: "CRVUSD", Address: "0xabef652195f98a91e490f047a5006b71c85f058d", Decimals: 18},
		{Symbol: "FRAX", Address: "0xca5d82e40081f220d59f7ed9e2e1428deaf55355", Decimals: 18},
		{Symbol: "GHO", Address: "0xfc421ad3c883bf9e7c4f42de845c4e4405799e73", Decimals: 18},
		{Symbol: "LDO", Address: "0x96e334926454cd4b7b4efb8a8fcb650a738ad244", Decimals: 18},
		{Symbol: "LINK", Address: "0xe2e73a1c69ecf83f464efce6a5be353a37ca09b2", Decimals: 18},
		{Symbol: "TUSD", Address: "0xb714654e905edad1ca1940b7790a8239ece5a9ff", Decimals: 18},
		{Symbol: "UNI", Address: "0x4537e328bf7e4efa29d05caea260d7fe26af9d74", Decimals: 18},
		{Symbol: "USDC", Address: "0xddafbb505ad214d7b80b1f830fccc89b60fb7a83", Decimals: 6},
		{Symbol: "USDT", Address: "0x4ecaba5870353805a9f068101a40e0f32ed605c6", Decimals: 6},
		{Symbol: "WETH", Address: "0x6a023ccd1ff6f2045c3309768ead9e68f978f6e1", Decimals: 18},
	},
	"eip155:137": {
		{Symbol: "AAVE", Address: "0xd6df932a45c0f255f85145f286ea0b292b21c90b", Decimals: 18},
		{Symbol: "CRV", Address: "0x172370d5cd63279efa6d502dab29171933a610af", Decimals: 18},
		{Symbol: "CRVUSD", Address: "0xc4ce1d6f5d98d65ee25cf85e9f2e9dcfee6cb5d6", Decimals: 18},
		{Symbol: "DAI", Address: "0x8f3cf7ad23cd3cadbd9735aff958023239c6a063", Decimals: 18},
		{Symbol: "FRAX", Address: "0x45c32fa6df82ead1e2ef74d17b76547eddfaff89", Decimals: 18},
		{Symbol: "LDO", Address: "0xc3c7d422809852031b44ab29eec9f1eff2a58756", Decimals: 18},
		{Symbol: "LINK", Address: "0x53e0bca35ec356bd5dddfebbd1fc0fd03fabad39", Decimals: 18},
		{Symbol: "TUSD", Address: "0x2e1ad108ff1d8c782fcbbb89aad783ac49586756", Decimals: 18},
		{Symbol: "UNI", Address: "0xb33eaad8d922b1083446dc23f610c2567fb5180f", Decimals: 18},
		{Symbol: "USDC", Address: "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359", Decimals: 6},
		{Symbol: "USDT", Address: "0xc2132d05d31c914a87c6611c10748aeb04b58e8f", Decimals: 6},
		{Symbol: "WETH", Address: "0x7ceb23fd6bc0add59e62ac25578270cff1b9f619", Decimals: 18},
		{Symbol: "ZRO", Address: "0x6985884c4392d348587b19cb9eaaf157f13271cd", Decimals: 18},
	},
	"eip155:146": {
		{Symbol: "CRVUSD", Address: "0x7fff4c4a827c84e32c5e175052834111b2ccd270", Decimals: 18},
		{Symbol: "LINK", Address: "0x71052bae71c25c78e37fd12e5ff1101a71d9018f", Decimals: 18},
		{Symbol: "PENDLE", Address: "0xf1ef7d2d4c0c881cd634481e0586ed5d2871a74b", Decimals: 18},
		{Symbol: "USDC", Address: "0x29219dd400f2bf60e5a23d13be72b486d4038894", Decimals: 6},
		{Symbol: "USDT", Address: "0x6047828dc181963ba44974801ff68e538da5eaf9", Decimals: 6},
		{Symbol: "WETH", Address: "0x50c42deacd8fc9773493ed674b675be577f2634b", Decimals: 18},
	},
	"eip155:252": {
		{Symbol: "CRV", Address: "0x331b9182088e2a7d6d3fe4742aba1fb231aecc56", Decimals: 18},
		{Symbol: "CRVUSD", Address: "0xb102f7efa0d5de071a8d37b3548e1c7cb148caf3", Decimals: 18},
		{Symbol: "ENA", Address: "0x58538e6a46e07434d7e7375bc268d3cb839c0133", Decimals: 18},
		{Symbol: "FRAX", Address: "0xfc00000000000000000000000000000000000001", Decimals: 18},
		{Symbol: "LINK", Address: "0xd6a6ba37faac229b9665e86739ca501401f5a940", Decimals: 18},
		{Symbol: "USDC", Address: "0xdcc0f2d8f90fde85b10ac1c8ab57dc0ae946a543", Decimals: 6},
		{Symbol: "USDE", Address: "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", Decimals: 18},
		{Symbol: "USDT", Address: "0x4d15ea9c2573addaed814e48c148b5262694646a", Decimals: 6},
	},
	"eip155:324": {
		{Symbol: "CAKE", Address: "0x3a287a06c66f9e95a56327185ca2bdf5f031cecd", Decimals: 18},
		{Symbol: "CRVUSD", Address: "0x43cd37cc4b9ec54833c8ac362dd55e58bfd62b86", Decimals: 18},
		{Symbol: "ENA", Address: "0x686b311f82b407f0be842652a98e5619f64cc25f", Decimals: 18},
		{Symbol: "FRAX", Address: "0xb4c1544cb4163f4c2eca1ae9ce999f63892d912a", Decimals: 18},
		{Symbol: "LINK", Address: "0x52869bae3e091e36b0915941577f2d47d8d8b534", Decimals: 18},
		{Symbol: "USDC", Address: "0x1d17cbcf0d6d143135ae902365d2e5e2a16538d4", Decimals: 6},
		{Symbol: "USDE", Address: "0x39fe7a0dacce31bd90418e3e659fb0b5f0b3db0d", Decimals: 18},
		{Symbol: "USDT", Address: "0x493257fd37edb34451f62edf8d2a0c418852ba4c", Decimals: 6},
		{Symbol: "WETH", Address: "0x5aea5775959fbc2557cc8789bc1bf90a239d9a91", Decimals: 18},
	},
	"eip155:480": {
		{Symbol: "EURC", Address: "0x1c60ba0a0ed1019e8eb035e6daf4155a5ce2380b", Decimals: 6},
		{Symbol: "LINK", Address: "0x915b648e994d5f31059b38223b9fbe98ae185473", Decimals: 18},
		{Symbol: "USDC", Address: "0x79a02482a880bce3f13e09da970dc34db4cd24d1", Decimals: 6},
	},
	"eip155:5000": {
		{Symbol: "ENA", Address: "0x58538e6a46e07434d7e7375bc268d3cb839c0133", Decimals: 18},
		{Symbol: "GHO", Address: "0xfc421ad3c883bf9e7c4f42de845c4e4405799e73", Decimals: 18},
		{Symbol: "LINK", Address: "0xfe36cf0b43aae49fbc5cfc5c0af22a623114e043", Decimals: 18},
		{Symbol: "USDC", Address: "0x09bc4e0d864854c6afb6eb9a9cdf58ac190d0df9", Decimals: 6},
		{Symbol: "USDE", Address: "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", Decimals: 18},
		{Symbol: "USDT", Address: "0x201eba5cc46d216ce6dc03f6a759e8e766e956ae", Decimals: 6},
		{Symbol: "WETH", Address: "0xdeaddeaddeaddeaddeaddeaddeaddeaddead1111", Decimals: 18},
	},
	"eip155:8453": {
		{Symbol: "AAVE", Address: "0x63706e401c06ac8513145b7687a14804d17f814b", Decimals: 18},
		{Symbol: "CAKE", Address: "0x3055913c90fcc1a6ce9a358911721eeb942013a1", Decimals: 18},
		{Symbol: "CBBTC", Address: "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf", Decimals: 8},
		{Symbol: "CRV", Address: "0x8ee73c484a26e0a5df2ee2a4960b789967dd0415", Decimals: 18},
		{Symbol: "CRVUSD", Address: "0x417ac0e078398c154edfadd9ef675d30be60af93", Decimals: 18},
		{Symbol: "DAI", Address: "0x50c5725949a6f0c72e6c4a641f24049a917db0cb", Decimals: 18},
		{Symbol: "ENA", Address: "0x58538e6a46e07434d7e7375bc268d3cb839c0133", Decimals: 18},
		{Symbol: "ETHFI", Address: "0x6c240dda6b5c336df09a4d011139beaaa1ea2aa2", Decimals: 18},
		{Symbol: "EURC", Address: "0x60a3e35cc302bfa44cb288bc5a4f316fdb1adb42", Decimals: 6},
		{Symbol: "FRAX", Address: "0x909dbde1ebe906af95660033e478d59efe831fed", Decimals: 18},
		{Symbol: "GHO", Address: "0x6bb7a212910682dcfdbd5bcbb3e28fb4e8da10ee", Decimals: 18},
		{Symbol: "LINK", Address: "0x88fb150bdc53a65fe94dea0c9ba0a6daf8c6e196", Decimals: 18},
		{Symbol: "MORPHO", Address: "0xbaa5cc21fd487b8fcc2f632f3f4e8d37262a0842", Decimals: 18},
		{Symbol: "PENDLE", Address: "0xa99f6e6785da0f5d6fb42495fe424bce029eeb3e", Decimals: 18},
		{Symbol: "SNX", Address: "0x22e6966b799c4d5b13be962e1d117b56327fda66", Decimals: 18},
		{Symbol: "UNI", Address: "0xc3de830ea07524a0761646a6a4e4be0e114a3c83", Decimals: 18},
		{Symbol: "USDC", Address: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913", Decimals: 6},
		{Symbol: "USDE", Address: "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", Decimals: 18},
		{Symbol: "USDS", Address: "0x820c137fa70c8691f0e44dc420a5e53c168921dc", Decimals: 18},
		{Symbol: "USDT", Address: "0xfde4c96c8593536e31f229ea8f37b2ada2699bb2", Decimals: 6},
		{Symbol: "WBTC", Address: "0x1cea84203673764244e05693e42e6ace62be9ba5", Decimals: 8},
		{Symbol: "WETH", Address: "0x4200000000000000000000000000000000000006", Decimals: 18},
		{Symbol: "ZRO", Address: "0x6985884c4392d348587b19cb9eaaf157f13271cd", Decimals: 18},
	},
	"eip155:42161": {
		{Symbol: "AAVE", Address: "0xba5ddd1f9d7f570dc94a51479a000e3bce967196", Decimals: 18},
		{Symbol: "ARB", Address: "0x912ce59144191c1204e64559fe8253a0e49e6548", Decimals: 18},
		{Symbol: "CAKE", Address: "0x1b896893dfc86bb67cf57767298b9073d2c1ba2c", Decimals: 18},
		{Symbol: "CBBTC", Address: "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf", Decimals: 8},
		{Symbol: "CRV", Address: "0x11cdb42b0eb46d95f990bedd4695a6e3fa034978", Decimals: 18},
		{Symbol: "CRVUSD", Address: "0x498bf2b1e120fed3ad3d42ea2165e9b73f99c1e5", Decimals: 18},
		{Symbol: "DAI", Address: "0xda10009cbd5d07dd0cecc66161fc93d7c9000da1", Decimals: 18},
		{Symbol: "ENA", Address: "0x58538e6a46e07434d7e7375bc268d3cb839c0133", Decimals: 18},
		{Symbol: "ETHFI", Address: "0x7189fb5b6504bbff6a852b13b7b82a3c118fdc27", Decimals: 18},
		{Symbol: "FRAX", Address: "0x17fc002b466eec40dae837fc4be5c67993ddbd6f", Decimals: 18},
		{Symbol: "GHO", Address: "0x7dff72693f6a4149b17e7c6314655f6a9f7c8b33", Decimals: 18},
		{Symbol: "LDO", Address: "0x13ad51ed4f1b7e9dc168d8a00cb3f4ddd85efa60", Decimals: 18},
		{Symbol: "LINK", Address: "0xf97f4df75117a78c1a5a0dbb814af92458539fb4", Decimals: 18},
		{Symbol: "MORPHO", Address: "0x40bd670a58238e6e230c430bbb5ce6ec0d40df48", Decimals: 18},
		{Symbol: "PENDLE", Address: "0x0c880f6761f1af8d9aa9c466984b80dab9a8c9e8", Decimals: 18},
		{Symbol: "PEPE", Address: "0x25d887ce7a35172c62febfd67a1856f20faebb00", Decimals: 18},
		{Symbol: "PYUSD", Address: "0x46850ad61c2b7d64d08c9c754f45254596696984", Decimals: 6},
		{Symbol: "TUSD", Address: "0x4d15a3a2286d883af0aa1b3f21367843fac63e07", Decimals: 18},
		{Symbol: "UNI", Address: "0xfa7f8980b0f1e64a2062791cc3b0871572f1f7f0", Decimals: 18},
		{Symbol: "USDC", Address: "0xaf88d065e77c8cc2239327c5edb3a432268e5831", Decimals: 6},
		{Symbol: "USDE", Address: "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", Decimals: 18},
		{Symbol: "USDS", Address: "0x6491c05a82219b8d1479057361ff1654749b876b", Decimals: 18},
		{Symbol: "USDT", Address: "0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9", Decimals: 6},
		{Symbol: "WBTC", Address: "0x2f2a2543b76a4166549f7aab2e75bef0aefc5b0f", Decimals: 8},
		{Symbol: "WETH", Address: "0x82af49447d8a07e3bd95bd0d56f35241523fbab1", Decimals: 18},
		{Symbol: "ZRO", Address: "0x6985884c4392d348587b19cb9eaaf157f13271cd", Decimals: 18},
	},
	"eip155:4326": {
		{Symbol: "MEGA", Address: "0x28B7E77f82B25B95953825F1E3eA0E36c1c29861", Decimals: 18},
		{Symbol: "USDT", Address: "0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb", Decimals: 6},
		{Symbol: "WETH", Address: "0x4200000000000000000000000000000000000006", Decimals: 18},
	},
	"eip155:42220": {
		{Symbol: "LINK", Address: "0xd07294e6e917e07dfdcee882dd1e2565085c2ae0", Decimals: 18},
		{Symbol: "USDC", Address: "0xceba9300f2b948710d2653dd7b07f33a8b32118c", Decimals: 6},
		{Symbol: "USDT", Address: "0x48065fbbe25f71c9282ddf5e1cd6d6a887483d5e", Decimals: 6},
		{Symbol: "WETH", Address: "0xd221812de1bd094f35587ee8e174b07b6167d9af", Decimals: 18},
	},
	"eip155:43114": {
		{Symbol: "AAVE", Address: "0x63a72806098bd3d9520cc43356dd78afe5d386d9", Decimals: 18},
		{Symbol: "DAI", Address: "0xd586e7f844cea2f87f50152665bcbc2c279d8d70", Decimals: 18},
		{Symbol: "ENA", Address: "0x58538e6a46e07434d7e7375bc268d3cb839c0133", Decimals: 18},
		{Symbol: "EURC", Address: "0xc891eb4cbdeff6e073e859e987815ed1505c2acd", Decimals: 6},
		{Symbol: "FRAX", Address: "0xd24c2ad096400b6fbcd2ad8b24e7acbc21a1da64", Decimals: 18},
		{Symbol: "GHO", Address: "0xfc421ad3c883bf9e7c4f42de845c4e4405799e73", Decimals: 18},
		{Symbol: "LINK", Address: "0x5947bb275c521040051d82396192181b413227a3", Decimals: 18},
		{Symbol: "PENDLE", Address: "0xfb98b335551a418cd0737375a2ea0ded62ea213b", Decimals: 18},
		{Symbol: "PEPE", Address: "0xa659d083b677d6bffe1cb704e1473b896727be6d", Decimals: 18},
		{Symbol: "TUSD", Address: "0x1c20e891bab6b1727d14da358fae2984ed9b59eb", Decimals: 18},
		{Symbol: "UNI", Address: "0x8ebaf22b6f053dffeaf46f4dd9efa95d89ba8580", Decimals: 18},
		{Symbol: "USDC", Address: "0xb97ef9ef8734c71904d8002f8b6bc66dd9c48a6e", Decimals: 6},
		{Symbol: "USDE", Address: "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", Decimals: 18},
		{Symbol: "USDT", Address: "0x9702230a8ea53601f5cd2dc00fdbc13d4df4a8c7", Decimals: 6},
		{Symbol: "WAVAX", Address: "0xb31f66aa3c1e785363f0875a1b74e27b85fd66c7", Decimals: 18},
		{Symbol: "WBTC", Address: "0x0555e30da8f98308edb960aa94c0db47230d2b9c", Decimals: 8},
		{Symbol: "WETH", Address: "0x49d5c2bdffac6ce2bfdb6640f4f80f226bc10bab", Decimals: 18},
		{Symbol: "ZRO", Address: "0x6985884c4392d348587b19cb9eaaf157f13271cd", Decimals: 18},
	},
	"eip155:57073": {
		{Symbol: "GHO", Address: "0xfc421ad3c883bf9e7c4f42de845c4e4405799e73", Decimals: 18},
		{Symbol: "LINK", Address: "0x71052bae71c25c78e37fd12e5ff1101a71d9018f", Decimals: 18},
		{Symbol: "USDC", Address: "0x2d270e6886d130d724215a266106e6832161eaed", Decimals: 6},
		{Symbol: "USDE", Address: "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", Decimals: 18},
		{Symbol: "WETH", Address: "0x4200000000000000000000000000000000000006", Decimals: 18},
	},
	"eip155:59144": {
		{Symbol: "CAKE", Address: "0x0d1e753a25ebda689453309112904807625befbe", Decimals: 18},
		{Symbol: "ENA", Address: "0x58538e6a46e07434d7e7375bc268d3cb839c0133", Decimals: 18},
		{Symbol: "LINK", Address: "0xa18152629128738a5c081eb226335fed4b9c95e9", Decimals: 18},
		{Symbol: "USDC", Address: "0x176211869ca2b568f2a7d4ee941e073a821ee1ff", Decimals: 6},
		{Symbol: "USDE", Address: "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", Decimals: 18},
		{Symbol: "USDT", Address: "0xa219439258ca9da29e9cc4ce5596924745e12b93", Decimals: 6},
		{Symbol: "WETH", Address: "0xe5d7c2a44ffddf6b295a15c148167daaaf5cf34f", Decimals: 18},
	},
	"eip155:80094": {
		{Symbol: "LINK", Address: "0x71052bae71c25c78e37fd12e5ff1101a71d9018f", Decimals: 18},
		{Symbol: "PENDLE", Address: "0xff9c599d51c407a45d631c6e89cb047efb88aef6", Decimals: 18},
		{Symbol: "USDE", Address: "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", Decimals: 18},
	},
	"eip155:81457": {
		{Symbol: "ENA", Address: "0x58538e6a46e07434d7e7375bc268d3cb839c0133", Decimals: 18},
		{Symbol: "FRAX", Address: "0x909dbde1ebe906af95660033e478d59efe831fed", Decimals: 18},
		{Symbol: "LINK", Address: "0x93202ec683288a9ea75bb829c6bacfb2bfea9013", Decimals: 18},
		{Symbol: "USDE", Address: "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", Decimals: 18},
	},
	"eip155:167000": {
		{Symbol: "CRVUSD", Address: "0xc8f4518ed4bab9a972808a493107926ce8237068", Decimals: 18},
		{Symbol: "LINK", Address: "0x917a3964c37993e99a47c779beb5db1e9d13804d", Decimals: 18},
		{Symbol: "TAIKO", Address: "0xa9d23408b9ba935c230493c40c73824df71a0975", Decimals: 18},
		{Symbol: "USDC", Address: "0x07d83526730c7438048d55a4fc0b850e2aab6f0b", Decimals: 6},
		{Symbol: "USDT", Address: "0x2def195713cf4a606b49d07e520e22c17899a736", Decimals: 6},
		{Symbol: "WETH", Address: "0xa51894664a773981c6c112c43ce576f315d5b1b6", Decimals: 18},
	},
	"eip155:534352": {
		{Symbol: "CAKE", Address: "0x1b896893dfc86bb67cf57767298b9073d2c1ba2c", Decimals: 18},
		{Symbol: "ENA", Address: "0x58538e6a46e07434d7e7375bc268d3cb839c0133", Decimals: 18},
		{Symbol: "ETHFI", Address: "0x056a5fa5da84ceb7f93d36e545c5905607d8bd81", Decimals: 18},
		{Symbol: "LINK", Address: "0x548c6944cba02b9d1c0570102c89de64d258d3ac", Decimals: 18},
		{Symbol: "USDC", Address: "0x06efdbff2a14a7c8e15944d1f4a48f9f95f663a4", Decimals: 6},
		{Symbol: "USDE", Address: "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", Decimals: 18},
		{Symbol: "USDT", Address: "0xf55bec9cafdbe8730f096aa55dad6d22d44099df", Decimals: 6},
		{Symbol: "WETH", Address: "0x5300000000000000000000000000000000000004", Decimals: 18},
	},
	// HyperEVM mainnet (chain ID 999)
	"eip155:999": {
		{Symbol: "HYPE", Address: "0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE", Decimals: 18},
		{Symbol: "WHYPE", Address: "0x5555555555555555555555555555555555555555", Decimals: 18},
		{Symbol: "USDC", Address: "0xb88339cb7199b77e23db6e890353e22632ba630f", Decimals: 6},
	},
	// Monad mainnet (chain ID 143)
	"eip155:143": {
		{Symbol: "MON", Address: "0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE", Decimals: 18},
		{Symbol: "WMON", Address: "0x3bd359C1119dA7Da1D913D1C4D2B7c461115433A", Decimals: 18},
		{Symbol: "USDC", Address: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603", Decimals: 6},
	},
	// Citrea mainnet (chain ID 4114)
	"eip155:4114": {
		{Symbol: "CBTC", Address: "0x0000000000000000000000000000000000000000", Decimals: 18},
		{Symbol: "WCBTC", Address: "0x3100000000000000000000000000000000000006", Decimals: 18},
		{Symbol: "USDC", Address: "0xE045e6c36cF77FAA2CfB54466D71A3aEF7bBE839", Decimals: 6},
	},
}

func ParseChain(input string) (Chain, error) {
	norm := strings.TrimSpace(strings.ToLower(input))
	if norm == "" {
		return Chain{}, clierr.New(clierr.CodeUsage, "chain is required")
	}

	if chain, ok := chainBySlug[norm]; ok {
		return chain, nil
	}
	if chain, ok := chainByCAIP2[norm]; ok {
		return chain, nil
	}

	if chainPattern.MatchString(norm) {
		parts := strings.Split(norm, ":")
		id, _ := strconv.ParseInt(parts[1], 10, 64)
		if known, ok := chainByID[id]; ok {
			return known, nil
		}
		return Chain{Name: fmt.Sprintf("EVM-%d", id), Slug: fmt.Sprintf("evm-%d", id), CAIP2: norm, EVMChainID: id}, nil
	}

	if id, err := strconv.ParseInt(norm, 10, 64); err == nil {
		if chain, ok := chainByID[id]; ok {
			return chain, nil
		}
		return Chain{Name: fmt.Sprintf("EVM-%d", id), Slug: fmt.Sprintf("evm-%d", id), CAIP2: fmt.Sprintf("eip155:%d", id), EVMChainID: id}, nil
	}

	return Chain{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("unsupported chain input: %s", input))
}

func ParseAsset(input string, chain Chain) (Asset, error) {
	norm := strings.TrimSpace(input)
	if norm == "" {
		return Asset{}, clierr.New(clierr.CodeUsage, "asset is required")
	}
	if parts := strings.SplitN(norm, "/", 2); len(parts) == 2 && strings.Contains(parts[1], ":") {
		if !strings.EqualFold(parts[0], chain.CAIP2) {
			return Asset{}, clierr.New(clierr.CodeUsage, "asset chain does not match --chain")
		}
		assetParts := strings.SplitN(parts[1], ":", 2)
		if len(assetParts) != 2 {
			return Asset{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("invalid CAIP-19 asset format: %s", input))
		}
		ns := strings.ToLower(strings.TrimSpace(assetParts[0]))
		if ns != "erc20" {
			return Asset{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("unsupported asset namespace %s for chain %s", ns, chain.CAIP2))
		}
		addrRaw := strings.TrimSpace(assetParts[1])
		if !evmAddrPattern.MatchString(addrRaw) {
			return Asset{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("invalid token address for chain %s", chain.CAIP2))
		}
		addr := canonicalizeAddress(chain.CAIP2, addrRaw)
		token, _ := findTokenByAddress(chain.CAIP2, addr)
		return Asset{
			ChainID:  chain.CAIP2,
			AssetID:  fmt.Sprintf("%s/erc20:%s", chain.CAIP2, addr),
			Address:  addr,
			Symbol:   token.Symbol,
			Decimals: token.Decimals,
		}, nil
	}

	if evmAddrPattern.MatchString(norm) {
		addr := canonicalizeAddress(chain.CAIP2, norm)
		token, _ := findTokenByAddress(chain.CAIP2, addr)
		return Asset{
			ChainID:  chain.CAIP2,
			AssetID:  fmt.Sprintf("%s/erc20:%s", chain.CAIP2, addr),
			Address:  addr,
			Symbol:   token.Symbol,
			Decimals: token.Decimals,
		}, nil
	}

	matches := findTokensBySymbol(chain.CAIP2, norm)
	if len(matches) == 0 {
		return Asset{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("symbol %s not found in registry for chain %s", input, chain.CAIP2))
	}
	if len(matches) > 1 {
		addresses := make([]string, 0, len(matches))
		for _, m := range matches {
			addresses = append(addresses, m.Address)
		}
		sort.Strings(addresses)
		return Asset{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("symbol %s is ambiguous on chain %s, use address or CAIP-19 (%s)", input, chain.CAIP2, strings.Join(addresses, ", ")))
	}
	t := matches[0]
	addr := canonicalizeAddress(chain.CAIP2, t.Address)
	return Asset{
		ChainID:  chain.CAIP2,
		AssetID:  fmt.Sprintf("%s/erc20:%s", chain.CAIP2, addr),
		Address:  addr,
		Symbol:   strings.ToUpper(t.Symbol),
		Decimals: t.Decimals,
	}, nil
}

func buildChainByCAIP2() map[string]Chain {
	m := make(map[string]Chain, len(chainBySlug))
	for _, chain := range chainBySlug {
		m[strings.ToLower(chain.CAIP2)] = chain
	}
	return m
}

func canonicalizeAddress(chainID, address string) string {
	addr := strings.TrimSpace(address)
	if strings.HasPrefix(strings.ToLower(chainID), "eip155:") {
		return strings.ToLower(addr)
	}
	return addr
}

func findTokenByAddress(chainID, address string) (Token, bool) {
	target := canonicalizeAddress(chainID, address)
	for _, t := range tokenRegistry[chainID] {
		candidate := canonicalizeAddress(chainID, t.Address)
		if candidate == target {
			return Token{Symbol: strings.ToUpper(t.Symbol), Address: candidate, Decimals: t.Decimals}, true
		}
	}
	return Token{}, false
}

func findTokensBySymbol(chainID, symbol string) []Token {
	matches := []Token{}
	for _, t := range tokenRegistry[chainID] {
		if strings.EqualFold(t.Symbol, symbol) {
			matches = append(matches, Token{Symbol: strings.ToUpper(t.Symbol), Address: canonicalizeAddress(chainID, t.Address), Decimals: t.Decimals})
		}
	}
	return matches
}

func KnownToken(chainID, symbol string) (Token, bool) {
	matches := findTokensBySymbol(chainID, symbol)
	if len(matches) != 1 {
		return Token{}, false
	}
	return matches[0], true
}

func LookupByAddress(chainID, address string) (Token, bool) {
	return findTokenByAddress(chainID, canonicalizeAddress(chainID, address))
}

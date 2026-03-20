package registry

import (
	"strings"

	"github.com/ethereum/go-ethereum/common"
)

// Canonical bridge execution targets are sourced from provider deployment artifacts.
// These checks are enforced at submit time so --unsafe-provider-tx remains the
// single explicit escape hatch for provider-generated payloads.
var bridgeExecutionTargets = map[string]map[int64]map[string]struct{}{
	"lifi": {
		1:      addressSet("0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"),
		10:     addressSet("0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"),
		56:     addressSet("0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"),
		100:    addressSet("0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"),
		137:    addressSet("0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"),
		143:    addressSet("0x026F252016A7C47CDEf1F05a3Fc9E20C92a49C37"),
		146:    addressSet("0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"),
		252:    addressSet("0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"),
		324:    addressSet("0x341e94069f53234fE6DabeF707aD424830525715"),
		480:    addressSet("0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"),
		999:    addressSet("0x0a0758d937d1059c356D4714e57F5df0239bce1A"),
		5000:   addressSet("0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"),
		4326:   addressSet("0x026F252016A7C47CDEf1F05a3Fc9E20C92a49C37"),
		8453:   addressSet("0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"),
		42161:  addressSet("0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"),
		42220:  addressSet("0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"),
		43114:  addressSet("0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"),
		57073:  addressSet("0x864b314D4C5a0399368609581d3E8933a63b9232"),
		59144:  addressSet("0xDE1E598b81620773454588B85D6b5D4eEC32573e"),
		80094:  addressSet("0xf909c4Ae16622898b885B89d7F839E0244851c66"),
		81457:  addressSet("0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"),
		167000: addressSet("0x3A9A5dBa8FE1C4Da98187cE4755701BCA182f63b"),
		534352: addressSet("0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"),
	},
	"across": {
		1: addressSet(
			"0x5c7BCd6E7De5423a257D81B442095A1a6ced35C5",
			"0x767e4c20F521a829dE4Ffc40C25176676878147f",
			"0x10D8b8DaA26d307489803e10477De69C0492B610",
			"0x5616194d65638086a3191B1fEF436f503ff329eC",
			"0x89004EA51Bac007FEc55976967135b2Aa6e838d4",
			"0x4607BceaF7b22cb0c46882FFc9fAB3c6efe66e5a",
		),
		10: addressSet(
			"0x3E7448657409278C9d6E192b92F2b69B234FCc42",
			"0x6f26Bf09B1C792e3228e5467807a900A503c0281",
			"0x767e4c20F521a829dE4Ffc40C25176676878147f",
			"0x10D8b8DaA26d307489803e10477De69C0492B610",
			"0x986E476F93a423d7a4CD0baF362c5E0903268142",
			"0x6f4A733c7889f038D77D4f540182Dda17423CcbF",
		),
		56: addressSet(
			"0x4e8E101924eDE233C13e2D8622DC8aED2872d505",
			"0x89415a82d909a7238d69094C3Dd1dCC1aCbDa85C",
			"0x10D8b8DaA26d307489803e10477De69C0492B610",
		),
		137: addressSet(
			"0xaBa0F11D55C5dDC52cD0Cb2cd052B621d45159d5",
			"0xF9735e425A36d22636EF4cb75c7a6c63378290CA",
			"0x9295ee1d8C5b022Be115A2AD3c30C72E34e7F096",
			"0x767e4c20F521a829dE4Ffc40C25176676878147f",
			"0x10D8b8DaA26d307489803e10477De69C0492B610",
			"0x473dEBE3dB7338E03E3c8Dc8e980bb1DACb25bc5",
			"0xC6A21E6A57777F2183312c19e614DD6054b1A54F",
			"0x9220Fa27ae680E4e8D9733932128FA73362E0393",
			"0xC2dCB88873E00c9d401De2CBBa4C6A28f8A6e2c2",
		),
		143: addressSet(
			"0xd2ecb3afe598b746F8123CaE365a598DA831A449",
			"0xe9b0666DFfC176Df6686726CB9aaC78fD83D20d7",
			"0x10D8b8DaA26d307489803e10477De69C0492B610",
			"0xCbf361EE59Cc74b9d6e7Af947fe4136828faf2C5",
			"0xa3dE5F042EFD4C732498883100A2d319BbB3c1A1",
		),
		324: addressSet(
			"0xE0B015E54d54fc84a6cB9B666099c46adE9335FF",
			"0x672b9ba0CE73b69b5F940362F0ee36AAA3F02986",
			"0x5a148a9260c1f670429361c34d40b477280F01a9",
		),
		480: addressSet(
			"0x09aea4b2242abC8bb4BB78D537A67a245A7bEC64",
			"0x89415a82d909a7238d69094C3Dd1dCC1aCbDa85C",
			"0x10D8b8DaA26d307489803e10477De69C0492B610",
			"0x1c8243198570658f818FC56538f2c837C2a32958",
		),
		999: addressSet(
			"0x35E63eA3eb0fb7A3bc543C71FB66412e1F6B0E04",
			"0xF1BF00D947267Da5cC63f8c8A60568c59FA31bCb",
			"0x10D8b8DaA26d307489803e10477De69C0492B610",
			"0x1c709Fd0Db6A6B877Ddb19ae3D485B7b4ADD879f",
		),
		4326: addressSet(
			"0x3Db06DA8F0a24A525f314eeC954fC5c6a973d40E",
			"0xf0aBCe137a493185c5E768F275E7E931109f8981",
			"0x10D8b8DaA26d307489803e10477De69C0492B610",
			"0x5BE9F2a2f00475406f09e5bE82c06eFf206721d9",
		),
		8453: addressSet(
			"0x7CFaBF2eA327009B39f40078011B0Fb714b65926",
			"0x09aea4b2242abC8bb4BB78D537A67a245A7bEC64",
			"0x767e4c20F521a829dE4Ffc40C25176676878147f",
			"0x10D8b8DaA26d307489803e10477De69C0492B610",
			"0xA7A8d1efC1EE3E69999D370380949092251a5c20",
			"0xbcfbCE9D92A516e3e7b0762AE218B4194adE34b4",
		),
		42161: addressSet(
			"0xC456398D5eE3B93828252e48beDEDbc39e03368E",
			"0xe35e9842fceaCA96570B734083f4a58e8F7C5f2A",
			"0x767e4c20F521a829dE4Ffc40C25176676878147f",
			"0x10D8b8DaA26d307489803e10477De69C0492B610",
			"0xce1FFE01eBB4f8521C12e74363A396ee3d337E1B",
			"0x2ac5Ee3796E027dA274fbDe84c82173a65868940",
			"0xF633b72A4C2Fb73b77A379bf72864A825aD35b6D",
		),
		57073: addressSet(
			"0xeF684C38F94F48775959ECf2012D7E864ffb9dd4",
			"0x89415a82d909a7238d69094C3Dd1dCC1aCbDa85C",
			"0x10D8b8DaA26d307489803e10477De69C0492B610",
			"0x1bE0bCd689Eac8e37346934BfafE8cd0dD231eEE",
			"0x06C61D54958a0772Ee8aF41789466d39FfeaeB13",
		),
		59144: addressSet(
			"0x7E63A5f1a8F0B4d0934B2f2327DAED3F6bb2ee75",
			"0xE0BCff426509723B18D6b2f0D8F4602d143bE3e0",
			"0x10D8b8DaA26d307489803e10477De69C0492B610",
			"0x60eB88A83434f13095B0A138cdCBf5078Aa5005C",
		),
		81457: addressSet(
			"0x2D509190Ed0172ba588407D4c2df918F955Cc6E1",
			"0x89415a82d909a7238d69094C3Dd1dCC1aCbDa85C",
			"0x10D8b8DaA26d307489803e10477De69C0492B610",
		),
		534352: addressSet(
			"0x3baD7AD0728f9917d1Bf08af5782dCbD516cDd96",
			"0x89415a82d909a7238d69094C3Dd1dCC1aCbDa85C",
			"0x10D8b8DaA26d307489803e10477De69C0492B610",
		),
	},
}

func HasBridgeExecutionTargetPolicy(provider string, chainID int64) bool {
	allowedByProvider, ok := bridgeExecutionTargets[normalizeBridgeProvider(provider)]
	if !ok {
		return false
	}
	_, ok = allowedByProvider[chainID]
	return ok
}

func IsAllowedBridgeExecutionTarget(provider string, chainID int64, target string) bool {
	allowedByProvider, ok := bridgeExecutionTargets[normalizeBridgeProvider(provider)]
	if !ok {
		return false
	}
	allowedTargets, ok := allowedByProvider[chainID]
	if !ok {
		return false
	}
	normalized := normalizeBridgeExecutionTarget(target)
	if normalized == "" {
		return false
	}
	_, ok = allowedTargets[normalized]
	return ok
}

func addressSet(addresses ...string) map[string]struct{} {
	out := make(map[string]struct{}, len(addresses))
	for _, address := range addresses {
		if normalized := normalizeBridgeExecutionTarget(address); normalized != "" {
			out[normalized] = struct{}{}
		}
	}
	return out
}

func normalizeBridgeProvider(provider string) string {
	return strings.ToLower(strings.TrimSpace(provider))
}

func normalizeBridgeExecutionTarget(target string) string {
	clean := strings.TrimSpace(target)
	if !common.IsHexAddress(clean) {
		return ""
	}
	return strings.ToLower(common.HexToAddress(clean).Hex())
}

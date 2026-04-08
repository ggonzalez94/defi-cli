package multicall

import (
	"context"
	"fmt"
	"strings"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/ethclient"
	"github.com/ggonzalez94/defi-cli/internal/registry"
)

// Addr is the Multicall3 contract address deployed at the same address on all major EVM chains.
var Addr = common.HexToAddress("0xcA11bde05977b3631167028862bE2a173976CA11")

// Call represents a single call in a Multicall3.aggregate3 batch.
type Call struct {
	Target       common.Address
	AllowFailure bool
	CallData     []byte
}

// Result represents the outcome of a single call in a Multicall3.aggregate3 batch.
type Result struct {
	Success    bool
	ReturnData []byte
}

// Aggregate3 batches multiple contract calls into a single Multicall3.aggregate3 RPC round-trip.
func Aggregate3(ctx context.Context, client *ethclient.Client, calls []Call) ([]Result, error) {
	if len(calls) == 0 {
		return nil, nil
	}

	packed, err := mc3ABI.Pack("aggregate3", calls)
	if err != nil {
		return nil, fmt.Errorf("pack aggregate3: %w", err)
	}
	mc3 := Addr
	out, err := client.CallContract(ctx, ethereum.CallMsg{To: &mc3, Data: packed}, nil)
	if err != nil {
		return nil, fmt.Errorf("call aggregate3: %w", err)
	}
	decoded, err := mc3ABI.Unpack("aggregate3", out)
	if err != nil {
		return nil, fmt.Errorf("decode aggregate3: %w", err)
	}
	if len(decoded) == 0 {
		return nil, fmt.Errorf("empty aggregate3 response")
	}

	rawResults, ok := decoded[0].([]struct {
		Success    bool   `json:"success"`
		ReturnData []byte `json:"returnData"`
	})
	if !ok {
		return nil, fmt.Errorf("unexpected aggregate3 result type: %T", decoded[0])
	}

	results := make([]Result, len(rawResults))
	for i, r := range rawResults {
		results[i] = Result{Success: r.Success, ReturnData: r.ReturnData}
	}
	return results, nil
}

var mc3ABI = mustABI(registry.Multicall3ABI)

func mustABI(raw string) abi.ABI {
	parsed, err := abi.JSON(strings.NewReader(raw))
	if err != nil {
		panic(fmt.Sprintf("invalid ABI: %v", err))
	}
	return parsed
}

package execution

import "time"

type ActionStatus string

type StepStatus string

type StepType string

const (
	ActionStatusPlanned   ActionStatus = "planned"
	ActionStatusRunning   ActionStatus = "running"
	ActionStatusCompleted ActionStatus = "completed"
	ActionStatusFailed    ActionStatus = "failed"
)

const (
	StepStatusPending   StepStatus = "pending"
	StepStatusSimulated StepStatus = "simulated"
	StepStatusSubmitted StepStatus = "submitted"
	StepStatusConfirmed StepStatus = "confirmed"
	StepStatusFailed    StepStatus = "failed"
)

const (
	StepTypeApproval StepType = "approval"
	StepTypeSwap     StepType = "swap"
	StepTypeBridge   StepType = "bridge_send"
	StepTypeLend     StepType = "lend_call"
	StepTypeClaim    StepType = "claim"
)

type Constraints struct {
	SlippageBps int64  `json:"slippage_bps,omitempty"`
	Deadline    string `json:"deadline,omitempty"`
	Simulate    bool   `json:"simulate"`
}

type ActionStep struct {
	StepID          string            `json:"step_id"`
	Type            StepType          `json:"type"`
	Status          StepStatus        `json:"status"`
	ChainID         string            `json:"chain_id"`
	RPCURL          string            `json:"rpc_url,omitempty"`
	Description     string            `json:"description,omitempty"`
	Target          string            `json:"target"`
	Data            string            `json:"data"`
	Value           string            `json:"value"`
	ExpectedOutputs map[string]string `json:"expected_outputs,omitempty"`
	TxHash          string            `json:"tx_hash,omitempty"`
	Error           string            `json:"error,omitempty"`
}

type Action struct {
	ActionID     string                 `json:"action_id"`
	IntentType   string                 `json:"intent_type"`
	Provider     string                 `json:"provider,omitempty"`
	Status       ActionStatus           `json:"status"`
	ChainID      string                 `json:"chain_id"`
	FromAddress  string                 `json:"from_address,omitempty"`
	ToAddress    string                 `json:"to_address,omitempty"`
	InputAmount  string                 `json:"input_amount,omitempty"`
	CreatedAt    string                 `json:"created_at"`
	UpdatedAt    string                 `json:"updated_at"`
	Constraints  Constraints            `json:"constraints"`
	Steps        []ActionStep           `json:"steps"`
	Metadata     map[string]any         `json:"metadata,omitempty"`
	ProviderData map[string]interface{} `json:"provider_data,omitempty"`
}

func NewAction(actionID, intentType, chainID string, constraints Constraints) Action {
	now := time.Now().UTC().Format(time.RFC3339)
	return Action{
		ActionID:    actionID,
		IntentType:  intentType,
		Status:      ActionStatusPlanned,
		ChainID:     chainID,
		CreatedAt:   now,
		UpdatedAt:   now,
		Constraints: constraints,
		Steps:       []ActionStep{},
	}
}

func (a *Action) Touch() {
	a.UpdatedAt = time.Now().UTC().Format(time.RFC3339)
}

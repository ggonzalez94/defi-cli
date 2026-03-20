package execution

import (
	"crypto/rand"
	"encoding/hex"
	"fmt"
)

func NewActionID() string {
	b := make([]byte, 16)
	if _, err := rand.Read(b); err != nil {
		return "action-unknown"
	}
	return fmt.Sprintf("act_%s", hex.EncodeToString(b))
}

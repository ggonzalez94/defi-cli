package version

import "fmt"

var (
	CLIName    = "defi"
	CLIVersion = "0.1.0"
	Commit     = "unknown"
	BuildDate  = "unknown"
)

func Long() string {
	return fmt.Sprintf("%s (commit: %s, built: %s)", CLIVersion, Commit, BuildDate)
}

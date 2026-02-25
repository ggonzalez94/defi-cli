package version

import "fmt"

var (
	CLIName    = "defi"
	CLIVersion = "0.3.1"
	Commit     = "unknown"
	BuildDate  = "unknown"
)

func Long() string {
	return fmt.Sprintf("%s (commit: %s, built: %s)", CLIVersion, Commit, BuildDate)
}
